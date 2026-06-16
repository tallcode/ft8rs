//! DX chase orchestration profile.
//!
//! This module intentionally stays outside `lib_wsjtx` and `lib_jtdx`. It builds
//! worker sessions with existing public configuration fields, filters output to
//! the operator-selected target, and later owns the DX-specific cross-slot
//! context.

use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage,
};
use crate::stream::time::SlotTimestamp;

use std::time::Duration;

mod context;
mod filter;

use context::TargetContextStore;
use filter::{normalize_message, DxTarget};

pub struct DxStreamDecodeSession {
    base_config: StreamDecodeConfig,
    target: DxTarget,
    context: TargetContextStore,
    hash_seed_calls: Vec<String>,
    listen: JtdxStreamDecodeSession,
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
        );
        Self {
            base_config: config,
            target,
            context,
            hash_seed_calls,
            listen,
        }
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
        let foci =
            if self.base_config.mycall.is_some() && self.context.should_run_focused(timestamp) {
                self.context.selected_foci()
            } else {
                Vec::new()
            };
        let mut emitted = Vec::new();
        let listen_results =
            self.listen
                .decode_slot_streaming_at(timestamp, samples, |decode| {
                    emit_target_row(&self.context, &mut emitted, decode, &mut on_decode)?;
                    Ok(())
                })?;
        self.context.harvest_listen(timestamp, &listen_results);

        let mut focused_target_rows = Vec::new();
        for focus in foci {
            if dx_focused_budget_expired(&self.base_config, started_at.elapsed()) {
                break;
            }
            let mut focused =
                JtdxStreamDecodeSession::new(dx_focus_config(&self.base_config, focus));
            focused.import_hash_calls(&self.hash_seed_calls);
            let focused_results =
                focused.decode_slot_streaming_at(timestamp, samples, |decode| {
                    emit_target_row(&self.context, &mut emitted, decode, &mut on_decode)?;
                    Ok(())
                })?;
            focused_target_rows.extend(
                focused_results
                    .into_iter()
                    .filter(|row| self.target.matches_message(&row.msg)),
            );

            if let Some(hisgrid) = self.context.hisgrid() {
                if dx_focused_budget_expired(&self.base_config, started_at.elapsed()) {
                    break;
                }
                let mut wsjtx =
                    StreamDecodeSession::new(dx_a8_config(&self.base_config, focus, hisgrid));
                wsjtx.import_hash_calls(&self.hash_seed_calls);
                let wsjtx_results =
                    wsjtx.decode_slot_streaming_at(timestamp, samples, |decode| {
                        emit_target_row(&self.context, &mut emitted, decode, &mut on_decode)?;
                        Ok(())
                    })?;
                focused_target_rows.extend(
                    wsjtx_results
                        .into_iter()
                        .filter(|row| self.target.matches_message(&row.msg)),
                );
            }
        }
        self.context
            .harvest_focused(timestamp, &focused_target_rows);

        Ok(emitted)
    }
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

fn dx_focus_config(config: &StreamDecodeConfig, focus: f64) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_jtdx();
    focused.profile = DecodeProfile::Jtdx;
    focused.swl = true;
    focused.nagain = true;
    focused.filter = false;
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    focused
}

fn dx_a8_config(config: &StreamDecodeConfig, focus: f64, hisgrid: &str) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_wsjt_x();
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
) -> Result<(), String>
where
    F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
{
    if !context.should_emit_target_row(decode)
        || emitted.iter().any(|row| is_same_signal(row, decode))
    {
        return Ok(());
    }
    emitted.push(decode.clone());
    on_decode(decode)
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
            mycall: Some("F1MLZ".to_string()),
            hiscall: Some("UA3QNA".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_focus_config(&config, 1152.0);

        assert_eq!(focused.profile, DecodeProfile::Jtdx);
        assert!(focused.swl);
        assert!(focused.nagain);
        assert_eq!(focused.nfqso, 1152.0);
        assert_eq!(focused.nfa, 1127.0);
        assert_eq!(focused.nfb, 1177.0);
        assert_eq!(focused.mycall.as_deref(), Some("F1MLZ"));
        assert_eq!(focused.hiscall.as_deref(), Some("UA3QNA"));
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
}
