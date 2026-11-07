//! Cross-slot state sent by supervised JTDX sessions. Confirmed rows can be
//! replayed if a worker dies before it commits its native signal banks.

use super::{ft8_mod1, ft8b, IntervalKind, JtdxStreamDecodeSession};
use crate::stream::session::{
    StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum JtdxBranch {
    Primary,
    Complement,
}

impl JtdxBranch {
    pub fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Complement => 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JtdxCheckpoint {
    Row(Box<JtdxConfirmed>),
    Complete(Box<JtdxHistory>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JtdxConfirmed {
    pub row: StreamDecodedWithProvenance,
    pub(super) hints: HistoryHints,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct HistoryHints {
    pub(super) source: ft8b::DecodeSource,
    pub(super) i3: i32,
    pub(super) free_text: bool,
    pub(super) hash_message: bool,
    pub(super) focused: bool,
}

impl HistoryHints {
    pub(super) fn new(result: &ft8b::Ft8bDecodeResult, focused: bool) -> Self {
        Self {
            source: result.source,
            i3: result.i3,
            free_text: result.l_free_text,
            hash_message: result.l_hashmsg,
            focused,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JtdxHistory {
    even: Vec<ft8_mod1::OddEvenMessage>,
    odd: Vec<ft8_mod1::OddEvenMessage>,
    call_dt_even: Vec<ft8_mod1::CallSignDt>,
    call_dt_odd: Vec<ft8_mod1::CallSignDt>,
    incall: Vec<ft8_mod1::InCall>,
    last_call: String,
    last_rx: ft8_mod1::LastRxMsg,
    average_dt: f32,
    interval_count: i32,
    signals: Vec<ft8b::SignalHistory>,
    regular_calls: Vec<String>,
    book: super::ft8v2::packjt77::HistoryHashBook,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JtdxRestore {
    pub completed: Option<Box<JtdxHistory>>,
    pub partial: Vec<(String, Vec<JtdxConfirmed>)>,
}

impl JtdxStreamDecodeSession {
    pub(super) fn history(&self) -> JtdxHistory {
        let state = &self._state;
        JtdxHistory {
            even: state.even.clone(),
            odd: state.odd.clone(),
            call_dt_even: state.calldteven.clone(),
            call_dt_odd: state.calldtodd.clone(),
            incall: state.incall.clone(),
            last_call: state.lasthcall.clone(),
            last_rx: state.lastrxmsg.clone(),
            average_dt: state.avexdt,
            interval_count: state.nintcount,
            signals: self
                .ft8b_workspaces
                .iter()
                .map(ft8b::Ft8bWorkspace::signal_history)
                .collect(),
            regular_calls: self.export_regular_hash_calls(),
            book: self.book.snapshot(),
        }
    }

    pub(crate) fn restore_history(&mut self, restore: JtdxRestore) {
        if let Some(history) = restore.completed {
            self.book.restore(history.book);
            self.regular_hash_calls.extend(history.regular_calls);
            let state = &mut self._state;
            state.even = history.even;
            state.odd = history.odd;
            state.calldteven = history.call_dt_even;
            state.calldtodd = history.call_dt_odd;
            state.incall = history.incall;
            state.lasthcall = history.last_call;
            state.lastrxmsg = history.last_rx;
            state.avexdt = history.average_dt;
            state.nintcount = history.interval_count;
            self.ensure_ft8b_workspaces(history.signals.len());
            for (workspace, saved) in self.ft8b_workspaces.iter_mut().zip(history.signals) {
                workspace.restore_signal_history(saved);
            }
        }

        for (slot, confirmed) in restore.partial {
            let Ok(timestamp) = SlotTimestamp::parse(&slot) else {
                continue;
            };
            let interval = IntervalKind::from_timestamp(&timestamp);
            super::rotate_odd_even_memory(&mut self._state, interval);
            super::reset_decode_arrays(&mut self._state);
            super::prepare_qso_memory(&self._config, &mut self._state, interval);
            let mut rows: Vec<StreamDecodedMessage> = Vec::with_capacity(confirmed.len());
            for entry in confirmed {
                let row = entry.row.decode;
                let calls = super::collect_book(&self.book, &row.msg);
                if entry.row.provenance == StreamDecodeProvenance::Regular {
                    self.regular_hash_calls.extend(calls);
                }
                super::update_qso_memory(
                    &self._config,
                    &mut self._state,
                    &row,
                    interval,
                    entry.hints,
                );
                rows.push(row);
            }
            super::update_avexdt_after_slot(&self._config, &mut self._state, &rows);
        }
    }
}
