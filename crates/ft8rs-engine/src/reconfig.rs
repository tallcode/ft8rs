//! Reconfiguration planner (P1).
//!
//! Pure, deterministic logic that decides — given the engine's current desired
//! state and a newly submitted desired state — *what* the engine must do to
//! apply the change: which sinks/sessions/capture to rebuild, which cross-slot
//! state buckets to reset vs. migrate, and whether the change needs operator
//! confirmation (DX target switch).
//!
//! This is the single source of truth for the dynamic-switch rules. It has no
//! audio, no threads, no I/O, so it is exhaustively unit-tested below. The
//! actors (P2) merely execute the plan it returns.
//!
//! Nothing here touches `lib_wsjtx`/`lib_jtdx`.

use std::collections::BTreeSet;

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};

use crate::report::UdpConfig;

/// Cross-slot state buckets. Used to express what a reconfig resets vs.
/// preserves/migrates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateBucket {
    /// S0 capture: cpal stream, resampler carry, sample clock.
    Capture,
    /// S1 kernel scratch: per-slot decoder work buffers (reset every slot anyway).
    Scratch,
    /// S2 hash book: learned hash callsigns (migratable today).
    HashBook,
    /// S3 cross-slot AP memory: wsjtx A7, jtdx odd/even AP, hybrid evidence.
    ApMemory,
    /// S4 DX target intel (hiscall-derived): foci, parity, harvested grid, dt.
    DxTarget,
    /// S5 DX operator intel (mycall-derived): inferred parity, recipient freqs.
    DxOperator,
    /// S6 output sinks: UDP socket.
    Output,
}

/// The engine's desired state: input device, decode config, and output config.
/// The GUI submits a full `EngineState`; the planner diffs old vs. new.
#[derive(Clone, Debug)]
pub struct EngineState {
    /// Soundcard selector (index or name), or `None` for the default device.
    pub device: Option<String>,
    pub config: StreamDecodeConfig,
    pub udp: Option<UdpConfig>,
}

/// The plan the engine executes to move from one `EngineState` to another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconfigOutcome {
    /// L0: rebuild/drop the output sinks.
    pub rebuild_output: bool,
    /// L1: drop and rebuild the decode session (at the next slot boundary).
    pub rebuild_session: bool,
    /// L2: tear down and reopen the capture path (and re-align to UTC).
    pub restart_capture: bool,
    /// State buckets that must be reset (cannot be carried across the change).
    pub reset: BTreeSet<StateBucket>,
    /// State buckets carried into the rebuilt session.
    pub migrate: BTreeSet<StateBucket>,
    /// The change discards collected DX target intel; the GUI must confirm first.
    pub confirm_required: bool,
}

impl ReconfigOutcome {
    /// True when the new desired state is identical to the old one.
    pub fn is_noop(&self) -> bool {
        *self == ReconfigOutcome::default()
    }
}

/// GUI-changeable decode-config fields. Whole-struct equality is avoided because
/// `StreamDecodeConfig` carries `f64` fields and many knobs the GUI never edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigField {
    Profile,
    MyCall,
    MyGrid,
    HisCall,
    HisGrid,
    Nfa,
    Nfb,
    Nfqso,
    Swl,
    Nagain,
    Filter,
    HideDupes,
    HideHash,
}

/// Diff the GUI-exposed decode-config fields.
pub fn config_diff(old: &StreamDecodeConfig, new: &StreamDecodeConfig) -> BTreeSet<ConfigField> {
    let mut changed = BTreeSet::new();
    let mut note = |cond: bool, field: ConfigField| {
        if cond {
            changed.insert(field);
        }
    };
    note(old.profile != new.profile, ConfigField::Profile);
    note(old.mycall != new.mycall, ConfigField::MyCall);
    note(old.mygrid != new.mygrid, ConfigField::MyGrid);
    note(old.hiscall != new.hiscall, ConfigField::HisCall);
    note(old.hisgrid != new.hisgrid, ConfigField::HisGrid);
    note(old.nfa != new.nfa, ConfigField::Nfa);
    note(old.nfb != new.nfb, ConfigField::Nfb);
    note(old.nfqso != new.nfqso, ConfigField::Nfqso);
    note(old.swl != new.swl, ConfigField::Swl);
    note(old.nagain != new.nagain, ConfigField::Nagain);
    note(old.filter != new.filter, ConfigField::Filter);
    note(old.hide_dupes != new.hide_dupes, ConfigField::HideDupes);
    note(old.hide_hash != new.hide_hash, ConfigField::HideHash);
    changed
}

/// Compute what the engine must do to go from `old` to `new`.
///
/// When several things change in one submission, the actions combine: a device
/// change restarts capture *and* a config change rebuilds the session in the
/// same step. Reset buckets are the union; the level is implied by the booleans.
pub fn plan_reconfig(old: &EngineState, new: &EngineState) -> ReconfigOutcome {
    let mut out = ReconfigOutcome::default();

    // L0 — output sinks.
    if old.udp != new.udp {
        out.rebuild_output = true;
        out.reset.insert(StateBucket::Output);
    }

    // L2 — capture path (device). Decode session and all decode state (S1..S6)
    // are preserved: only the capture path restarts and re-aligns.
    if old.device != new.device {
        out.restart_capture = true;
        out.reset.insert(StateBucket::Capture);
    }

    // L1 — decode session (any GUI-exposed config field).
    let changed = config_diff(&old.config, &new.config);
    if !changed.is_empty() {
        out.rebuild_session = true;
        // S1 scratch is reset by definition (rebuilding the session). Hash book
        // migrates today; S3 (A7/AP/evidence) migration is deferred (§5.3), so
        // it is reset for now (a one-slot transient AP loss on user action).
        out.reset.insert(StateBucket::Scratch);
        out.migrate.insert(StateBucket::HashBook);
        out.reset.insert(StateBucket::ApMemory);

        let dx_involved =
            old.config.profile == DecodeProfile::Dx || new.config.profile == DecodeProfile::Dx;

        // Switching profile into or out of DX gains/loses the DX intel entirely.
        if changed.contains(&ConfigField::Profile) && dx_involved {
            out.reset.insert(StateBucket::DxTarget);
            out.reset.insert(StateBucket::DxOperator);
        }

        if dx_involved {
            // Changing the target (hiscall) invalidates ALL collected intel and
            // needs operator confirmation.
            if changed.contains(&ConfigField::HisCall) {
                out.reset.insert(StateBucket::DxTarget);
                out.reset.insert(StateBucket::DxOperator);
                out.confirm_required = true;
            }
            // Changing the operator call resets only S5 (mycall-derived intel);
            // FrequencyOrigin lets the DX store keep target-derived candidates,
            // the observed parity, and the harvested grid (§6.5).
            if changed.contains(&ConfigField::MyCall) {
                out.reset.insert(StateBucket::DxOperator);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> EngineState {
        EngineState {
            device: Some("0".to_string()),
            config: StreamDecodeConfig::default(),
            udp: None,
        }
    }

    fn dx_base() -> EngineState {
        let mut state = base();
        state.config.profile = DecodeProfile::Dx;
        state.config.hiscall = Some("UA3QNA".to_string());
        state.config.mycall = Some("F1MLZ".to_string());
        state
    }

    fn buckets(items: &[StateBucket]) -> BTreeSet<StateBucket> {
        items.iter().copied().collect()
    }

    #[test]
    fn no_change_is_noop() {
        let s = base();
        assert!(plan_reconfig(&s, &s).is_noop());
    }

    #[test]
    fn udp_toggle_is_output_only() {
        let old = base();
        let mut new = base();
        new.udp = Some(UdpConfig {
            host: "127.0.0.1".to_string(),
            port: 2238,
        });
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_output);
        assert!(!out.rebuild_session);
        assert!(!out.restart_capture);
        assert_eq!(out.reset, buckets(&[StateBucket::Output]));
        assert!(out.migrate.is_empty());
    }

    #[test]
    fn device_change_restarts_capture_only_and_preserves_session() {
        let old = base();
        let mut new = base();
        new.device = Some("1".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.restart_capture);
        assert!(!out.rebuild_session);
        assert!(!out.rebuild_output);
        // Only capture resets; decode/session/DX state (S1..S6) preserved.
        assert_eq!(out.reset, buckets(&[StateBucket::Capture]));
    }

    #[test]
    fn device_change_in_dx_keeps_dx_intel() {
        let old = dx_base();
        let mut new = dx_base();
        new.device = Some("2".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.restart_capture);
        assert!(!out.rebuild_session);
        assert!(!out.reset.contains(&StateBucket::DxTarget));
        assert!(!out.reset.contains(&StateBucket::DxOperator));
    }

    #[test]
    fn frequency_window_change_rebuilds_session_migrates_hash() {
        let old = base();
        let mut new = base();
        new.config.nfa = 300.0;
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(!out.restart_capture);
        assert!(out.reset.contains(&StateBucket::Scratch));
        assert!(out.reset.contains(&StateBucket::ApMemory));
        assert!(out.migrate.contains(&StateBucket::HashBook));
        assert!(!out.confirm_required);
        // Non-DX: no DX buckets touched.
        assert!(!out.reset.contains(&StateBucket::DxTarget));
    }

    #[test]
    fn sensitivity_toggles_rebuild_session() {
        for mutate in [
            (|c: &mut StreamDecodeConfig| c.swl = true) as fn(&mut StreamDecodeConfig),
            |c: &mut StreamDecodeConfig| c.nagain = true,
            |c: &mut StreamDecodeConfig| c.filter = true,
            |c: &mut StreamDecodeConfig| c.hide_dupes = true,
            |c: &mut StreamDecodeConfig| c.hide_hash = true,
        ] {
            let old = base();
            let mut new = base();
            mutate(&mut new.config);
            let out = plan_reconfig(&old, &new);
            assert!(out.rebuild_session, "expected session rebuild");
            assert!(out.migrate.contains(&StateBucket::HashBook));
            assert!(!out.confirm_required);
        }
    }

    #[test]
    fn mycall_change_outside_dx_has_no_dx_reset() {
        let old = base();
        let mut new = base();
        new.config.mycall = Some("K1ABC".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(!out.reset.contains(&StateBucket::DxTarget));
        assert!(!out.reset.contains(&StateBucket::DxOperator));
        assert!(!out.confirm_required);
    }

    #[test]
    fn hiscall_change_outside_dx_does_not_confirm() {
        let old = base();
        let mut new = base();
        new.config.hiscall = Some("W9XYZ".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(!out.confirm_required);
        assert!(!out.reset.contains(&StateBucket::DxTarget));
    }

    #[test]
    fn hiscall_change_in_dx_resets_intel_and_requires_confirm() {
        let old = dx_base();
        let mut new = dx_base();
        new.config.hiscall = Some("DL8YHR".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(out.confirm_required);
        assert!(out.reset.contains(&StateBucket::DxTarget));
        assert!(out.reset.contains(&StateBucket::DxOperator));
    }

    #[test]
    fn mycall_change_in_dx_resets_intel_without_confirm() {
        let old = dx_base();
        let mut new = dx_base();
        new.config.mycall = Some("K1JT".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(!out.confirm_required);
        // FrequencyOrigin lets us keep target intel (S4); only S5 resets.
        assert!(!out.reset.contains(&StateBucket::DxTarget));
        assert!(out.reset.contains(&StateBucket::DxOperator));
    }

    #[test]
    fn profile_change_into_dx_resets_dx_intel() {
        let old = base(); // wsjtx
        let mut new = base();
        new.config.profile = DecodeProfile::Dx;
        new.config.hiscall = Some("UA3QNA".to_string());
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(out.reset.contains(&StateBucket::DxTarget));
        assert!(out.reset.contains(&StateBucket::DxOperator));
    }

    #[test]
    fn profile_change_between_non_dx_leaves_dx_buckets_alone() {
        let old = base(); // wsjtx
        let mut new = base();
        new.config.profile = DecodeProfile::Jtdx;
        let out = plan_reconfig(&old, &new);
        assert!(out.rebuild_session);
        assert!(out.reset.contains(&StateBucket::Scratch));
        assert!(!out.reset.contains(&StateBucket::DxTarget));
        assert!(!out.reset.contains(&StateBucket::DxOperator));
    }

    #[test]
    fn device_and_config_change_combine() {
        let old = base();
        let mut new = base();
        new.device = Some("1".to_string());
        new.config.nfqso = 1153.0;
        let out = plan_reconfig(&old, &new);
        assert!(out.restart_capture);
        assert!(out.rebuild_session);
        assert!(out.reset.contains(&StateBucket::Capture));
        assert!(out.reset.contains(&StateBucket::Scratch));
        assert!(out.migrate.contains(&StateBucket::HashBook));
    }
}
