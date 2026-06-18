//! DX chase orchestration profile.
//!
//! This module intentionally stays outside `lib_wsjtx` and `lib_jtdx`. It builds
//! worker sessions with existing public configuration fields, filters output to
//! the operator-selected target, and later owns the DX-specific cross-slot
//! context.

use crate::decode::lib_jtdx::ft8_params::NFFT1_LONG;
use crate::decode::lib_jtdx::ft8b::{dx_symbol_field, DxSymbolSeed, Ft8bWorkspace};
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::decode::lib_jtdx::{ft8_decode, sync8};
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage,
    StreamDeepConfidence, StreamSnrSource,
};
use crate::stream::time::SlotTimestamp;

use std::time::Duration;

mod context;
mod deepsearch;
mod filter;
mod stack;

use context::{DeepFieldInput, TargetContextStore};
use deepsearch::{build_v1_hypotheses, DeepConfidence, DeepHit, DeepSearchGate};
use filter::{normalize_message, DxTarget};
use stack::PhysicalAdmissionGate;

const DX_DEEP_SNR_UNAVAILABLE: f64 = -99.0;

pub struct DxStreamDecodeSession {
    base_config: StreamDecodeConfig,
    target: DxTarget,
    context: TargetContextStore,
    hash_seed_calls: Vec<String>,
    listen: JtdxStreamDecodeSession,
    exposure: DxExposure,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DxExposure {
    pub slots: usize,
    pub focus_trials: usize,
    pub field_trials: usize,
    pub hypothesis_trials: usize,
    pub stack_osd_candidates: usize,
    pub stack_osd_attempts: usize,
    pub stack_osd_skipped_budget: usize,
    pub deep_rows_emitted: usize,
}

impl DxExposure {
    pub fn accumulate(&mut self, other: Self) {
        self.slots += other.slots;
        self.focus_trials += other.focus_trials;
        self.field_trials += other.field_trials;
        self.hypothesis_trials += other.hypothesis_trials;
        self.stack_osd_candidates += other.stack_osd_candidates;
        self.stack_osd_attempts += other.stack_osd_attempts;
        self.stack_osd_skipped_budget += other.stack_osd_skipped_budget;
        self.deep_rows_emitted += other.deep_rows_emitted;
    }
}

struct FocusDecodeOutput {
    rows: Vec<StreamDecodedMessage>,
    deep_fields: Vec<DeepFieldObservation>,
}

struct DeepFieldObservation {
    focus: f64,
    field: crate::decode::lib_jtdx::ft8b::DxSymbolField,
}

struct FocusDecodeJob<'a> {
    base_config: &'a StreamDecodeConfig,
    hash_seed_calls: &'a [String],
    hisgrid: Option<&'a str>,
    focus: f64,
    target_dt: Option<f64>,
    qso_progress: Option<usize>,
    timestamp: &'a SlotTimestamp,
    samples: &'a [f32],
    started_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, Default)]
struct DxSlotDeepReport {
    foci: usize,
    fields: usize,
    hits: usize,
    emitted: usize,
    crc_candidates: usize,
    crc_attempts: usize,
    crc_skipped_budget: usize,
}

struct DxSlotSnapshot {
    foci: Vec<f64>,
    target_dt: Option<f64>,
    qso_progress: Option<usize>,
    hisgrid: Option<String>,
    hypotheses: Vec<deepsearch::Hypothesis>,
}

impl DxStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        let target = DxTarget::new(config.hiscall.as_deref().unwrap_or_default());
        let hash_seed_calls = dx_hash_seed_calls(&config);
        let mut listen = JtdxStreamDecodeSession::new(dx_listen_config(&config));
        listen.import_hash_calls(&hash_seed_calls);
        let context = TargetContextStore::new(
            target.clone(),
            config.mycall.as_deref(),
            config.nfqso,
            config.hisgrid.as_deref(),
            config.lhound,
            config.nfa,
            config.nfb,
        );
        Self {
            base_config: config,
            target,
            context,
            hash_seed_calls,
            listen,
            exposure: DxExposure::default(),
        }
    }

    pub fn exposure(&self) -> DxExposure {
        self.exposure
    }

    pub fn decode_slot_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
    ) -> Vec<StreamDecodedMessage> {
        self.decode_slot_streaming_at(timestamp, samples, |_| Ok(()))
            .expect("in-memory DX decode callback cannot fail")
    }

    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let started_at = std::time::Instant::now();
        let snapshot = self.slot_snapshot(timestamp);
        self.exposure.slots += 1;
        self.exposure.focus_trials += snapshot.foci.len();
        let mut emitted = Vec::new();
        let listen_results =
            self.listen
                .decode_slot_streaming_at(timestamp, samples, |decode| {
                    emit_target_row(&self.context, &mut emitted, decode, &mut on_decode)?;
                    Ok(())
                })?;
        self.context.harvest_listen(timestamp, &listen_results);

        // Focused recovery runs one fully isolated disposable worker per focus,
        // so the foci are mutually independent and share no kernel state. Decode
        // them concurrently for wall-clock, then merge in the deterministic foci
        // order: every worker's rows are byte-identical to a serial run (no
        // shared residual/AP/hash state across foci, and each kernel stays
        // single-threaded internally), and emit/harvest stay reproducible
        // because they replay the sorted foci order, never thread-completion
        // order.
        let focus_outputs =
            self.decode_foci_concurrently(timestamp, samples, &snapshot, started_at)?;

        let mut focused_target_rows = Vec::new();
        let mut deep_observations = Vec::new();
        for output in focus_outputs {
            for decode in &output.rows {
                emit_target_row(&self.context, &mut emitted, decode, &mut on_decode)?;
            }
            focused_target_rows.extend(
                output
                    .rows
                    .into_iter()
                    .filter(|row| self.target.matches_message(&row.msg)),
            );
            deep_observations.extend(output.deep_fields);
        }
        let deep_inputs: Vec<DeepFieldInput<'_>> = deep_observations
            .iter()
            .map(|observation| DeepFieldInput {
                focus: observation.focus,
                field: &observation.field,
            })
            .collect();
        self.exposure.field_trials += deep_inputs.len();
        self.exposure.hypothesis_trials += deep_inputs.len() * snapshot.hypotheses.len();
        let diagnostics_before_deep = self.context.deep_diagnostics();
        let deep_hits = self.context.observe_deep_fields(
            timestamp,
            &deep_inputs,
            &snapshot.hypotheses,
            DeepSearchGate::default(),
            PhysicalAdmissionGate::default(),
        );
        let diagnostics_after_deep = self.context.deep_diagnostics();
        let deep_hit_count = deep_hits.len();
        let deep_emitted = emit_deep_hits_if_enabled(
            &self.base_config,
            &self.context,
            &self.target,
            &mut emitted,
            &mut focused_target_rows,
            deep_hits,
            &mut on_decode,
        )?;
        maybe_print_deep_report(
            &self.base_config,
            timestamp,
            DxSlotDeepReport {
                foci: snapshot.foci.len(),
                fields: deep_inputs.len(),
                hits: deep_hit_count,
                emitted: deep_emitted,
                crc_candidates: diagnostics_after_deep
                    .crc_candidates
                    .saturating_sub(diagnostics_before_deep.crc_candidates),
                crc_attempts: diagnostics_after_deep
                    .crc_attempts
                    .saturating_sub(diagnostics_before_deep.crc_attempts),
                crc_skipped_budget: diagnostics_after_deep
                    .crc_skipped_budget
                    .saturating_sub(diagnostics_before_deep.crc_skipped_budget),
            },
        );
        self.exposure.stack_osd_candidates += diagnostics_after_deep
            .crc_candidates
            .saturating_sub(diagnostics_before_deep.crc_candidates);
        self.exposure.stack_osd_attempts += diagnostics_after_deep
            .crc_attempts
            .saturating_sub(diagnostics_before_deep.crc_attempts);
        self.exposure.stack_osd_skipped_budget += diagnostics_after_deep
            .crc_skipped_budget
            .saturating_sub(diagnostics_before_deep.crc_skipped_budget);
        self.exposure.deep_rows_emitted += deep_emitted;
        self.context
            .harvest_focused(timestamp, &focused_target_rows);

        Ok(emitted)
    }

    fn slot_snapshot(&self, timestamp: &SlotTimestamp) -> DxSlotSnapshot {
        let run_focused = self.context.should_run_focused(timestamp);
        let hisgrid = self.context.hisgrid().map(str::to_string);
        let hypotheses = build_v1_hypotheses(
            self.base_config.mycall.as_deref(),
            self.base_config.hiscall.as_deref().unwrap_or_default(),
            hisgrid.as_deref(),
        );
        DxSlotSnapshot {
            foci: if run_focused {
                self.context.selected_foci()
            } else {
                Vec::new()
            },
            target_dt: self.context.target_dt(),
            qso_progress: self.context.qso_progress(),
            hisgrid,
            hypotheses,
        }
    }

    /// Decode every focus on its own thread and return the raw rows per focus in
    /// the same order as `foci` (deterministic). Each closure builds, seeds, and
    /// drops its own disposable workers, so nothing is shared between threads but
    /// immutable config/seed/grid references; the returned rows are merged by the
    /// caller in foci order to keep emit/harvest reproducible.
    fn decode_foci_concurrently(
        &self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        snapshot: &DxSlotSnapshot,
        started_at: std::time::Instant,
    ) -> Result<Vec<FocusDecodeOutput>, String> {
        if snapshot.foci.is_empty() {
            return Ok(Vec::new());
        }
        let base_config = &self.base_config;
        let hash_seed_calls = &self.hash_seed_calls;

        std::thread::scope(|scope| {
            let handles: Vec<_> = snapshot
                .foci
                .iter()
                .map(|&focus| {
                    let hisgrid = snapshot.hisgrid.as_deref();
                    let target_dt = snapshot.target_dt;
                    let qso_progress = snapshot.qso_progress;
                    scope.spawn(move || {
                        decode_one_focus(FocusDecodeJob {
                            base_config,
                            hash_seed_calls,
                            hisgrid,
                            focus,
                            target_dt,
                            qso_progress,
                            timestamp,
                            samples,
                            started_at,
                        })
                    })
                })
                .collect();

            let mut outputs = Vec::with_capacity(handles.len());
            for handle in handles {
                let rows = handle
                    .join()
                    .map_err(|_| "dx focus worker panicked".to_string())??;
                outputs.push(rows);
            }
            Ok(outputs)
        })
    }
}

/// Run the focused JTDX `swl+nagain` worker and, when a grid is known, the a8d
/// WSJT-X worker for a single focus. Returns every decoded row (JTDX first, then
/// a8d) so the caller can emit/harvest them in a deterministic order. Both
/// workers are fresh and disposable, matching the serial path exactly.
fn decode_one_focus(job: FocusDecodeJob<'_>) -> Result<FocusDecodeOutput, String> {
    let mut rows = Vec::new();
    if dx_focused_budget_expired(job.base_config, job.started_at.elapsed()) {
        return Ok(FocusDecodeOutput {
            rows,
            deep_fields: Vec::new(),
        });
    }
    let mut focused = JtdxStreamDecodeSession::new(dx_focus_config(
        job.base_config,
        job.focus,
        job.qso_progress,
    ));
    focused.import_hash_calls(job.hash_seed_calls);
    rows.extend(focused.decode_slot_streaming_at(job.timestamp, job.samples, |_| Ok(()))?);

    if let Some(hisgrid) = job.hisgrid.filter(|_| job.base_config.mycall.is_some()) {
        if dx_focused_budget_expired(job.base_config, job.started_at.elapsed()) {
            return Ok(FocusDecodeOutput {
                rows,
                deep_fields: Vec::new(),
            });
        }
        // Do not skip a8d just because the JTDX focused pass already found a
        // target row near this focus. In FH/multi-stream traffic, another
        // target-related message at the same focus can still be the one a8d
        // recovers.
        let mut wsjtx = StreamDecodeSession::new(dx_a8_config(job.base_config, job.focus, hisgrid));
        wsjtx.import_hash_calls(job.hash_seed_calls);
        rows.extend(wsjtx.decode_slot_streaming_at(job.timestamp, job.samples, |_| Ok(()))?);
    }
    let deep_fields = if dx_deep_engine_enabled(job.base_config) {
        run_deep_probe_fields(
            job.base_config,
            job.focus,
            job.target_dt,
            job.qso_progress,
            job.samples,
        )
        .into_iter()
        .map(|field| DeepFieldObservation {
            focus: job.focus,
            field,
        })
        .collect()
    } else {
        Vec::new()
    };
    Ok(FocusDecodeOutput { rows, deep_fields })
}

fn run_deep_probe_fields(
    base_config: &StreamDecodeConfig,
    focus: f64,
    target_dt: Option<f64>,
    qso_progress: Option<usize>,
    samples: &[f32],
) -> Vec<crate::decode::lib_jtdx::ft8b::DxSymbolField> {
    if base_config.hiscall.is_none() {
        return Vec::new();
    }

    let mut dd8 = vec![0.0f32; NFFT1_LONG];
    for (dst, src) in dd8.iter_mut().zip(samples.iter().copied()) {
        *dst = src;
    }
    let mut workspace = Ft8bWorkspace::default();
    let focused = dx_focus_config(base_config, focus, qso_progress);
    let seeds = deep_symbol_seeds(&focused, &dd8, focus, target_dt);
    let mut fields = Vec::new();
    for seed in seeds {
        if let Some(field) = dx_symbol_field(&mut dd8, &mut workspace, &focused, seed) {
            fields.push(field);
        }
    }
    fields
}

fn deep_symbol_seeds(
    focused: &StreamDecodeConfig,
    dd8: &[f32],
    focus: f64,
    target_dt: Option<f64>,
) -> Vec<DxSymbolSeed> {
    let mut seeds = Vec::new();
    if let Some(xdt0) = target_dt {
        push_unique_seed(
            &mut seeds,
            DxSymbolSeed {
                freq: focus as f32,
                xdt0: xdt0 as f32,
            },
        );
    }

    let syncmin = ft8_decode::syncmin(focused, 1);
    let sync8_config = sync8::Sync8Config::from_stream(focused, 1, syncmin, 0.0);
    let mut candidates = sync8::sync8(dd8, sync8_config);
    candidates.sort_by(|a, b| {
        (a.freq - focus as f32)
            .abs()
            .total_cmp(&(b.freq - focus as f32).abs())
            .then_with(|| b.sync.total_cmp(&a.sync))
            .then_with(|| a.dt.total_cmp(&b.dt))
    });

    for candidate in candidates {
        if candidate.sync <= 0.0 || !candidate.sync.is_finite() {
            continue;
        }
        if (candidate.freq as f64 - focus).abs() > 25.0 {
            continue;
        }
        push_unique_seed(
            &mut seeds,
            DxSymbolSeed {
                freq: candidate.freq,
                xdt0: candidate.dt,
            },
        );
        if seeds.len() >= 3 {
            break;
        }
    }
    if seeds.is_empty() {
        push_unique_seed(
            &mut seeds,
            DxSymbolSeed {
                freq: focus as f32,
                xdt0: 0.0,
            },
        );
    }
    seeds
}

fn push_unique_seed(seeds: &mut Vec<DxSymbolSeed>, seed: DxSymbolSeed) {
    if seeds.iter().any(|existing| {
        (existing.freq - seed.freq).abs() <= 3.0 && (existing.xdt0 - seed.xdt0).abs() <= 0.08
    }) {
        return;
    }
    seeds.push(seed);
}

fn dx_listen_config(config: &StreamDecodeConfig) -> StreamDecodeConfig {
    let mut listen = config.clone_for_profile_jtdx();
    listen.swl = true;
    listen.nagain = false;
    listen.filter = false;
    listen.mycall = None;
    listen.hiscall = None;
    listen.hisgrid = None;
    listen.lhound = false;
    listen
}

fn dx_focus_config(
    config: &StreamDecodeConfig,
    focus: f64,
    qso_progress: Option<usize>,
) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_jtdx();
    let focus = focus.clamp(config.nfa, config.nfb);
    focused.profile = DecodeProfile::Jtdx;
    focused.swl = true;
    focused.nagain = true;
    focused.filter = false;
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    if let Some(qso_progress) = qso_progress {
        focused.nQSOProgress = qso_progress;
    }
    focused
}

fn dx_a8_config(config: &StreamDecodeConfig, focus: f64, hisgrid: &str) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_wsjt_x();
    let focus = focus.clamp(config.nfa, config.nfb);
    focused.profile = DecodeProfile::Wsjtx;
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    focused.hisgrid = Some(hisgrid.to_string());
    focused.lft8apon = true;
    focused
}

fn dx_hash_seed_calls(config: &StreamDecodeConfig) -> Vec<String> {
    let mut calls = Vec::new();
    push_normalized_call(&mut calls, config.hiscall.as_deref());
    push_normalized_call(&mut calls, config.mycall.as_deref());
    calls
}

fn emit_target_row<F>(
    context: &TargetContextStore,
    emitted: &mut Vec<StreamDecodedMessage>,
    decode: &StreamDecodedMessage,
    on_decode: &mut F,
) -> Result<bool, String>
where
    F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
{
    if !context.should_emit_target_row(decode)
        || emitted.iter().any(|row| is_same_signal(row, decode))
    {
        return Ok(false);
    }
    emitted.push(decode.clone());
    on_decode(decode)?;
    Ok(true)
}

fn emit_deep_hits_if_enabled<F>(
    config: &StreamDecodeConfig,
    context: &TargetContextStore,
    target: &DxTarget,
    emitted: &mut Vec<StreamDecodedMessage>,
    focused_target_rows: &mut Vec<StreamDecodedMessage>,
    deep_hits: Vec<DeepHit>,
    on_decode: &mut F,
) -> Result<usize, String>
where
    F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
{
    if !dx_deep_output_enabled(config) {
        return Ok(0);
    }
    let mut deep_emitted = 0usize;
    for deep_hit in deep_hits {
        let row = deep_hit_to_row(&deep_hit);
        let row_emitted = emit_target_row(context, emitted, &row, on_decode)?;
        if row_emitted {
            deep_emitted += 1;
        }
        if row_emitted && target.matches_message(&row.msg) {
            focused_target_rows.push(row);
        }
    }
    Ok(deep_emitted)
}

fn deep_hit_to_row(hit: &DeepHit) -> StreamDecodedMessage {
    let (snr, snr_source) = match hit.snr {
        Some(snr) => (snr as f64, StreamSnrSource::DxDeepEstimated),
        None => (DX_DEEP_SNR_UNAVAILABLE, StreamSnrSource::DxDeepUnavailable),
    };
    StreamDecodedMessage {
        freq: hit.freq,
        dt: hit.dt,
        snr,
        snr_source,
        deep_confidence: Some(stream_deep_confidence(hit.conf)),
        msg: hit.msg.clone(),
        sync: hit.stat as f64,
        itone: [0; 79],
    }
}

fn stream_deep_confidence(conf: DeepConfidence) -> StreamDeepConfidence {
    match conf {
        DeepConfidence::TwoSlotMatched => StreamDeepConfidence::TwoSlotMatched,
        DeepConfidence::StackedLlrMatched => StreamDeepConfidence::StackedLlrMatched,
        DeepConfidence::CrcConfirmedExperimental => StreamDeepConfidence::CrcConfirmedExperimental,
    }
}

fn dx_deep_output_enabled(config: &StreamDecodeConfig) -> bool {
    config.dx_deep_experimental_output
}

/// Whether the deep-integration engine (T1/T2 field extraction + stacking) runs
/// at all this slot. The engine is pure cost for a normal `--profile dx` user —
/// ~15 extra 192k FFTs per slot plus sync8/matched-filter/OSD work — and only
/// pays off when its rows can surface (`--dx-deep-experimental-output`) or when
/// its diagnostics are requested (`--dx-deep-diagnostics`). Gate the whole probe
/// on those flags so the default path stays at baseline speed.
fn dx_deep_engine_enabled(config: &StreamDecodeConfig) -> bool {
    config.dx_deep_experimental_output || config.dx_deep_diagnostics
}

fn maybe_print_deep_report(
    config: &StreamDecodeConfig,
    timestamp: &SlotTimestamp,
    report: DxSlotDeepReport,
) {
    if !config.dx_deep_diagnostics {
        return;
    }
    eprintln!("{}", format_deep_report(timestamp, report));
}

fn format_deep_report(timestamp: &SlotTimestamp, report: DxSlotDeepReport) -> String {
    format!(
        "dx-deep {timestamp}: foci={} fields={} hits={} emitted={} crc_candidates={} crc_attempts={} crc_skipped={}",
        report.foci,
        report.fields,
        report.hits,
        report.emitted,
        report.crc_candidates,
        report.crc_attempts,
        report.crc_skipped_budget
    )
}

fn dx_focused_budget_expired(config: &StreamDecodeConfig, elapsed: Duration) -> bool {
    config
        .dx_monitor_watchdog_ms
        .is_some_and(|budget_ms| elapsed >= Duration::from_millis(budget_ms))
}

fn is_same_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    normalize_message(&a.msg) == normalize_message(&b.msg)
        && (a.freq - b.freq).abs() <= 3.0
        && (a.dt - b.dt).abs() <= 0.3
}

fn push_normalized_call(calls: &mut Vec<String>, call: Option<&str>) {
    let Some(call) = call else {
        return;
    };
    let call = call.trim();
    if call.is_empty() {
        return;
    }
    let call = call.to_ascii_uppercase();
    if !calls.iter().any(|existing| existing == &call) {
        calls.push(call);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::dx::deepsearch::{
        dx_deep_score, dx_deep_search, estimate_message_snr, DeepConfidence, DeepHit,
    };
    use crate::decode::lib_jtdx::ft8v2::encode174_91::encode174_91;
    use crate::decode::lib_jtdx::gen_ft8wave::{gen_ft8wave, NFRAME};
    use crate::decode::lib_jtdx::genft8::genft8;
    use crate::input::audio::{read_wav_mono_f32, resample_linear};
    use crate::stream::session::DecodeProfile;

    #[test]
    fn dx_listen_config_pins_primary_pass() {
        let mut config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            swl: false,
            nagain: true,
            filter: true,
            lhound: true,
            mycall: Some("F1MLZ".to_string()),
            hiscall: Some("UA3QNA".to_string()),
            hisgrid: Some("KO95".to_string()),
            ..StreamDecodeConfig::default()
        };
        config.nfa = 100.0;
        config.nfb = 3200.0;
        config.nfqso = 1152.0;

        let listen = dx_listen_config(&config);

        assert_eq!(listen.profile, DecodeProfile::Jtdx);
        assert!(listen.swl);
        assert!(!listen.nagain);
        assert!(!listen.filter);
        assert!(!listen.lhound);
        assert_eq!(listen.nfa, 100.0);
        assert_eq!(listen.nfb, 3200.0);
        assert_eq!(listen.nfqso, 1152.0);
        assert!(listen.mycall.is_none());
        assert!(listen.hiscall.is_none());
        assert!(listen.hisgrid.is_none());
    }

    #[test]
    fn dx_hash_seed_calls_keeps_target_calls_only_once() {
        let config = StreamDecodeConfig {
            mycall: Some(" f1mlz ".to_string()),
            hiscall: Some("F1MLZ".to_string()),
            ..StreamDecodeConfig::default()
        };

        assert_eq!(dx_hash_seed_calls(&config), vec!["F1MLZ".to_string()]);
    }

    #[test]
    fn dx_focus_config_enables_focused_deep_only_for_focus() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            nQSOProgress: 1,
            mycall: Some("F1MLZ".to_string()),
            hiscall: Some("UA3QNA".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_focus_config(&config, 1152.0, None);

        assert_eq!(focused.profile, DecodeProfile::Jtdx);
        assert!(focused.swl);
        assert!(focused.nagain);
        assert_eq!(focused.nfqso, 1152.0);
        assert_eq!(focused.nfa, 1127.0);
        assert_eq!(focused.nfb, 1177.0);
        assert_eq!(focused.mycall.as_deref(), Some("F1MLZ"));
        assert_eq!(focused.hiscall.as_deref(), Some("UA3QNA"));
        assert_eq!(focused.nQSOProgress, 1);

        let progressed = dx_focus_config(&config, 1152.0, Some(3));
        assert_eq!(progressed.nQSOProgress, 3);
    }

    #[test]
    fn dx_a8_config_builds_focused_wsjtx_worker_config() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_a8_config(&config, 1000.0, "PM00");

        assert_eq!(focused.profile, DecodeProfile::Wsjtx);
        assert_eq!(focused.nfqso, 1000.0);
        assert_eq!(focused.nfa, 975.0);
        assert_eq!(focused.nfb, 1025.0);
        assert_eq!(focused.mycall.as_deref(), Some("K1JT"));
        assert_eq!(focused.hiscall.as_deref(), Some("BG5ATV"));
        assert_eq!(focused.hisgrid.as_deref(), Some("PM00"));
        assert!(focused.lft8apon);
    }

    #[test]
    fn dx_focused_configs_clamp_out_of_band_focus() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_focus_config(&config, 3500.0, None);
        assert_eq!(focused.nfqso, 3000.0);
        assert_eq!(focused.nfa, 2975.0);
        assert_eq!(focused.nfb, 3000.0);

        let a8 = dx_a8_config(&config, 100.0, "PM00");
        assert_eq!(a8.nfqso, 200.0);
        assert_eq!(a8.nfa, 200.0);
        assert_eq!(a8.nfb, 225.0);
    }

    #[test]
    fn dx_monitor_watchdog_is_opt_in_and_budget_based() {
        let mut config = StreamDecodeConfig::default();

        assert!(!dx_focused_budget_expired(
            &config,
            Duration::from_secs(999)
        ));

        config.dx_monitor_watchdog_ms = Some(12_000);
        assert!(!dx_focused_budget_expired(
            &config,
            Duration::from_millis(11_999)
        ));
        assert!(dx_focused_budget_expired(
            &config,
            Duration::from_millis(12_000)
        ));
    }

    #[test]
    fn deep_hit_to_row_marks_snr_unavailable_explicitly() {
        let hit = DeepHit {
            msg: "K1JT BG5ATV -10".to_string(),
            stat: 123.5,
            margin: 12.0,
            freq: 1000.25,
            dt: 0.42,
            snr: None,
            conf: DeepConfidence::CrcConfirmedExperimental,
        };

        let row = deep_hit_to_row(&hit);

        assert_eq!(row.msg, "K1JT BG5ATV -10");
        assert_eq!(row.freq, 1000.25);
        assert_eq!(row.dt, 0.42);
        assert_eq!(row.snr, DX_DEEP_SNR_UNAVAILABLE);
        assert_eq!(row.snr_source, StreamSnrSource::DxDeepUnavailable);
        assert_eq!(
            row.deep_confidence,
            Some(StreamDeepConfidence::CrcConfirmedExperimental)
        );
        assert_eq!(row.sync, 123.5);
        assert_eq!(row.itone, [0; 79]);
    }

    #[test]
    fn deep_hit_to_row_uses_estimated_snr_when_available() {
        let hit = DeepHit {
            msg: "K1JT BG5ATV -10".to_string(),
            stat: 123.5,
            margin: 12.0,
            freq: 1000.25,
            dt: 0.42,
            snr: Some(-18.4),
            conf: DeepConfidence::CrcConfirmedExperimental,
        };

        let row = deep_hit_to_row(&hit);

        assert!((row.snr - -18.4).abs() < 1e-6);
        assert_eq!(row.snr_source, StreamSnrSource::DxDeepEstimated);
        assert_eq!(
            row.deep_confidence,
            Some(StreamDeepConfidence::CrcConfirmedExperimental)
        );
    }

    #[test]
    fn dx_deep_output_is_experimental_opt_in() {
        let mut config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            ..StreamDecodeConfig::default()
        };

        assert!(!dx_deep_output_enabled(&config));
        config.dx_deep_experimental_output = true;
        assert!(dx_deep_output_enabled(&config));
    }

    #[test]
    fn dx_deep_rows_are_runtime_suppressed_without_experimental_output() {
        let expected_msg = "K1JT BG5ATV -10";
        let base_config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&base_config, 1000.0, None);
        let slots: Vec<Vec<f32>> = (0..4)
            .map(|idx| {
                synthetic_ft8_slot_with_noise(
                    expected_msg,
                    1000.0,
                    0.003,
                    0.08,
                    0x5eed_0000 + idx as u64,
                )
            })
            .collect();

        let hit = synthetic_stack_recovery_hit(&focused, &slots, expected_msg)
            .expect("deep stack should internally recover this weak repeated target");
        assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
        assert_eq!(normalize_message(&hit.msg), normalize_message(expected_msg));

        let context = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        );
        let target = DxTarget::new("BG5ATV");
        let mut emitted = Vec::new();
        let mut focused_target_rows = Vec::new();
        let mut callback_rows = Vec::new();
        let deep_emitted = emit_deep_hits_if_enabled(
            &base_config,
            &context,
            &target,
            &mut emitted,
            &mut focused_target_rows,
            vec![hit.clone()],
            &mut |row| {
                callback_rows.push(row.clone());
                Ok(())
            },
        )
        .expect("default deep emit gate should not fail");

        assert_eq!(deep_emitted, 0);
        assert!(emitted.is_empty());
        assert!(focused_target_rows.is_empty());
        assert!(callback_rows.is_empty());

        let experimental_config = StreamDecodeConfig {
            dx_deep_experimental_output: true,
            ..base_config
        };
        let deep_emitted = emit_deep_hits_if_enabled(
            &experimental_config,
            &context,
            &target,
            &mut emitted,
            &mut focused_target_rows,
            vec![hit],
            &mut |row| {
                callback_rows.push(row.clone());
                Ok(())
            },
        )
        .expect("experimental deep emit gate should not fail");

        assert_eq!(deep_emitted, 1);
        assert_eq!(emitted.len(), 1);
        assert_eq!(focused_target_rows.len(), 1);
        assert_eq!(callback_rows.len(), 1);
        assert_eq!(
            normalize_message(&emitted[0].msg),
            normalize_message(expected_msg)
        );
    }

    #[test]
    fn dx_deep_report_is_human_readable_and_count_based() {
        let report = format_deep_report(
            &SlotTimestamp::parse("140630").unwrap(),
            DxSlotDeepReport {
                foci: 2,
                fields: 3,
                hits: 1,
                emitted: 0,
                crc_candidates: 4,
                crc_attempts: 2,
                crc_skipped_budget: 2,
            },
        );

        assert_eq!(
            report,
            "dx-deep 140630: foci=2 fields=3 hits=1 emitted=0 crc_candidates=4 crc_attempts=2 crc_skipped=2"
        );
    }

    #[test]
    fn dx_slot_snapshot_uses_only_committed_prior_context() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };
        let mut session = DxStreamDecodeSession::new(config);
        let timestamp = SlotTimestamp::parse("140630").unwrap();

        let before = session.slot_snapshot(&timestamp);
        assert!(before.hisgrid.is_none());
        assert!(before.hypotheses.iter().any(|hyp| hyp.msg == "CQ BG5ATV"));
        assert!(!before
            .hypotheses
            .iter()
            .any(|hyp| normalize_message(&hyp.msg) == "CQ BG5ATV PM00"));

        session.context.harvest_listen(
            &timestamp,
            &[StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.0,
                snr: 0.0,
                snr_source: StreamSnrSource::Decoder,
                deep_confidence: None,
                msg: "CQ BG5ATV PM00".to_string(),
                sync: 0.0,
                itone: [0; 79],
            }],
        );

        assert!(before.hisgrid.is_none());
        let after = session.slot_snapshot(&SlotTimestamp::parse("140700").unwrap());
        assert_eq!(after.hisgrid.as_deref(), Some("PM00"));
        assert!(after
            .hypotheses
            .iter()
            .any(|hyp| normalize_message(&hyp.msg) == "CQ BG5ATV PM00"));
    }

    #[test]
    fn dx_committed_qso_progress_feeds_next_focused_config() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            nQSOProgress: 1,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };
        let mut session = DxStreamDecodeSession::new(config.clone());
        let timestamp = SlotTimestamp::parse("140630").unwrap();

        let before = session.slot_snapshot(&timestamp);
        assert_eq!(before.qso_progress, None);
        assert_eq!(
            dx_focus_config(&config, 1000.0, before.qso_progress).nQSOProgress,
            1
        );

        session.context.harvest_focused(
            &timestamp,
            &[StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.2,
                snr: -12.0,
                snr_source: StreamSnrSource::Decoder,
                deep_confidence: None,
                msg: "K1JT BG5ATV R-12".to_string(),
                sync: 2.0,
                itone: [0; 79],
            }],
        );

        let after = session.slot_snapshot(&SlotTimestamp::parse("140700").unwrap());
        assert_eq!(after.qso_progress, Some(3));
        assert_eq!(
            dx_focus_config(&config, 1000.0, after.qso_progress).nQSOProgress,
            3
        );
    }

    #[test]
    fn dx_focus_and_deep_probe_do_not_require_mycall() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };
        let session = DxStreamDecodeSession::new(config.clone());
        let snapshot = session.slot_snapshot(&SlotTimestamp::parse("140630").unwrap());
        assert_eq!(snapshot.foci, vec![1000.0]);
        assert!(snapshot.hypotheses.iter().any(|hyp| hyp.msg == "CQ BG5ATV"));
        assert!(!snapshot
            .hypotheses
            .iter()
            .any(|hyp| hyp.msg.starts_with("K1JT BG5ATV")));

        let samples = vec![0.0f32; NFFT1_LONG];
        let fields = run_deep_probe_fields(&config, 1000.0, Some(0.0), None, &samples);
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn deep_symbol_seeds_prefer_harvested_dt() {
        let focused = dx_focus_config(
            &StreamDecodeConfig {
                profile: DecodeProfile::Dx,
                nfa: 200.0,
                nfb: 3000.0,
                mycall: Some("K1JT".to_string()),
                hiscall: Some("BG5ATV".to_string()),
                ..StreamDecodeConfig::default()
            },
            1000.0,
            None,
        );
        let dd8 = vec![0.0f32; NFFT1_LONG];

        let seeds = deep_symbol_seeds(&focused, &dd8, 1000.0, Some(0.42));

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].freq, 1000.0);
        assert!((seeds[0].xdt0 - 0.42).abs() < 1e-6);
    }

    #[test]
    fn deep_symbol_seeds_fall_back_to_nominal_dt_when_sync8_has_no_candidate() {
        let focused = dx_focus_config(
            &StreamDecodeConfig {
                profile: DecodeProfile::Dx,
                nfa: 200.0,
                nfb: 3000.0,
                mycall: Some("K1JT".to_string()),
                hiscall: Some("BG5ATV".to_string()),
                ..StreamDecodeConfig::default()
            },
            1000.0,
            None,
        );
        let dd8 = vec![0.0f32; NFFT1_LONG];

        let seeds = deep_symbol_seeds(&focused, &dd8, 1000.0, None);

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].freq, 1000.0);
        assert_eq!(seeds[0].xdt0, 0.0);
    }

    #[test]
    fn dx_audio_llr_stack_depth_trend_recovers_synthetic_repetition() {
        let expected_msg = "K1JT BG5ATV -10";
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            dx_deep_experimental_output: true,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&config, 1000.0, None);
        let samples = synthetic_ft8_slot(expected_msg, 1000.0, 0.025);
        let slots = vec![samples.clone(), samples.clone(), samples.clone(), samples];
        let hit = synthetic_stack_recovery_hit(&focused, &slots, expected_msg);

        let hit = hit.expect("repeated weak synthetic audio should recover after stacking");
        assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
        assert_eq!(normalize_message(&hit.msg), normalize_message(expected_msg));
        assert!(
            hit.snr.is_some(),
            "audio-derived deep hit should carry a JTDX-formula SNR estimate"
        );
    }

    #[test]
    fn dx_audio_llr_stack_recovers_target_working_someone_else() {
        let expected_msg = "RA3ABG BG5ATV -10";
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            dx_deep_experimental_output: true,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&config, 1000.0, None);
        let samples = synthetic_ft8_slot(expected_msg, 1000.0, 0.025);
        let slots = vec![samples.clone(), samples.clone(), samples.clone(), samples];
        let hit = synthetic_stack_recovery_hit(&focused, &slots, expected_msg);

        let hit =
            hit.expect("blind T2 stack should recover repeated target-as-sender traffic to others");
        assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
        assert_eq!(normalize_message(&hit.msg), normalize_message(expected_msg));
        assert!(
            hit.snr.is_some(),
            "audio-derived deep hit should carry a JTDX-formula SNR estimate"
        );
    }

    #[test]
    fn dx_deep_snr_estimate_increases_with_synthetic_signal_strength() {
        let msg = "K1JT BG5ATV -10";
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&config, 1000.0, None);
        let weak = synthetic_ft8_slot_with_noise(msg, 1000.0, 0.015, 0.08, 0x5a11_0001);
        let strong = synthetic_ft8_slot_with_noise(msg, 1000.0, 0.030, 0.08, 0x5a11_0001);
        let weak_snr = synthetic_deep_field(&focused, &weak)
            .and_then(|field| estimate_message_snr(&field, msg))
            .expect("weak synthetic slot should produce an estimated SNR");
        let strong_snr = synthetic_deep_field(&focused, &strong)
            .and_then(|field| estimate_message_snr(&field, msg))
            .expect("strong synthetic slot should produce an estimated SNR");

        assert!(
            strong_snr > weak_snr,
            "stronger synthetic target should estimate higher SNR: weak={weak_snr:.2}, strong={strong_snr:.2}"
        );
    }

    #[test]
    fn dx_t1_audio_frontend_ranks_target_hypothesis_first() {
        let expected_msg = "K1JT BG5ATV -10";
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&config, 1000.0, None);
        let samples = synthetic_ft8_slot_with_noise(expected_msg, 1000.0, 0.015, 0.08, 0x7110_3300);
        let field = synthetic_deep_field(&focused, &samples)
            .expect("synthetic audio should produce a dx symbol field");
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));

        let score = dx_deep_score(&field, &hypotheses, hypotheses.len())
            .expect("audio-derived field should produce a T1 score");

        assert_eq!(
            normalize_message(&hypotheses[score.idx].msg),
            normalize_message(expected_msg)
        );
        assert!(
            dx_deep_search(&field, &hypotheses, DeepSearchGate::default()).is_none(),
            "T1 detection may rank the target, but the default calibrated gate remains disabled"
        );
    }

    #[test]
    #[ignore = "manual real-audio T1 scaffold; run with --release --ignored before enabling matched-filter thresholds"]
    fn dx_t1_real_audio_frontend_ranks_decoded_rows_first() {
        let samples = samples_12k_from_wav_for_dx_test("tests/ft8/210703_133430.wav");
        let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
        let mut decoder = JtdxStreamDecodeSession::new(StreamDecodeConfig {
            nfa: 100.0,
            ..StreamDecodeConfig::default()
        });
        let decoded = decoder
            .decode_slot_streaming_at(&timestamp, &samples, |_| Ok(()))
            .expect("short real-audio fixture should decode");
        let mut checked = Vec::new();
        let mut misses = Vec::new();

        for row in decoded {
            let Some((mycall, hiscall, hisgrid)) = v1_context_for_real_row(&row.msg) else {
                continue;
            };
            let hypotheses = build_v1_hypotheses(mycall.as_deref(), &hiscall, hisgrid.as_deref());
            if hypotheses.is_empty() {
                continue;
            }
            let focused_template = StreamDecodeConfig {
                profile: DecodeProfile::Dx,
                nfa: 100.0,
                nfb: 3000.0,
                hiscall: Some(hiscall.clone()),
                mycall: mycall.clone(),
                hisgrid: hisgrid.clone(),
                ..StreamDecodeConfig::default()
            };
            let fields =
                run_deep_probe_fields(&focused_template, row.freq, Some(row.dt), None, &samples);
            let Some(score) = fields
                .iter()
                .filter_map(|field| dx_deep_score(field, &hypotheses, hypotheses.len()))
                .max_by(|a, b| a.stat.total_cmp(&b.stat))
            else {
                misses.push(format!("{}: no field/score", row.msg));
                continue;
            };
            let expected = normalize_message(&row.msg);
            let ranked = normalize_message(&hypotheses[score.idx].msg);
            if ranked == expected {
                checked.push(format!(
                    "{} stat={:.2} margin={:.2}",
                    expected, score.stat, score.margin
                ));
            } else {
                misses.push(format!(
                    "{} ranked {} stat={:.2} margin={:.2}",
                    expected, ranked, score.stat, score.margin
                ));
            }
        }

        eprintln!(
            "DX T1 real-audio rank scaffold: checked={} misses={} rows={checked:?} miss_rows={misses:?}",
            checked.len(),
            misses.len()
        );
        assert!(
            checked.len() >= 8,
            "real-audio T1 scaffold needs enough v1-compatible rows, checked={} misses={}",
            checked.len(),
            misses.len()
        );
        assert!(
            misses.len() <= 2,
            "real-audio T1 scaffold has too many rank mismatches: {misses:?}"
        );
    }

    #[test]
    #[ignore = "manual real-audio SNR scaffold; run with --release --ignored before trusting deep SNR calibration"]
    fn dx_deep_snr_real_audio_calibration_scaffold() {
        let samples = samples_12k_from_wav_for_dx_test("tests/ft8/210703_133430.wav");
        let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
        let focused_template = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 100.0,
            nfb: 3000.0,
            hiscall: Some("DXSNRT".to_string()),
            ..StreamDecodeConfig::default()
        };
        let mut decoder = JtdxStreamDecodeSession::new(StreamDecodeConfig {
            nfa: 100.0,
            ..StreamDecodeConfig::default()
        });
        let decoded = decoder
            .decode_slot_streaming_at(&timestamp, &samples, |_| Ok(()))
            .expect("short real-audio fixture should decode");
        let mut deltas = Vec::new();
        for row in decoded {
            let fields =
                run_deep_probe_fields(&focused_template, row.freq, Some(row.dt), None, &samples);
            let Some(estimated) = fields
                .iter()
                .filter_map(|field| estimate_message_snr(field, &row.msg))
                .min_by(|a, b| {
                    (a - row.snr as f32)
                        .abs()
                        .total_cmp(&(b - row.snr as f32).abs())
                })
            else {
                continue;
            };
            deltas.push((
                row.msg,
                row.snr as f32,
                estimated,
                estimated - row.snr as f32,
            ));
        }
        assert!(
            deltas.len() >= 8,
            "real-audio SNR scaffold needs enough paired rows, got {}: {:?}",
            deltas.len(),
            deltas
        );
        let mean_abs_delta = deltas
            .iter()
            .map(|(_, _, _, delta)| delta.abs())
            .sum::<f32>()
            / deltas.len() as f32;
        let max_abs_delta = deltas
            .iter()
            .map(|(_, _, _, delta)| delta.abs())
            .fold(0.0f32, f32::max);
        eprintln!(
            "DX deep SNR real-audio scaffold: pairs={} mean_abs_delta={mean_abs_delta:.2} max_abs_delta={max_abs_delta:.2} rows={deltas:?}",
            deltas.len()
        );
        assert!(
            mean_abs_delta <= 12.0,
            "deep SNR estimate diverged too far from decoder SNR on real audio"
        );
        assert!(
            max_abs_delta <= 25.0,
            "deep SNR estimate has an extreme real-audio outlier"
        );
    }

    #[test]
    #[ignore = "manual G1 gate; run with --release --ignored after threshold calibration"]
    fn dx_g1_audio_gate_stack_recovers_kernel_misses() {
        run_g1_audio_gate("K1JT BG5ATV -10", 0x5eed_0000);
    }

    #[test]
    #[ignore = "manual G1-B gate; run with --release --ignored after threshold calibration"]
    fn dx_g1_audio_gate_recovers_target_working_someone_else_kernel_misses() {
        run_g1_audio_gate("RA3ABG BG5ATV -10", 0x5eed_1000);
    }

    fn run_g1_audio_gate(expected_msg: &str, seed_base: u64) {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 900.0,
            nfb: 1100.0,
            nfqso: 1000.0,
            dx_deep_experimental_output: true,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            hisgrid: Some("PM00".to_string()),
            ..StreamDecodeConfig::default()
        };
        let focused = dx_focus_config(&config, 1000.0, None);
        let amplitudes = [0.001, 0.0015, 0.002, 0.003, 0.004, 0.006, 0.008];
        let noises = [0.08, 0.12, 0.18, 0.27, 0.40, 0.60];
        let mut diagnostics = Vec::new();

        for noise in noises {
            for amplitude in amplitudes {
                let slots: Vec<Vec<f32>> = (0..4)
                    .map(|idx| {
                        synthetic_ft8_slot_with_noise(
                            expected_msg,
                            1000.0,
                            amplitude,
                            noise,
                            seed_base + idx as u64,
                        )
                    })
                    .collect();
                let stack_hit = synthetic_stack_recovery_hit(&focused, &slots, expected_msg);
                let stack_ok = stack_hit.as_ref().is_some_and(|hit| {
                    normalize_message(&hit.msg) == normalize_message(expected_msg)
                });
                if !stack_ok {
                    diagnostics.push(format!("amp={amplitude:.4} noise={noise:.4}: stack miss"));
                    continue;
                }

                let kernel_hit = focused_kernel_decodes_target(&focused, &slots[0], expected_msg);
                diagnostics.push(format!(
                    "amp={amplitude:.4} noise={noise:.4}: stack hit kernel={kernel_hit}"
                ));
                if !kernel_hit {
                    let hit = stack_hit.unwrap();
                    assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
                    assert!(
                        dx_profile_emits_target_for_slots(&config, &slots, expected_msg),
                        "DX profile did not emit the target for amp={amplitude:.4} noise={noise:.4}"
                    );
                    eprintln!(
                        "G1 gate: msg={} amp={amplitude:.4} noise={noise:.4}",
                        normalize_message(expected_msg)
                    );
                    return;
                }
            }
        }

        panic!(
            "no G1 amplitude found where T2 stack recovers and focused kernel misses: {diagnostics:?}"
        );
    }

    #[test]
    fn dx_false_alarm_smoke_rejects_wrong_call_and_noise() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let physical_gate = PhysicalAdmissionGate::default();

        for timestamp in ["140630", "140700", "140730", "140800"] {
            let field = llr_field_from_message("K1JT RA3ABG -10", 1000.0, 0.0, 0.9);
            let hit = store.observe_deep_field(
                &SlotTimestamp::parse(timestamp).unwrap(),
                1000.0,
                &field,
                &hypotheses,
                DeepSearchGate::default(),
                physical_gate,
            );
            assert!(
                hit.is_none(),
                "wrong-call LLR fabricated target at {timestamp}"
            );
        }

        let mut noise_store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        );
        for (idx, timestamp) in ["140630", "140700", "140730", "140800"]
            .into_iter()
            .enumerate()
        {
            let field = noise_llr_field(1000.0, 0.0, 0xfa15_0000 + idx as u64);
            let hit = noise_store.observe_deep_field(
                &SlotTimestamp::parse(timestamp).unwrap(),
                1000.0,
                &field,
                &hypotheses,
                DeepSearchGate::default(),
                physical_gate,
            );
            assert!(hit.is_none(), "noise LLR fabricated target at {timestamp}");
        }
    }

    #[test]
    fn dx_false_alarm_smoke_rejects_changing_target_messages_on_one_stack() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let physical_gate = PhysicalAdmissionGate::default();
        let messages = [
            "RA3ABG BG5ATV -10",
            "F1MLZ BG5ATV R-12",
            "UA3QNA BG5ATV RR73",
            "VE7ON BG5ATV 73",
        ];

        for (timestamp, msg) in ["140630", "140700", "140730", "140800"]
            .into_iter()
            .zip(messages)
        {
            let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.8);
            add_deterministic_llr_noise(&mut field.llr, 0.35, 0xfa15_e000);
            let hit = store.observe_deep_field(
                &SlotTimestamp::parse(timestamp).unwrap(),
                1000.0,
                &field,
                &hypotheses,
                DeepSearchGate::default(),
                physical_gate,
            );
            assert!(
                hit.is_none(),
                "changing target messages should not fabricate a stack decode at {timestamp}: {hit:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual T1 calibration scaffold; run with --release --ignored before enabling matched-filter thresholds"]
    fn dx_t1_matched_filter_calibration_scaffold() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let target_messages = [
            "K1JT BG5ATV -10",
            "K1JT BG5ATV R-12",
            "K1JT BG5ATV RR73",
            "CQ BG5ATV PM00",
        ];
        let wrong_messages = false_alarm_wrong_messages();
        let near_call_messages = false_alarm_near_call_messages();
        let hash_collision_messages = false_alarm_hash_collision_messages();

        let mut min_target_stat = f32::INFINITY;
        let mut min_target_margin = f32::INFINITY;
        for (idx, msg) in target_messages.iter().enumerate() {
            let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.40);
            add_deterministic_llr_noise(&mut field.llr, 0.30, 0x7110_0000 + idx as u64);
            let score = dx_deep_score(&field, &hypotheses, hypotheses.len())
                .expect("target field should produce a matched-filter score");
            assert_eq!(
                normalize_message(&hypotheses[score.idx].msg),
                normalize_message(msg)
            );
            min_target_stat = min_target_stat.min(score.stat);
            min_target_margin = min_target_margin.min(score.margin);
        }

        let mut max_false_stat = f32::NEG_INFINITY;
        let mut max_false_margin = f32::NEG_INFINITY;
        let mut false_cases = 0usize;
        for (idx, msg) in wrong_messages
            .iter()
            .chain(near_call_messages.iter())
            .chain(hash_collision_messages.iter())
            .enumerate()
        {
            let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.40);
            add_deterministic_llr_noise(&mut field.llr, 0.30, 0x7110_1000 + idx as u64);
            let score = dx_deep_score(&field, &hypotheses, hypotheses.len())
                .expect("false field should still produce a score for calibration");
            max_false_stat = max_false_stat.max(score.stat);
            max_false_margin = max_false_margin.max(score.margin);
            false_cases += 1;
        }
        for idx in 0..256 {
            let field = noise_llr_field(1000.0, 0.0, 0x7110_2000 + idx as u64);
            let score = dx_deep_score(&field, &hypotheses, hypotheses.len())
                .expect("noise field should still produce a score for calibration");
            max_false_stat = max_false_stat.max(score.stat);
            max_false_margin = max_false_margin.max(score.margin);
            false_cases += 1;
        }

        eprintln!(
            "DX T1 scaffold: target_min_stat={min_target_stat:.3} target_min_margin={min_target_margin:.3} false_max_stat={max_false_stat:.3} false_max_margin={max_false_margin:.3} false_cases={false_cases}"
        );
        assert!(
            min_target_stat > max_false_stat,
            "target/false stat ranges overlap"
        );
        assert!(
            min_target_margin > max_false_margin,
            "target/false margin ranges overlap"
        );
    }

    #[test]
    #[ignore = "manual T1 real-audio false-alarm ceiling scaffold; run with --release --ignored before enabling matched-filter thresholds"]
    fn dx_t1_real_audio_false_alarm_ceiling_scaffold() {
        // Measures the raw matched-filter (stat, margin) that an ABSENT target's
        // v1 hypotheses reach on a real on-band recording. This is the empirical
        // false-alarm ceiling any finite min_stat/min_margin must clear before the
        // matched-filter-only emit paths (TwoSlotMatched / StackedLlrMatched) can
        // be enabled. The runtime gate stays INFINITY; this scaffold only reports
        // the number, it never lowers a threshold. Pairs with the present-target
        // rank scaffold (dx_t1_real_audio_frontend_ranks_decoded_rows_first) to
        // give the real-audio separation gap, and is the per-recording probe the
        // deferred field corpus will accumulate over.
        let samples = samples_12k_from_wav_for_dx_test("tests/ft8/230208_140300.wav");
        let sps = 15 * 12000;
        let nseg = samples.len().div_ceil(sps).min(6);
        // Calls that are NOT present in this recording: every score is a false
        // alarm. Mirrors the integration absent-target matrix gate.
        let targets = ["ZZ1ZZZ", "QQ9QQQ", "K0ZZZ", "N0ABC"];
        let focuses = [500.0, 1000.0, 1500.0, 2200.0];

        let mut max_false_stat = f32::NEG_INFINITY;
        // Track the max *finite* margin separately: a field whose hypothesis set
        // has no scored runner-up yields margin = stat - (-inf) = +inf, so a
        // min_margin gate would pass trivially. Counting those degenerate fields
        // documents that `stat` is the reliable axis and `margin` cannot be a sole
        // gate.
        let mut max_finite_false_margin = f32::NEG_INFINITY;
        let mut degenerate_margin_fields = 0usize;
        let mut worst = String::new();
        let mut scored = 0usize;
        let mut hyp_counts = Vec::new();
        for target in targets {
            // Include a mycall so the directed {mycall hiscall report} hypotheses
            // are enumerated too — the worst case for the false-alarm ceiling.
            let hypotheses = build_v1_hypotheses(Some("K1JT"), target, None);
            hyp_counts.push(format!("{target}:{}", hypotheses.len()));
            for focus in focuses {
                let config = StreamDecodeConfig {
                    profile: DecodeProfile::Dx,
                    nfqso: focus,
                    nfa: (focus - 100.0_f64).max(200.0),
                    nfb: (focus + 100.0_f64).min(3000.0),
                    hiscall: Some(target.to_string()),
                    mycall: Some("K1JT".to_string()),
                    ..StreamDecodeConfig::default()
                };
                for seg in 0..nseg {
                    let begin = seg * sps;
                    let end = (begin + sps).min(samples.len());
                    let slot = &samples[begin..end];
                    let fields = run_deep_probe_fields(&config, focus, None, None, slot);
                    for field in &fields {
                        let Some(score) = dx_deep_score(field, &hypotheses, hypotheses.len()) else {
                            continue;
                        };
                        scored += 1;
                        if score.stat > max_false_stat {
                            max_false_stat = score.stat;
                            worst = format!(
                                "target={target} focus={focus:.0} seg={seg} stat={:.3} margin={:.3} msg={}",
                                score.stat, score.margin, hypotheses[score.idx].msg
                            );
                        }
                        if score.margin.is_finite() {
                            max_finite_false_margin = max_finite_false_margin.max(score.margin);
                        } else {
                            degenerate_margin_fields += 1;
                        }
                    }
                }
            }
        }

        eprintln!(
            "DX T1 real-audio false-alarm ceiling: scored={scored} false_max_stat={max_false_stat:.3} \
             false_max_finite_margin={max_finite_false_margin:.3} degenerate_margin_fields={degenerate_margin_fields} \
             hyp_counts=[{}] worst=[{worst}]",
            hyp_counts.join(" ")
        );
        assert!(
            scored > 0,
            "scaffold should score at least one real-audio field"
        );
        // `stat` is the binding axis for any future min_stat threshold, so it must
        // be finite. `margin` is intentionally allowed to be +inf on degenerate
        // single-runner fields (reported above), which is exactly why min_margin
        // cannot stand alone as a gate.
        assert!(
            max_false_stat.is_finite(),
            "real-audio false-alarm stat ceiling must be finite"
        );
        // Regression guard, NOT a calibrated Pfa threshold. Observed ceiling on
        // tests/ft8/230208_140300.wav is stat≈144.46 (bare `CQ HISCALL`). The
        // bound is generous so it catches a gross inflation of the matched-filter
        // ceiling without baking in a per-fixture number as if it were validated.
        // A real min_stat threshold awaits the field corpus characterizing this
        // ceiling's distribution across many recordings (absolute stat is not
        // normalized across SNR/recording).
        assert!(
            max_false_stat < 180.0,
            "real-audio false-alarm stat ceiling inflated past the recorded ~144.46 guard: {max_false_stat:.3}"
        );
    }

    #[test]
    #[ignore = "manual release G2 scaffold; extend with 24h noise/real recordings before declaring safety"]
    fn dx_false_alarm_corpus_manual_gate() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let physical_gate = PhysicalAdmissionGate::default();
        let timestamps = false_alarm_timestamps();
        let mut total = 0usize;
        let mut fabricated = 0usize;
        let mut wrong_slots = 0usize;
        let mut near_call_slots = 0usize;
        let mut hash_collision_slots = 0usize;
        let mut noise_slots = 0usize;
        let mut exposure = DxExposure::default();

        let wrong_messages = false_alarm_wrong_messages();
        for (case_idx, msg) in wrong_messages.iter().enumerate() {
            let mut store = false_alarm_store();
            for (slot_idx, timestamp) in timestamps.iter().enumerate() {
                total += 1;
                wrong_slots += 1;
                record_llr_g2_exposure(&mut exposure, hypotheses.len());
                let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.8);
                add_deterministic_llr_noise(
                    &mut field.llr,
                    0.35,
                    0xfa15_c000 + (case_idx * 16 + slot_idx) as u64,
                );
                let hit = store.observe_deep_field(
                    timestamp,
                    1000.0,
                    &field,
                    &hypotheses,
                    DeepSearchGate::default(),
                    physical_gate,
                );
                if hit.is_some() {
                    fabricated += 1;
                }
            }
            record_llr_g2_stack_exposure(&mut exposure, &store);
        }

        let near_call_messages = false_alarm_near_call_messages();
        for (case_idx, msg) in near_call_messages.iter().enumerate() {
            let mut store = false_alarm_store();
            for (slot_idx, timestamp) in timestamps.iter().enumerate() {
                total += 1;
                near_call_slots += 1;
                record_llr_g2_exposure(&mut exposure, hypotheses.len());
                let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.8);
                add_deterministic_llr_noise(
                    &mut field.llr,
                    0.40,
                    0xfa15_d000 + (case_idx * 32 + slot_idx) as u64,
                );
                let hit = store.observe_deep_field(
                    timestamp,
                    1000.0,
                    &field,
                    &hypotheses,
                    DeepSearchGate::default(),
                    physical_gate,
                );
                if hit.is_some() {
                    fabricated += 1;
                }
            }
            record_llr_g2_stack_exposure(&mut exposure, &store);
        }

        let hash_collision_messages = false_alarm_hash_collision_messages();
        for (case_idx, msg) in hash_collision_messages.iter().enumerate() {
            let mut store = false_alarm_store();
            for (slot_idx, timestamp) in timestamps.iter().enumerate() {
                total += 1;
                hash_collision_slots += 1;
                record_llr_g2_exposure(&mut exposure, hypotheses.len());
                let mut field = llr_field_from_message(msg, 1000.0, 0.0, 0.8);
                add_deterministic_llr_noise(
                    &mut field.llr,
                    0.40,
                    0xfa15_e000 + (case_idx * 32 + slot_idx) as u64,
                );
                let hit = store.observe_deep_field(
                    timestamp,
                    1000.0,
                    &field,
                    &hypotheses,
                    DeepSearchGate::default(),
                    physical_gate,
                );
                if hit.is_some() {
                    fabricated += 1;
                }
            }
            record_llr_g2_stack_exposure(&mut exposure, &store);
        }

        // 720 cases * 8 synthetic timestamps = 5760 15 s slots, matching a
        // 24 h slot count at the LLR-scaffold level. This still does not replace
        // the required real audio/noise corpus in PLAN.md.
        for case_idx in 0..720 {
            let mut store = false_alarm_store();
            for (slot_idx, timestamp) in timestamps.iter().enumerate() {
                total += 1;
                noise_slots += 1;
                record_llr_g2_exposure(&mut exposure, hypotheses.len());
                let field =
                    noise_llr_field(1000.0, 0.0, 0xfa15_5000 + (case_idx * 16 + slot_idx) as u64);
                let hit = store.observe_deep_field(
                    timestamp,
                    1000.0,
                    &field,
                    &hypotheses,
                    DeepSearchGate::default(),
                    physical_gate,
                );
                if hit.is_some() {
                    fabricated += 1;
                }
            }
            record_llr_g2_stack_exposure(&mut exposure, &store);
        }

        let pfa95_slots = rule_of_three_upper(total);
        let pfa95_focus = rule_of_three_upper(exposure.focus_trials);
        let pfa95_hypothesis = rule_of_three_upper(exposure.hypothesis_trials);
        let pfa95_stack_osd = rule_of_three_upper(exposure.stack_osd_attempts);
        eprintln!(
            "DX G2 scaffold: emitted_fabrications={fabricated}/{total}, pfa95_slots<={pfa95_slots:.6}, pfa95_focus<={pfa95_focus:.6}, pfa95_hypothesis<={pfa95_hypothesis:.6}, pfa95_stack_osd<={pfa95_stack_osd:.6}, slots={} focus_trials={} field_trials={} hypothesis_trials={} stack_osd_candidates={} stack_osd_attempts={} stack_osd_skipped_budget={} deep_rows_emitted={} wrong_slots={wrong_slots}, near_call_slots={near_call_slots}, hash_collision_slots={hash_collision_slots}, noise_slots={noise_slots}",
            exposure.slots,
            exposure.focus_trials,
            exposure.field_trials,
            exposure.hypothesis_trials,
            exposure.stack_osd_candidates,
            exposure.stack_osd_attempts,
            exposure.stack_osd_skipped_budget,
            exposure.deep_rows_emitted
        );
        assert_eq!(
            exposure.slots, total,
            "G2 scaffold exposure slots must match total trial slots"
        );
        assert!(
            wrong_slots >= 1000,
            "G2 scaffold should exercise at least 1000 wrong-call slots"
        );
        assert!(
            hash_collision_slots >= 50,
            "G2 scaffold should exercise at least 50 hash/near-callsign slots"
        );
        assert!(
            noise_slots >= 5760,
            "G2 scaffold should exercise at least 24h-equivalent pure-noise slots"
        );
        assert_eq!(fabricated, 0);
    }

    fn rule_of_three_upper(exposure: usize) -> f64 {
        3.0 / exposure.max(1) as f64
    }

    fn record_llr_g2_exposure(exposure: &mut DxExposure, hypotheses_len: usize) {
        exposure.slots += 1;
        exposure.focus_trials += 1;
        exposure.field_trials += 1;
        exposure.hypothesis_trials += hypotheses_len;
    }

    fn record_llr_g2_stack_exposure(exposure: &mut DxExposure, store: &TargetContextStore) {
        let diagnostics = store.deep_diagnostics();
        exposure.stack_osd_candidates += diagnostics.crc_candidates;
        exposure.stack_osd_attempts += diagnostics.crc_attempts;
        exposure.stack_osd_skipped_budget += diagnostics.crc_skipped_budget;
    }

    fn synthetic_stack_recovery_hit(
        focused: &StreamDecodeConfig,
        slots: &[Vec<f32>],
        expected_msg: &str,
    ) -> Option<DeepHit> {
        assert!(!slots.is_empty());
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let physical_gate = PhysicalAdmissionGate {
            min_nsync: 1,
            min_syncavemax: 0.0,
            ..PhysicalAdmissionGate::default()
        };

        let first = synthetic_deep_field(focused, &slots[0]).and_then(|field| {
            store.observe_deep_field(
                &SlotTimestamp::parse("140630").unwrap(),
                1000.0,
                &field,
                &hypotheses,
                DeepSearchGate::default(),
                physical_gate,
            )
        });
        assert!(
            first.is_none(),
            "single weak synthetic slot should not emit"
        );

        for (timestamp, samples) in ["140700", "140730", "140800"]
            .into_iter()
            .zip(slots.iter().skip(1))
        {
            let field = synthetic_deep_field(focused, samples)
                .expect("synthetic slot should produce a dx symbol field");
            let hit = store.observe_deep_field(
                &SlotTimestamp::parse(timestamp).unwrap(),
                1000.0,
                &field,
                &hypotheses,
                DeepSearchGate::default(),
                physical_gate,
            );
            if hit
                .as_ref()
                .is_some_and(|hit| normalize_message(&hit.msg) == normalize_message(expected_msg))
            {
                return hit;
            }
        }
        None
    }

    fn focused_kernel_decodes_target(
        focused: &StreamDecodeConfig,
        samples: &[f32],
        expected_msg: &str,
    ) -> bool {
        JtdxStreamDecodeSession::new(focused.clone())
            .decode_slot_streaming_at(
                &SlotTimestamp::parse("140630").unwrap(),
                samples,
                |_| Ok(()),
            )
            .map(|rows| {
                rows.iter()
                    .any(|row| normalize_message(&row.msg) == normalize_message(expected_msg))
            })
            .unwrap_or(false)
    }

    fn dx_profile_emits_target_for_slots(
        config: &StreamDecodeConfig,
        slots: &[Vec<f32>],
        expected_msg: &str,
    ) -> bool {
        let mut decoder = DxStreamDecodeSession::new(config.clone());
        for (timestamp, samples) in ["140630", "140700", "140730", "140800"]
            .into_iter()
            .zip(slots)
        {
            let rows = decoder.decode_slot_at(&SlotTimestamp::parse(timestamp).unwrap(), samples);
            if rows
                .iter()
                .any(|row| normalize_message(&row.msg) == normalize_message(expected_msg))
            {
                return true;
            }
        }
        false
    }

    fn synthetic_deep_field(
        focused: &StreamDecodeConfig,
        samples: &[f32],
    ) -> Option<crate::decode::lib_jtdx::ft8b::DxSymbolField> {
        run_deep_probe_fields(focused, 1000.0, Some(0.0), None, samples)
            .into_iter()
            .next()
    }

    fn v1_context_for_real_row(msg: &str) -> Option<(Option<String>, String, Option<String>)> {
        let parts: Vec<&str> = msg.split_whitespace().collect();
        match parts.as_slice() {
            ["CQ", hiscall, grid] if is_grid4(grid) => {
                Some((None, (*hiscall).to_string(), Some((*grid).to_string())))
            }
            ["CQ", hiscall] => Some((None, (*hiscall).to_string(), None)),
            [mycall, hiscall, report] if is_v1_report_or_73(report) => {
                Some((Some((*mycall).to_string()), (*hiscall).to_string(), None))
            }
            _ => None,
        }
    }

    fn is_v1_report_or_73(value: &str) -> bool {
        value == "73" || value == "RR73" || parse_report(value).is_some()
    }

    fn parse_report(value: &str) -> Option<i32> {
        let report = value.strip_prefix('R').unwrap_or(value);
        let parsed = report.parse::<i32>().ok()?;
        (-24..=0).contains(&parsed).then_some(parsed)
    }

    fn is_grid4(value: &str) -> bool {
        let bytes = value.as_bytes();
        bytes.len() == 4
            && bytes[0].is_ascii_alphabetic()
            && bytes[1].is_ascii_alphabetic()
            && bytes[2].is_ascii_digit()
            && bytes[3].is_ascii_digit()
    }

    fn samples_12k_from_wav_for_dx_test(path: &str) -> Vec<f32> {
        let audio = read_wav_mono_f32(path).expect("test wav should be readable");
        if audio.sample_rate == 12000 {
            audio.samples
        } else {
            resample_linear(&audio.samples, audio.sample_rate, 12000)
        }
    }

    fn synthetic_ft8_slot(msg: &str, freq: f64, amplitude: f64) -> Vec<f32> {
        synthetic_ft8_slot_with_noise(msg, freq, amplitude, 0.0, 0)
    }

    fn synthetic_ft8_slot_with_noise(
        msg: &str,
        freq: f64,
        amplitude: f64,
        noise_amplitude: f64,
        seed: u64,
    ) -> Vec<f32> {
        let (_, _, itone) = genft8(msg).expect("test message must pack");
        let (wave_re, _) = gen_ft8wave(&itone, freq);
        let mut samples = vec![0.0f32; NFFT1_LONG];
        add_deterministic_noise(&mut samples, noise_amplitude, seed);
        let start = 6_000usize;
        debug_assert!(start + NFRAME <= samples.len());
        for (idx, value) in wave_re.iter().copied().enumerate() {
            samples[start + idx] += (amplitude * value) as f32;
        }
        samples
    }

    fn add_deterministic_noise(samples: &mut [f32], amplitude: f64, mut state: u64) {
        if amplitude == 0.0 {
            return;
        }
        for sample in samples {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let upper = (state >> 32) as u32;
            let unit = upper as f64 / u32::MAX as f64;
            *sample += ((2.0 * unit - 1.0) * amplitude) as f32;
        }
    }

    fn llr_field_from_message(
        msg: &str,
        freq: f64,
        dt: f64,
        magnitude: f32,
    ) -> crate::decode::lib_jtdx::ft8b::DxSymbolField {
        let (_, bits77, _) = genft8(msg).expect("test message must pack");
        let codeword = encode174_91(&bits77);
        let mut llr = [0.0f32; 174];
        for (dst, bit) in llr.iter_mut().zip(codeword) {
            *dst = if bit == 1 { magnitude } else { -magnitude };
        }
        crate::decode::lib_jtdx::ft8b::DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr,
            ibest: 0,
            refined_freq: freq,
            refined_dt: dt,
            syncavemax: 1.0,
            nsync: 8,
        }
    }

    fn noise_llr_field(
        freq: f64,
        dt: f64,
        mut state: u64,
    ) -> crate::decode::lib_jtdx::ft8b::DxSymbolField {
        let mut llr = [0.0f32; 174];
        for dst in &mut llr {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let upper = (state >> 32) as u32;
            let unit = upper as f32 / u32::MAX as f32;
            *dst = 2.0 * unit - 1.0;
        }
        crate::decode::lib_jtdx::ft8b::DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr,
            ibest: 0,
            refined_freq: freq,
            refined_dt: dt,
            syncavemax: 1.0,
            nsync: 8,
        }
    }

    fn false_alarm_store() -> TargetContextStore {
        TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            900.0,
            1100.0,
        )
    }

    fn false_alarm_timestamps() -> Vec<SlotTimestamp> {
        let start = SlotTimestamp::parse("140630").unwrap();
        (0..8).map(|idx| start.add_seconds(30 * idx)).collect()
    }

    fn false_alarm_wrong_messages() -> Vec<String> {
        const OTHER_CALLS: [&str; 16] = [
            "RA3ABG", "W1ABC", "DL5PH", "EA8CBP", "F1MLZ", "UA3QNA", "S56KFG", "VE7ON", "IV3KEI",
            "UT7UJ", "DL1LSL", "OH5NBJ", "IZ7MFY", "IC8SQS", "IW1PUR", "VJ6X",
        ];
        const GRIDS: [&str; 8] = [
            "KO95", "JN07", "PM00", "JN65", "IL18", "KP41", "FN31", "JN44",
        ];
        const REPORTS: [&str; 8] = ["-24", "-18", "-10", "-03", "+00", "+05", "R-18", "R-03"];
        let mut messages = Vec::new();
        for (idx, call) in OTHER_CALLS.iter().enumerate() {
            messages.push(format!("K1JT {call} {}", REPORTS[idx % REPORTS.len()]));
            messages.push(format!("CQ {call} {}", GRIDS[idx % GRIDS.len()]));
            messages.push(format!(
                "{} {call} {}",
                OTHER_CALLS[(idx + 3) % OTHER_CALLS.len()],
                REPORTS[(idx + 5) % REPORTS.len()]
            ));
            messages.push(format!(
                "{} {call} {}",
                OTHER_CALLS[(idx + 5) % OTHER_CALLS.len()],
                GRIDS[(idx + 2) % GRIDS.len()]
            ));
            messages.push(format!(
                "{call} {} {}",
                OTHER_CALLS[(idx + 7) % OTHER_CALLS.len()],
                REPORTS[(idx + 1) % REPORTS.len()]
            ));
            messages.push(format!(
                "{call} {} {}",
                OTHER_CALLS[(idx + 11) % OTHER_CALLS.len()],
                GRIDS[(idx + 4) % GRIDS.len()]
            ));
            messages.push(format!("CQ {}", OTHER_CALLS[(idx + 9) % OTHER_CALLS.len()]));
            messages.push(format!(
                "{} {call} RR73",
                OTHER_CALLS[(idx + 13) % OTHER_CALLS.len()]
            ));
        }
        messages
    }

    fn false_alarm_near_call_messages() -> Vec<String> {
        const NEAR_CALLS: [&str; 12] = [
            "BG5ATU", "BG5ATW", "BG5AAV", "BG5BTV", "BG4ATV", "BH5ATV", "BG5ATM", "BG5ATX",
            "BG5AVV", "BG6ATV", "BG5CTV", "BG5ATP",
        ];
        const REPORTS: [&str; 4] = ["-20", "-10", "R-12", "RR73"];
        let mut messages = Vec::new();
        for (idx, call) in NEAR_CALLS.iter().enumerate() {
            messages.push(format!("K1JT {call} {}", REPORTS[idx % REPORTS.len()]));
            messages.push(format!("CQ {call} PM00"));
            messages.push(format!(
                "W1ABC {call} {}",
                REPORTS[(idx + 1) % REPORTS.len()]
            ));
        }
        messages
    }

    fn false_alarm_hash_collision_messages() -> Vec<String> {
        const HASH_LIKE_CALLS: [&str; 8] = [
            "BG5ATU", "BG5ATW", "BG4ATV", "BH5ATV", "BG5ATM", "BG5ATX", "BG6ATV", "BG5ATP",
        ];
        const REPORTS: [&str; 4] = ["-20", "-10", "R-12", "RR73"];
        let mut messages = Vec::new();
        for (idx, call) in HASH_LIKE_CALLS.iter().enumerate() {
            // Braced calls exercise the same normalization hazard as resolved
            // FT8 hash calls: similar-looking calls must not be accepted as the
            // target unless the resolved call is exactly `hiscall`.
            messages.push(format!("K1JT <{call}> {}", REPORTS[idx % REPORTS.len()]));
            messages.push(format!(
                "<{call}> K1JT {}",
                REPORTS[(idx + 1) % REPORTS.len()]
            ));
            messages.push(format!(
                "W1ABC <{call}> {}",
                REPORTS[(idx + 2) % REPORTS.len()]
            ));
        }
        messages
    }

    fn add_deterministic_llr_noise(llr: &mut [f32; 174], amplitude: f32, mut state: u64) {
        for value in llr {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let upper = (state >> 32) as u32;
            let unit = upper as f32 / u32::MAX as f32;
            *value += (2.0 * unit - 1.0) * amplitude;
        }
    }
}
