//! JTDX-oriented FT8 decoder path.
//!
//! This module is intentionally separate from the WSJT-X-aligned decoder. The
//! files below mirror the JTDX FT8 dependency closure and should be filled in
//! from the corresponding JTDX source files.

// Preserve source-audit shape against JTDX Fortran. Non-mirror orchestration code
// remains clippy-checked normally.
#![allow(clippy::all)]

pub mod agccft8;
pub mod call_q;
pub mod callsign_q;
pub mod chkfalse8;
pub mod chkflscall;
pub mod chkgrid;
pub mod chklong8;
pub mod chkspecial8;
pub mod delbraces;
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
pub mod genft8;
pub mod genft8sd;
pub mod indexx;
pub mod msgparser;
pub mod partintft8;
pub mod searchcalls;
pub mod sync8;
pub mod sync8d;
pub mod syncdist;
pub mod tone8;
pub mod tone8myc;
pub mod tonesd;
pub mod twkfreq1;

use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
    StreamSnrSource,
};
use crate::stream::time::SlotTimestamp;

use self::agccft8::agccft8;
use self::ft8apset::{ft8apset, Ft8ApSet};
use self::ft8b::DecodeSource;
use self::ft8v2::packjt77::HashCallBook;
use self::tone8::{tone8, Tone8Tables};

/// JTDX decoder state.
///
/// The implementation must stay independent from the WSJT-X decoder state. In
/// particular, hash/AP/odd-even memory should not be shared with another
/// decoder profile.
pub struct JtdxStreamDecodeSession {
    _config: StreamDecodeConfig,
    _state: ft8_mod1::Ft8Mod1,
    book: HashCallBook,
    regular_hash_calls: std::collections::HashSet<String>,
    last_provenance: Vec<StreamDecodedWithProvenance>,
    ft8b_workspaces: Vec<ft8b::Ft8bWorkspace>,
    tone8_tables: Tone8Tables,
    ft8apset: Ft8ApSet,
}

impl JtdxStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        let config = config.clone_for_profile_jtdx_high_sensitivity();
        let mut state = ft8_mod1::Ft8Mod1::default();
        state.nft8cycles = config.nft8cycles;
        state.nft8swlcycles = config.nft8swlcycles;
        state.nft8rxfsens = config.nft8rxfsens;
        state.lhound = config.lhound;
        state.nintcount = 3;
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
            regular_hash_calls: std::collections::HashSet::new(),
            last_provenance: Vec::new(),
            ft8b_workspaces: Vec::new(),
        }
    }

    pub fn is_implemented(&self) -> bool {
        true
    }

    pub fn import_hash_calls(&mut self, calls: &[String]) {
        for call in calls {
            self.book.save(call);
        }
    }

    pub fn export_regular_hash_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = self.regular_hash_calls.iter().cloned().collect();
        calls.sort();
        calls
    }

    pub fn decode_slot_streaming_with_provenance_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedWithProvenance>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.decode_slot_streaming_at(timestamp, samples, on_decode)?;
        Ok(std::mem::take(&mut self.last_provenance))
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
        self.last_provenance.clear();
        self._state.dd8.fill(0.0);
        for (dst, src) in self._state.dd8.iter_mut().zip(samples.iter().copied()) {
            *dst = src;
        }
        let interval = IntervalKind::from_timestamp(timestamp);
        rotate_odd_even_memory(&mut self._state, interval);
        reset_decode_arrays(&mut self._state);
        prepare_qso_memory(&self._config, &mut self._state, interval);
        apply_agc_state(&self._config, &mut self._state);
        let passes = ft8_decode::decode_passes(&self._config, self._state.avexdt);
        let npass = passes.len();
        let bands = jtdx_decode_bands(&self._config);
        self.ensure_ft8b_workspaces(bands.len());
        for workspace in &mut self.ft8b_workspaces {
            workspace.begin_slot();
        }
        let mut decoded = Vec::new();
        let mut dd8m: Option<Vec<f32>> = None;
        for pass_block in jtdx_pass_blocks(&passes) {
            if let Some(first) = pass_block.first() {
                apply_pass_sample_shift(&mut self._state.dd8, &mut dd8m, first.ipass, npass);
            }
            for band in &bands {
                for pass in pass_block {
                    self.decode_pass_band(
                        *pass,
                        npass,
                        *band,
                        interval,
                        &mut decoded,
                        &mut on_decode,
                    )?;
                }
            }
        }
        for workspace in &mut self.ft8b_workspaces {
            workspace.finish_slot(
                interval == IntervalKind::Even,
                interval == IntervalKind::Odd,
                normalized_context_call(self._config.mycall.as_deref()).is_some(),
                self._state.lqsomsgdcd,
            );
        }
        update_avexdt_after_slot(&self._config, &mut self._state, &decoded);
        Ok(decoded)
    }

    fn ensure_ft8b_workspaces(&mut self, len: usize) {
        if self.ft8b_workspaces.len() < len {
            self.ft8b_workspaces
                .resize_with(len, ft8b::Ft8bWorkspace::default);
        }
    }

    fn decode_pass_band<F>(
        &mut self,
        pass: ft8_decode::JtdxPass,
        npass: usize,
        band: JtdxDecodeBand,
        interval: IntervalKind,
        decoded: &mut Vec<StreamDecodedMessage>,
        on_decode: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let workspace = &mut self.ft8b_workspaces[band.index];
        workspace.new_pass();
        let mut band_config = self._config.clone();
        band_config.nfa = band.nfa as f64;
        band_config.nfb = band.nfb as f64;

        let mut sync8_config = pass.sync8;
        sync8_config.nfa = band.nfa;
        sync8_config.nfb = band.nfb;
        sync8_config.lqsothread =
            band_config.nfqso >= band_config.nfa && band_config.nfqso <= band_config.nfb;
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
                stophint: band_config.stophint,
                nlasttx: band_config.nQSOProgress,
                call_dt_xdt: call_dt_xdt(&self._state, &band_config, interval),
                sd_msg: sd_candidate.map(|(_, entry)| ft8b::LastRxMsgText::from_str(&entry.msg)),
                sd_lcq: sd_candidate.is_some_and(|(_, entry)| is_cq_like(&entry.msg)),
                sd_index: sd_candidate.map(|(index, _)| index),
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
                last_rx_is_rrr: last_rx_is_rrr(&band_config, &self._state),
            };
            if let Some(result) = ft8b::ft8b(
                workspace,
                &band_config,
                &self.book,
                &self.tone8_tables,
                &self.ft8apset,
                &mut self._state.dd8,
                newdat1,
                candidate,
                context,
            ) {
                if rejects_special_deep_decode(&band_config, &self._state, &result) {
                    newdat1 = false;
                    continue;
                }
                for result in result_variants(result) {
                    if result.source == DecodeSource::Ft8s {
                        self._state.lft8sdec = true;
                    }
                    if result.source == DecodeSource::Ft8sd {
                        clear_sd_candidate(&mut self._state, interval, context.sd_index);
                    }
                    if is_duplicate_decode(&self._state, &band_config, &result, band.numthreads) {
                        continue;
                    }
                    workspace.remember_decoded_message(
                        &result.msg37,
                        result.freq,
                        result.dt + 0.5,
                        band_config.mycall.as_deref().unwrap_or(""),
                        is_standard_context_call(band_config.mycall.as_deref()),
                    );
                    let message = StreamDecodedMessage {
                        freq: result.freq as f64,
                        dt: result.dt as f64,
                        snr: result.snr as f64,
                        snr_source: StreamSnrSource::Decoder,
                        deep_confidence: None,
                        msg: result.msg37.clone(),
                        sync: candidate.sync as f64,
                        itone: result.itone,
                    };
                    if result.source == DecodeSource::Regular && result.iaptype == 0 {
                        for call in collect_book(&self.book, &message.msg) {
                            self.regular_hash_calls.insert(call);
                        }
                    } else {
                        collect_book(&self.book, &message.msg);
                    }
                    self.last_provenance.push(StreamDecodedWithProvenance {
                        decode: message.clone(),
                        provenance: jtdx_stream_provenance(&result),
                    });
                    on_decode(&message)?;
                    update_qso_memory(&band_config, &mut self._state, &message, interval, &result);
                    update_deep_false_state(&band_config, &mut self._state, &result);
                    save_decode_state(&mut self._state, &result);
                    decoded.push(message);
                }
            }
            newdat1 = false;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntervalKind {
    Even,
    Odd,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct JtdxDecodeBand {
    index: usize,
    nfa: i32,
    nfb: i32,
    numthreads: usize,
}

fn jtdx_decode_bands(config: &StreamDecodeConfig) -> Vec<JtdxDecodeBand> {
    let (nfa, nfb) = active_jtdx_decode_band(config);
    let numthreads = jtdx_numthreads(config);
    if numthreads <= 1 || nfb <= nfa + 1 {
        return vec![JtdxDecodeBand {
            index: 0,
            nfa,
            nfb,
            numthreads: 1,
        }];
    }

    let nfdelta = ((nfb - nfa).abs() as f64 / numthreads as f64).round() as i32;
    let mut mids = Vec::with_capacity(numthreads.saturating_sub(1));
    for index in 1..numthreads {
        let mut nfmid = nfa + nfdelta * index as i32;
        if index == numthreads - 1 && nfmid + 1 > nfb {
            nfmid = nfb - 1;
        }
        mids.push(nfmid);
    }

    let bands: Vec<_> = (0..numthreads)
        .map(|index| {
            let lo = if index == 0 { nfa } else { mids[index - 1] + 1 };
            let hi = if index + 1 == numthreads {
                nfb
            } else {
                mids[index]
            };
            JtdxDecodeBand {
                index,
                nfa: lo,
                nfb: hi,
                numthreads,
            }
        })
        .collect();

    jtdx_section_order(numthreads)
        .into_iter()
        .filter_map(|index| bands.get(index).copied())
        .collect()
}

fn active_jtdx_decode_band(config: &StreamDecodeConfig) -> (i32, i32) {
    let mut nfa = config.nfa.round() as i32;
    let mut nfb = config.nfb.round() as i32;
    let nfqso = config.nfqso.round() as i32;

    if config.filter && nfqso >= nfa && nfqso <= nfb {
        let half_width = if config.lhound { 290 } else { 60 };
        nfa = nfa.max(nfqso - half_width);
        nfb = nfb.min(nfqso + half_width);
    }
    if config.nagain && nfqso >= nfa && nfqso <= nfb {
        nfa = nfa.max(nfqso - 25);
        nfb = nfb.min(nfqso + 25);
    }

    (nfa, nfb)
}

fn jtdx_numthreads(config: &StreamDecodeConfig) -> usize {
    let numcores = std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(1);
    let nuserthr = config.jtdx_threads;
    let mut numthreads = if nuserthr == 0 {
        match numcores {
            0 | 1 => 1,
            2..=4 => numcores - 1,
            5..=8 => numcores - 2,
            9..=15 => numcores - 3,
            16..=20 => numcores - 4,
            21..=29 => numcores - 5,
            _ => 24,
        }
    } else if nuserthr < 25 {
        if numcores >= nuserthr {
            nuserthr
        } else {
            numcores
        }
    } else {
        1
    };
    if config.filter {
        numthreads = numthreads.min(8);
    }
    if config.nagain {
        numthreads = numthreads.min(4);
    }
    numthreads.max(1)
}

fn jtdx_section_order(numthreads: usize) -> Vec<usize> {
    if numthreads <= 3 {
        return (0..numthreads).collect();
    }

    let mut order = Vec::with_capacity(numthreads);
    if numthreads % 2 == 0 {
        let left = numthreads / 2 - 1;
        let right = numthreads / 2;
        order.push(left);
        order.push(right);
        for offset in 1..=left {
            order.push(left - offset);
            let upper = right + offset;
            if upper < numthreads {
                order.push(upper);
            }
        }
    } else {
        let center = numthreads / 2;
        order.push(center);
        for offset in 1..=center {
            order.push(center - offset);
            let upper = center + offset;
            if upper < numthreads {
                order.push(upper);
            }
        }
    }
    order
}

fn jtdx_pass_blocks(passes: &[ft8_decode::JtdxPass]) -> Vec<&[ft8_decode::JtdxPass]> {
    passes.chunks(3).filter(|chunk| !chunk.is_empty()).collect()
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

    if is_qso_thread(config) && !state.lastrxmsg.lstate && !config.stophint && !hiscall.is_empty() {
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
    result: &ft8b::Ft8bDecodeResult,
) {
    save_call_dt(state, message, interval, result);
    save_odd_even_message(config, state, message, interval, result);
    save_incall(config, state, message);

    if !is_focused_qso_decode(config, message) {
        return;
    }
    let Some(msgroot) = msgroot(config) else {
        return;
    };
    let ft8s = result.source == DecodeSource::Ft8s;
    if ((result.i3 == 1 && !ft8s) || ft8s) && message.msg.starts_with(&msgroot) {
        state.lasthcall = config.hiscall.clone().unwrap_or_default();
        state.lastrxmsg.lastmsg = message.msg.clone();
        state.lastrxmsg.xdt = message.dt as f32;
        state.lastrxmsg.lstate = true;
        state.lqsomsgdcd = true;
    } else if config.hiscall.as_deref().unwrap_or("").trim().len() > 3
        && !state.lqsomsgdcd
        && message.msg.starts_with(&msgroot)
    {
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

fn result_variants(result: ft8b::Ft8bDecodeResult) -> Vec<ft8b::Ft8bDecodeResult> {
    if !result.l_special || result.msg37_2.trim().is_empty() {
        return vec![result];
    }
    let mut second = result.clone();
    second.msg37 = second.msg37_2.clone();
    vec![result, second]
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
        && result.i3 == 1
        && mycall.len() > 2
        && result.source != DecodeSource::Ft8s
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
    let (nfa, nfb) = active_jtdx_decode_band(config);
    let nfqso = config.nfqso.round() as i32;
    nfqso >= nfa && nfqso <= nfb
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
) -> Option<(usize, &ft8_mod1::OddEvenMessage)> {
    let entries = match interval {
        IntervalKind::Even => &state.evencopy,
        IntervalKind::Odd => &state.oddcopy,
        IntervalKind::Other => return None,
    };
    entries.iter().enumerate().rev().find(|(_, entry)| {
        entry.lstate && (entry.freq - freq).abs() < 3.0 && (entry.dt - dt).abs() < 0.19
    })
}

fn clear_sd_candidate(state: &mut ft8_mod1::Ft8Mod1, interval: IntervalKind, index: Option<usize>) {
    let Some(index) = index else {
        return;
    };
    let entries = match interval {
        IntervalKind::Even => &mut state.evencopy,
        IntervalKind::Odd => &mut state.oddcopy,
        IntervalKind::Other => return,
    };
    if let Some(entry) = entries.get_mut(index) {
        entry.lstate = false;
    }
}

fn is_cq_like(msg: &str) -> bool {
    msg.starts_with("CQ ") || msg.starts_with("DE ") || msg.starts_with("QRZ ")
}

fn save_call_dt(
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
    interval: IntervalKind,
    result: &ft8b::Ft8bDecodeResult,
) {
    if result.l_free_text {
        return;
    }
    let Some(call2) = extract_call(&message.msg) else {
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
    config: &StreamDecodeConfig,
    state: &mut ft8_mod1::Ft8Mod1,
    message: &StreamDecodedMessage,
    interval: IntervalKind,
    result: &ft8b::Ft8bDecodeResult,
) {
    if !should_save_odd_even_message(config, message, result) {
        return;
    }
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

fn should_save_odd_even_message(
    config: &StreamDecodeConfig,
    message: &StreamDecodedMessage,
    result: &ft8b::Ft8bDecodeResult,
) -> bool {
    if result.i3 == 4 && message.msg.starts_with("CQ ") {
        return true;
    }
    if result.l_free_text || result.l_hashmsg || message.msg.contains('<') {
        return false;
    }
    let ft8sd = result.source == DecodeSource::Ft8sd;
    if !((result.i3 == 1 && !ft8sd) || ft8sd) {
        return false;
    }
    let first = message.msg.split_whitespace().next().unwrap_or("");
    if first == config.mycall.as_deref().unwrap_or("").trim() {
        return false;
    }
    !(message.msg.contains('/') && !message.msg.starts_with("CQ "))
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

fn extract_call(msg: &str) -> Option<String> {
    let mut parts = msg.split_whitespace();
    let part1 = parts.next()?;
    let part2 = parts.next().unwrap_or("");
    let part3 = parts.next().unwrap_or("");

    let call2 = if msg.starts_with("CQ ") || msg.starts_with("DE ") || msg.starts_with("QRZ ") {
        match part2.len() {
            5.. => part2,
            4 => {
                let bytes = part2.as_bytes();
                if bytes.get(1).is_some_and(u8::is_ascii_digit)
                    || bytes.get(2).is_some_and(u8::is_ascii_digit)
                {
                    part2
                } else {
                    part3
                }
            }
            3 => {
                let bytes = part2.as_bytes();
                if bytes.first().is_some_and(u8::is_ascii_uppercase)
                    && bytes.get(1).is_some_and(u8::is_ascii_digit)
                {
                    part2
                } else {
                    part3
                }
            }
            2 => part3,
            _ => "",
        }
    } else if part1.len() > 3 && part1.len() < 13 {
        part2
    } else {
        ""
    };

    (!call2.trim().is_empty()).then(|| call2.to_string())
}

fn normalized_context_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim().trim_start_matches('<').trim_end_matches('>');
    if call.len() < 3 {
        return None;
    }
    Some(call.to_ascii_uppercase())
}

fn is_standard_context_call(call: Option<&str>) -> bool {
    normalized_context_call(call).is_some_and(|call| {
        !call.contains('/')
            && call.len() <= 6
            && call
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    })
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
    let mut sumxdt = 0.0f32;
    if config.lforcesync {
        state.nintcount = 3;
    } else if state.nintcount > 0 {
        state.nintcount -= 1;
    }

    if n_ft8_decd == 0 {
        if config.lforcesync {
            // JTDX `decoder.f90` temporarily assigns `forcedt` for the
            // DecodeFinished report, then resets `avexdt` to zero before the
            // next slot. ft8rs stores only the persistent next-slot state.
            state.avexdt = 0.0;
        }
        return;
    }

    if n_ft8_decd > 2 {
        sumxdt = jtdx_sumxdt_median(decoded);
        if n_ft8_decd > 5 {
            state.avexdt = (state.avexdt + sumxdt / n_ft8_decd as f32) / 2.0;
        } else if n_ft8_decd == 5 {
            state.avexdt = (1.1 * state.avexdt + 0.9 * sumxdt / n_ft8_decd as f32) / 2.0;
        } else if n_ft8_decd == 4 {
            state.avexdt = (1.25 * state.avexdt + 0.75 * sumxdt / n_ft8_decd as f32) / 2.0;
        } else if n_ft8_decd == 3 {
            state.avexdt = (1.35 * state.avexdt + 0.65 * sumxdt / n_ft8_decd as f32) / 2.0;
        }
    } else {
        for message in decoded.iter().take(n_ft8_decd) {
            sumxdt += message.dt as f32;
        }
        if n_ft8_decd == 2 {
            state.avexdt = (1.5 * state.avexdt + 0.5 * sumxdt / n_ft8_decd as f32) / 2.0;
        } else if n_ft8_decd == 1 {
            state.avexdt = (1.75 * state.avexdt + 0.25 * sumxdt) / 2.0;
        }
    }
    if n_ft8_decd > 10 && state.nintcount == 1 {
        state.avexdt = sumxdt / n_ft8_decd as f32;
    }
}

fn jtdx_sumxdt_median(decoded: &[StreamDecodedMessage]) -> f32 {
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
    numthreads: usize,
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
        } else if nsnr > state.allsnrs[i] && freq_delta < 45.0 && numthreads != 1 {
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

fn jtdx_stream_provenance(result: &ft8b::Ft8bDecodeResult) -> StreamDecodeProvenance {
    match result.source {
        DecodeSource::Ft8s | DecodeSource::Ft8sd => StreamDecodeProvenance::JtdxDeep,
        DecodeSource::Regular if result.iaptype == 0 => StreamDecodeProvenance::Regular,
        DecodeSource::Regular => StreamDecodeProvenance::A7Memory,
    }
}

fn collect_book(book: &HashCallBook, msg: &str) -> Vec<String> {
    let mut saved = Vec::new();
    for part in msg.split_whitespace() {
        let token = part.trim_matches(|c: char| c == ';' || c == ',');
        if is_hashable_callsign_token(token) {
            book.save(token);
            saved.push(
                token
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_ascii_uppercase(),
            );
        }
    }
    saved
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
