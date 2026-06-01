//! JTDX-oriented FT8 decoder path.
//!
//! This module is intentionally separate from the WSJT-X-aligned decoder. The
//! files below mirror the JTDX FT8 dependency closure and should be filled in
//! from the corresponding JTDX source files.

pub mod agccft8;
pub mod callsign_q;
pub mod chkfalse8;
pub mod chkflscall;
pub mod chkgrid;
pub mod chklong8;
pub mod chkspecial8;
pub mod filtersfree;
pub mod four2a;
pub mod ft8_decode;
pub mod ft8_downsample;
pub mod ft8_mod1;
pub mod ft8_params;
pub mod ft8apset;
pub mod ft8b;
pub mod ft8mf1;
pub mod ft8mfcq;
pub mod ft8s;
pub mod ft8sd;
pub mod ft8sd1;
pub mod ft8v2;
pub mod gen_ft8wave;
pub mod indexx;
pub mod msgparser;
pub mod partintft8;
pub mod searchcalls;
pub mod sync8;
pub mod sync8d;
pub mod syncdist;
pub mod tone8;
pub mod tonesd;
pub mod twkfreq1;

use crate::stream::session::{StreamDecodeConfig, StreamDecodedMessage};
use crate::stream::time::SlotTimestamp;

use self::agccft8::agccft8;
use self::ft8apset::{ft8apset, Ft8ApSet};
use self::ft8b::DecodeSource;
use self::ft8v2::packjt77::HashCallBook;
use self::tone8::{tone8, Tone8Tables};

/// JTDX decoder state placeholder.
///
/// The implementation must stay independent from the WSJT-X decoder state. In
/// particular, hash/AP/odd-even memory should not be shared with another
/// decoder profile.
pub struct JtdxStreamDecodeSession {
    _config: StreamDecodeConfig,
    _state: ft8_mod1::Ft8Mod1,
    book: HashCallBook,
    ft8b_workspace: ft8b::Ft8bWorkspace,
    tone8_tables: Tone8Tables,
    ft8apset: Ft8ApSet,
}

impl JtdxStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        let config = config.clone_for_profile_jtdx_high_sensitivity();
        let mut state = ft8_mod1::Ft8Mod1::default();
        state.nft8cycles = config.nft8cycles;
        state.nft8swlcycles = config.nft8swlcycles;
        state.lhound = config.lhound;
        if let Some(mycall) = &config.mycall {
            state.mycall = mycall.clone();
        }
        if let Some(hiscall) = &config.hiscall {
            state.hiscall = hiscall.clone();
        }
        if let Some(hisgrid) = &config.hisgrid {
            state.hisgrid4 = hisgrid.chars().take(4).collect();
        }
        state.nfawide = config.nfa.round() as i32;
        state.nfbwide = config.nfb.round() as i32;
        Self {
            tone8_tables: tone8(&config),
            ft8apset: ft8apset(&config),
            _config: config,
            _state: state,
            book: HashCallBook::new(),
            ft8b_workspace: ft8b::Ft8bWorkspace::default(),
        }
    }

    pub fn is_implemented(&self) -> bool {
        true
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
        self._state.dd8.fill(0.0);
        for (dst, src) in self._state.dd8.iter_mut().zip(samples.iter().copied()) {
            *dst = src;
        }
        let interval = IntervalKind::from_timestamp(timestamp);
        rotate_odd_even_memory(&mut self._state, interval);
        reset_decode_arrays(&mut self._state);
        prepare_qso_memory(&self._config, &mut self._state, interval);
        apply_agc_state(&self._config, &mut self._state);
        self.ft8b_workspace.begin_slot();
        let passes = ft8_decode::decode_passes(&self._config, self._state.avexdt);
        let npass = passes.len();
        let mut decoded = Vec::new();
        let mut dd8m: Option<Vec<f32>> = None;
        for pass in passes {
            apply_pass_sample_shift(&mut self._state.dd8, &mut dd8m, pass.ipass, npass);
            self.ft8b_workspace.new_pass();
            let mut sync8_config = pass.sync8;
            sync8_config.lagcc = self._state.lagcc;
            sync8_config.lagccbail = self._state.lagccbail;
            sync8_config.nfawide = self._state.nfawide;
            sync8_config.nfbwide = self._state.nfbwide;
            let candidates = sync8::sync8(&self._state.dd8, sync8_config);
            let mut newdat1 = true;
            for candidate in candidates {
                let sd_candidate =
                    find_sd_candidate(&self._state, interval, candidate.freq, candidate.dt);
                let context = ft8b::Ft8bCandidateContext {
                    ipass: pass.ipass,
                    npass,
                    lsubtract: matches!(pass.subtract, ft8_decode::SubtractPolicy::Enabled),
                    lhighsens: candidate.sync < 1.9
                        || ((pass.ipass == 2 || pass.ipass == 4 || pass.ipass == 6)
                            && candidate.sync < 3.15),
                    lcqcand: candidate.lcq(),
                    levenint: interval == IntervalKind::Even,
                    loddint: interval == IntervalKind::Odd,
                    lqsomsgdcd: self._state.lqsomsgdcd,
                    lft8sdec: self._state.lft8sdec,
                    stophint: false,
                    nlasttx: self._config.nQSOProgress,
                    call_dt_xdt: call_dt_xdt(&self._state, &self._config, interval),
                    sd_msg: sd_candidate.map(|entry| ft8b::LastRxMsgText::from_str(&entry.msg)),
                    sd_lcq: sd_candidate.is_some_and(|entry| is_cq_like(&entry.msg)),
                    last_rx_msg: self
                        ._state
                        .lastrxmsg
                        .lstate
                        .then(|| ft8b::LastRxMsgText::from_str(&self._state.lastrxmsg.lastmsg)),
                    last_rx_xdt: self
                        ._state
                        .lastrxmsg
                        .lstate
                        .then_some(self._state.lastrxmsg.xdt),
                    last_rx_is_rrr: last_rx_is_rrr(&self._config, &self._state),
                };
                if let Some(result) = ft8b::ft8b(
                    &mut self.ft8b_workspace,
                    &self._config,
                    &self.book,
                    &self.tone8_tables,
                    &self.ft8apset,
                    &mut self._state.dd8,
                    newdat1,
                    candidate,
                    context,
                ) {
                    if rejects_special_deep_decode(&self._config, &self._state, &result) {
                        newdat1 = false;
                        continue;
                    }
                    if result.source == DecodeSource::Ft8s {
                        self._state.lft8sdec = true;
                    }
                    if !is_duplicate_decode(&self._state, &self._config, &result) {
                        save_decode_state(&mut self._state, &result);
                        self.ft8b_workspace.remember_decoded_message(
                            &result.msg37,
                            result.freq,
                            result.dt + 0.5,
                            self._config.mycall.as_deref().unwrap_or(""),
                        );
                        let message = StreamDecodedMessage {
                            freq: result.freq as f64,
                            dt: result.dt as f64,
                            snr: result.snr as f64,
                            msg: result.msg37.clone(),
                            sync: candidate.sync as f64,
                            itone: result.itone,
                        };
                        collect_book(&self.book, &message.msg);
                        on_decode(&message)?;
                        update_qso_memory(&self._config, &mut self._state, &message, interval);
                        update_deep_false_state(&self._config, &mut self._state, &result);
                        decoded.push(message);
                    }
                }
                newdat1 = false;
            }
        }
        self.ft8b_workspace.finish_slot(
            interval == IntervalKind::Even,
            interval == IntervalKind::Odd,
        );
        update_avexdt_after_slot(&self._config, &mut self._state, &decoded);
        Ok(decoded)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntervalKind {
    Even,
    Odd,
    Other,
}

impl IntervalKind {
    fn from_timestamp(timestamp: &SlotTimestamp) -> Self {
        match timestamp.nutc() % 100 {
            0 | 30 => Self::Even,
            15 | 45 => Self::Odd,
            _ => Self::Other,
        }
    }
}

fn rotate_odd_even_memory(state: &mut ft8_mod1::Ft8Mod1, interval: IntervalKind) {
    match interval {
        IntervalKind::Even => {
            state.evencopy.clone_from(&state.even);
            for entry in &mut state.even {
                *entry = ft8_mod1::OddEvenMessage::default();
            }
        }
        IntervalKind::Odd => {
            state.oddcopy.clone_from(&state.odd);
            for entry in &mut state.odd {
                *entry = ft8_mod1::OddEvenMessage::default();
            }
        }
        IntervalKind::Other => {
            clear_odd_even_messages(&mut state.even);
            clear_odd_even_messages(&mut state.odd);
            clear_odd_even_messages(&mut state.evencopy);
            clear_odd_even_messages(&mut state.oddcopy);
        }
    }
}

fn clear_odd_even_messages(entries: &mut [ft8_mod1::OddEvenMessage]) {
    for entry in entries {
        *entry = ft8_mod1::OddEvenMessage::default();
    }
}

fn prepare_qso_memory(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    interval: IntervalKind,
) {
    let hiscall = config.hiscall.as_deref().unwrap_or("").trim();
    if hiscall.is_empty() {
        state.lastrxmsg = ft8_mod1::LastRxMsg::default();
        state.lasthcall.clear();
    } else if state.lastrxmsg.lstate
        && state.lasthcall != hiscall
        && !state.lastrxmsg.lastmsg.contains(hiscall)
    {
        state.lastrxmsg = ft8_mod1::LastRxMsg::default();
    }

    if is_qso_thread(config) && !state.lastrxmsg.lstate && !hiscall.is_empty() {
        if restore_lastrx_from_incall(config, state) {
            return;
        }
        restore_lastrx_from_odd_even_copy(config, state, interval);
    }
}

fn update_qso_memory(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
    interval: IntervalKind,
) {
    save_call_dt(state, message, interval);
    save_odd_even_message(state, message, interval);
    save_incall(config, state, message);

    if !is_focused_qso_decode(config, message) {
        return;
    }
    let Some(msgroot) = msgroot(config) else {
        return;
    };
    if message.msg.starts_with(&msgroot) {
        state.lasthcall = config.hiscall.clone().unwrap_or_default();
        state.lastrxmsg.lastmsg = message.msg.clone();
        state.lastrxmsg.xdt = message.dt as f32;
        state.lastrxmsg.lstate = true;
        state.lqsomsgdcd = true;
    }
}

fn rejects_special_deep_decode(
    config: &StreamDecodeConfig,
    state: &ft8_mod1::Ft8Mod1,
    result: &ft8b::Ft8bDecodeResult,
) -> bool {
    match result.source {
        DecodeSource::Ft8s => {
            if state.lrepliedother {
                return true;
            }
            deep_message_mentions_mycall_late(config, &result.msg37)
        }
        DecodeSource::Ft8sd => {
            if deep_message_mentions_mycall_late(config, &result.msg37) {
                return true;
            }
            let Some(base) = message_base_two_calls(&result.msg37) else {
                return false;
            };
            state
                .msgsrcvd
                .iter()
                .take_while(|msg| !msg.trim().is_empty())
                .any(|msg| msg.trim() == base)
        }
        DecodeSource::Regular => false,
    }
}

fn update_deep_false_state(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    result: &ft8b::Ft8bDecodeResult,
) {
    let dfqso = (result.freq as f64 - config.nfqso).abs();
    let mycall = config.mycall.as_deref().unwrap_or("").trim();
    let hiscall = config.hiscall.as_deref().unwrap_or("").trim();
    let dupe = state
        .allmessages
        .iter()
        .zip(state.allfreq.iter())
        .any(|(msg, freq)| msg == &result.msg37 && (*freq - result.freq).abs() < 45.0);

    if !dupe
        && dfqso < 2.0
        && result.source != DecodeSource::Ft8s
        && mycall.len() > 2
        && !result.msg37.starts_with(&format!("{mycall} "))
        && result.msg37.contains(&format!(" {hiscall} "))
    {
        state.lrepliedother = true;
    }
    if !dupe
        && dfqso < 2.0
        && (1..6).contains(&config.nQSOProgress)
        && result.msg37.starts_with("CQ ")
        && hiscall.len() > 2
        && (result.msg37.starts_with(&format!("CQ {hiscall} "))
            || result.msg37.starts_with(&format!("CQ DX {hiscall} ")))
    {
        state.lrepliedother = true;
    }
    if !dupe
        && result.i3 == 1
        && result.source == DecodeSource::Regular
        && !result.msg37.starts_with("CQ ")
    {
        if let Some(base) = message_base_two_calls(&result.msg37) {
            if let Some(slot) = state.msgsrcvd.iter_mut().find(|msg| msg.trim().is_empty()) {
                *slot = base.to_string();
            }
        }
    }
}

fn deep_message_mentions_mycall_late(config: &StreamDecodeConfig, msg: &str) -> bool {
    let mycall = config.mycall.as_deref().unwrap_or("").trim();
    mycall.len() > 3 && msg.find(&format!(" {mycall} ")).is_some_and(|idx| idx > 0)
}

fn message_base_two_calls(msg: &str) -> Option<&str> {
    let first = msg.find(' ')?;
    let rest = &msg[first + 1..];
    let second_rel = rest.find(' ')?;
    let end = first + 1 + second_rel;
    Some(msg[..end].trim())
}

fn is_focused_qso_decode(config: &StreamDecodeConfig, message: &StreamDecodedMessage) -> bool {
    is_qso_thread(config) && (message.freq - config.nfqso).abs() < 2.0
}

fn msgroot(config: &StreamDecodeConfig) -> Option<String> {
    let mycall = config.mycall.as_deref()?.trim();
    let hiscall = config.hiscall.as_deref()?.trim();
    if mycall.len() < 3 || hiscall.len() < 3 {
        return None;
    }
    Some(format!("{mycall} {hiscall} "))
}

fn last_rx_is_rrr(config: &StreamDecodeConfig, state: &ft8_mod1::Ft8Mod1) -> bool {
    if !state.lastrxmsg.lstate {
        return false;
    }
    let Some(root) = msgroot(config) else {
        return false;
    };
    state.lastrxmsg.lastmsg.trim() == format!("{}RRR", root).trim()
}

fn is_qso_thread(config: &StreamDecodeConfig) -> bool {
    config.nfqso >= config.nfa && config.nfqso <= config.nfb
}

fn restore_lastrx_from_incall(config: &StreamDecodeConfig, state: &mut ft8_mod1::Ft8Mod1) -> bool {
    let Some(msgroot) = msgroot(config) else {
        return false;
    };
    for entry in &state.incall {
        if entry.msg.trim().is_empty() {
            break;
        }
        if entry.msg.starts_with(&msgroot) {
            state.lastrxmsg.lastmsg = entry.msg.clone();
            state.lastrxmsg.xdt = entry.xdt;
            state.lastrxmsg.lstate = true;
            return true;
        }
    }
    false
}

fn restore_lastrx_from_odd_even_copy(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    interval: IntervalKind,
) {
    let hiscall = config.hiscall.as_deref().unwrap_or("").trim();
    if hiscall.is_empty() {
        return;
    }
    let needle = format!(" {hiscall} ");
    let entries = match interval {
        IntervalKind::Even => &state.evencopy,
        IntervalKind::Odd => &state.oddcopy,
        IntervalKind::Other => return,
    };
    for entry in entries {
        if !entry.lstate {
            continue;
        }
        if entry.msg.contains(&needle) {
            state.lastrxmsg.lastmsg = entry.msg.clone();
            state.lastrxmsg.xdt = entry.dt;
            state.lastrxmsg.lstate = true;
            return;
        }
    }
}

fn find_sd_candidate(
    state: &ft8_mod1::Ft8Mod1,
    interval: IntervalKind,
    freq: f32,
    dt: f32,
) -> Option<&ft8_mod1::OddEvenMessage> {
    let entries = match interval {
        IntervalKind::Even => &state.evencopy,
        IntervalKind::Odd => &state.oddcopy,
        IntervalKind::Other => return None,
    };
    entries.iter().find(|entry| {
        entry.lstate && (entry.freq - freq).abs() < 3.0 && (entry.dt - dt).abs() < 0.19
    })
}

fn is_cq_like(msg: &str) -> bool {
    msg.starts_with("CQ ") || msg.starts_with("DE ") || msg.starts_with("QRZ ")
}

fn save_call_dt(
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
    interval: IntervalKind,
) {
    let Some(call2) = extract_call2(&message.msg) else {
        return;
    };
    let target = match interval {
        IntervalKind::Even => &mut state.calldteven,
        IntervalKind::Odd => &mut state.calldtodd,
        IntervalKind::Other => return,
    };
    if target.is_empty() {
        return;
    }
    target.rotate_right(1);
    target[0].call2 = call2;
    target[0].dt = message.dt as f32;
}

fn save_odd_even_message(
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
    interval: IntervalKind,
) {
    let target = match interval {
        IntervalKind::Even => &mut state.even,
        IntervalKind::Odd => &mut state.odd,
        IntervalKind::Other => return,
    };
    if let Some(slot) = target.iter_mut().find(|entry| !entry.lstate) {
        slot.msg = message.msg.clone();
        slot.freq = message.freq as f32;
        slot.dt = message.dt as f32;
        slot.lstate = true;
    }
}

fn save_incall(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
) {
    let mycall = config.mycall.as_deref().unwrap_or("").trim();
    if mycall.len() < 3 {
        return;
    }
    let prefix = format!("{mycall} ");
    if message.msg.starts_with(&prefix) {
        state.incall.rotate_right(1);
        state.incall[0].msg = message.msg.clone();
        state.incall[0].xdt = message.dt as f32;
    }
}

fn call_dt_xdt(
    state: &ft8_mod1::Ft8Mod1,
    config: &StreamDecodeConfig,
    interval: IntervalKind,
) -> Option<f32> {
    let hiscall = config.hiscall.as_deref()?.trim();
    if hiscall.is_empty() {
        return None;
    }
    let source = match interval {
        IntervalKind::Even => &state.calldteven,
        IntervalKind::Odd => &state.calldtodd,
        IntervalKind::Other => return None,
    };
    source
        .iter()
        .find(|entry| entry.call2.trim() == hiscall)
        .map(|entry| entry.dt)
}

fn extract_call2(msg: &str) -> Option<String> {
    let mut parts = msg.split_whitespace();
    let _call1 = parts.next()?;
    let call2 = parts.next()?;
    if call2.len() < 3 {
        None
    } else {
        Some(call2.trim_matches(|c| c == '<' || c == '>').to_string())
    }
}

fn reset_decode_arrays(state: &mut ft8_mod1::Ft8Mod1) {
    state.ndecodes = 0;
    state.nmsg = 0;
    state.lqsomsgdcd = false;
    state.lft8sdec = false;
    for msg in &mut state.allmessages {
        msg.clear();
    }
    state.allsnrs.fill(0);
    state.allfreq.fill(0.0);
    state.lrepliedother = false;
    for msg in &mut state.msgsrcvd {
        msg.clear();
    }
}

fn apply_agc_state(config: &StreamDecodeConfig, state: &mut ft8_mod1::Ft8Mod1) {
    state.lagcc = config.nagcc;
    state.lagccbail = false;
    state.nfawide = config.nfa.round() as i32;
    state.nfbwide = config.nfb.round() as i32;
    state.forcedt = 0.0;

    if state.lagcc || config.lforcesync {
        let agc = agccft8(
            &mut state.dd8,
            state.nfawide,
            state.nfbwide,
            config.lforcesync,
        );
        state.lagccbail = agc.lagccbail;
        state.forcedt = agc.forcedt;
    }
}

fn update_avexdt_after_slot(
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    decoded: &[StreamDecodedMessage],
) {
    let n_ft8_decd = decoded.len();
    if config.lforcesync && n_ft8_decd == 0 {
        state.avexdt = state.forcedt;
        return;
    }

    if n_ft8_decd == 0 {
        return;
    }

    let sumxdt = jtdx_sumxdt(decoded);
    let mean = sumxdt / n_ft8_decd as f32;
    state.avexdt = match n_ft8_decd {
        1 => (1.75 * state.avexdt + 0.25 * sumxdt) / 2.0,
        2 => (1.5 * state.avexdt + 0.5 * mean) / 2.0,
        3 => (1.35 * state.avexdt + 0.65 * mean) / 2.0,
        4 => (1.25 * state.avexdt + 0.75 * mean) / 2.0,
        5 => (1.1 * state.avexdt + 0.9 * mean) / 2.0,
        _ => (state.avexdt + mean) / 2.0,
    };
}

fn jtdx_sumxdt(decoded: &[StreamDecodedMessage]) -> f32 {
    if decoded.len() <= 2 {
        return decoded.iter().map(|message| message.dt as f32).sum();
    }

    let mut sumxdt = 0.0f32;
    let mut dtmed = 0.0f32;
    for i in 0..decoded.len() {
        if i < decoded.len().saturating_sub(2) {
            dtmed = median3(
                decoded[i].dt as f32,
                decoded[i + 1].dt as f32,
                decoded[i + 2].dt as f32,
            );
        }
        sumxdt += dtmed;
    }
    sumxdt
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    if (a > b && a < c) || (a < b && a > c) {
        a
    } else if (b > a && b < c) || (b < a && b > c) {
        b
    } else if (c > a && c < b) || (c < a && c > b) {
        c
    } else {
        a
    }
}

fn is_duplicate_decode(
    state: &ft8_mod1::Ft8Mod1,
    config: &StreamDecodeConfig,
    result: &ft8b::Ft8bDecodeResult,
) -> bool {
    let msg = result.msg37.trim();
    if msg.is_empty() {
        return true;
    }

    let nsnr = result.snr.round() as i32;
    let ndecodes = state.ndecodes.min(state.allmessages.len());
    for i in 0..ndecodes {
        if state.allmessages[i].trim() != msg {
            continue;
        }
        let freq_delta = (state.allfreq[i] - result.freq).abs();
        if config.hide_dupes {
            if nsnr <= state.allsnrs[i] || (nsnr > state.allsnrs[i] && freq_delta < 45.0) {
                return true;
            }
        } else if nsnr <= state.allsnrs[i] && freq_delta < 45.0 {
            return true;
        }
    }

    false
}

fn apply_pass_sample_shift(
    dd8: &mut [f32],
    dd8m: &mut Option<Vec<f32>>,
    ipass: usize,
    npass: usize,
) {
    match ipass {
        4 => {
            if npass == 9 {
                *dd8m = Some(dd8.to_vec());
            }
            for i in 0..dd8.len().saturating_sub(1) {
                dd8[i] = 0.5 * (dd8[i] + dd8[i + 1]);
            }
        }
        7 => {
            if let Some(saved) = dd8m.take() {
                if !dd8.is_empty() && !saved.is_empty() {
                    dd8[0] = saved[0];
                    let n = dd8.len().min(saved.len());
                    for i in 1..n {
                        dd8[i] = 0.5 * (saved[i - 1] + saved[i]);
                    }
                }
            }
        }
        _ => {}
    }
}

fn save_decode_state(state: &mut ft8_mod1::Ft8Mod1, result: &ft8b::Ft8bDecodeResult) {
    let idx = state.ndecodes;
    if idx < state.allmessages.len() {
        state.allmessages[idx] = result.msg37.clone();
        state.allsnrs[idx] = result.snr.round() as i32;
        state.allfreq[idx] = result.freq;
    }
    state.ndecodes += 1;
    state.nmsg = state.ndecodes;
}

fn collect_book(book: &HashCallBook, msg: &str) {
    for part in msg.split_whitespace() {
        let token = part.trim_matches(|c: char| c == ';' || c == ',');
        if is_hashable_callsign_token(token) {
            book.save(token);
        }
    }
}

fn is_hashable_callsign_token(token: &str) -> bool {
    let token = token.trim();
    if token.len() < 3 || token == "<...>" || token.eq_ignore_ascii_case("CQ") {
        return false;
    }
    if matches!(
        token.to_ascii_uppercase().as_str(),
        "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
    ) {
        return false;
    }

    let bare = token.trim_start_matches('<').trim_end_matches('>');
    !is_grid4(bare)
        && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
        && bare.chars().any(|c| c.is_ascii_alphabetic())
        && bare.chars().any(|c| c.is_ascii_digit())
}

fn is_grid4(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4
        && bytes[0] >= b'A'
        && bytes[0] <= b'R'
        && bytes[1] >= b'A'
        && bytes[1] <= b'R'
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}
