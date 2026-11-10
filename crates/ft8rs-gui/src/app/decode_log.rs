//! Receive log rows, comparison, filtering, and slot identity.
use std::time::{SystemTime, UNIX_EPOCH};

use ft8rs::stream::session::StreamDecodeProvenance;

use super::norm;

/// Hard cap on retained decode rows. Painting is virtualized; DecodeBatch
/// applies this limit after each insertion and compacts once per input frame.
const MAX_ROWS: usize = 20_000;

/// A clock step begins a new timeline, including when UTC moves backwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SlotKey {
    pub(super) clock_segment: u64,
    pub(super) utc_slot: u64,
}

impl SlotKey {
    fn utc(utc_slot: u64) -> Self {
        Self {
            clock_segment: 0,
            utc_slot,
        }
    }
}

/// Where a decode row came from, for the compare-mode coloring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DecodeSource {
    Local,
    External,
    Both,
}

#[cfg_attr(test, derive(Clone, Debug, PartialEq))]
pub(super) struct Row {
    pub(super) text: String,
    pub(super) slot: SlotKey,
    // Label captured when the first decode for this slot entered the log.
    pub(super) slot_band: Option<&'static str>,
    // A paired row displays the local measurement and also retains the UDP one.
    paired_external: Option<(f64, f64)>,
    pub(super) freq: f64,
    dt: f64,
    pub(super) tokens: Vec<String>,
    pub(super) source: DecodeSource,
    filterable: Option<FilterableMessage>,
}

impl Row {
    fn local_filterable(&self) -> Option<FilterableMessage> {
        if self.source == DecodeSource::Local {
            self.filterable
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(super) enum DisplayItem {
    Decode(usize),
    Separator(SlotKey, Option<&'static str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterableMessage {
    Contest,
    Telemetry,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct DisplayFilters {
    pub(super) hide_contest: bool,
    pub(super) hide_telemetry: bool,
}

impl DisplayFilters {
    const NONE: Self = Self {
        hide_contest: false,
        hide_telemetry: false,
    };

    fn hides(self, row: &Row) -> bool {
        match row.local_filterable() {
            Some(FilterableMessage::Contest) => self.hide_contest,
            Some(FilterableMessage::Telemetry) => self.hide_telemetry,
            None => false,
        }
    }
}

#[derive(Default)]
pub(super) struct DecodeLog {
    rows: Vec<Row>,
    items: Vec<DisplayItem>,
}

impl DecodeLog {
    pub(super) fn batch(&mut self, tolerance: f64, filters: DisplayFilters) -> DecodeBatch<'_> {
        DecodeBatch::new(&mut self.rows, &mut self.items, tolerance, filters)
    }

    pub(super) fn display(&self) -> (&[Row], &[DisplayItem]) {
        (&self.rows, &self.items)
    }

    pub(super) fn refilter(&mut self, filters: DisplayFilters) {
        self.items = display_items(&self.rows, filters);
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.items.clear();
    }
}

fn absolute_slot(slot_of_day: u32, now_seconds: u64) -> u64 {
    const SLOTS_PER_DAY: u64 = 5760;
    let current = now_seconds / 15;
    let today = current / SLOTS_PER_DAY * SLOTS_PER_DAY + u64::from(slot_of_day);
    [
        today.saturating_sub(SLOTS_PER_DAY),
        today,
        today + SLOTS_PER_DAY,
    ]
    .into_iter()
    .min_by_key(|candidate| candidate.abs_diff(current))
    .unwrap()
}

/// Build a divider for each slot with at least one visible decode.
fn display_items(rows: &[Row], filters: DisplayFilters) -> Vec<DisplayItem> {
    let mut items = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.iter().enumerate() {
        if !filters.hides(row) {
            push_display_row(&mut items, rows, index);
        }
    }
    items
}

fn push_display_row(items: &mut Vec<DisplayItem>, rows: &[Row], index: usize) {
    let slot = rows[index].slot;
    if !matches!(items.last(), Some(DisplayItem::Decode(last)) if rows[*last].slot == slot) {
        items.push(DisplayItem::Separator(slot, rows[index].slot_band));
    }
    items.push(DisplayItem::Decode(index));
}

fn ft8_payload(tones: &[i32; 79]) -> Option<[u8; 77]> {
    const COSTAS: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
    if tones[..7] != COSTAS || tones[36..43] != COSTAS || tones[72..] != COSTAS {
        return None;
    }

    // All 77 payload bits are systematic and fit in the first data block.
    const UNGRAY: [u8; 8] = [0, 1, 3, 2, 6, 4, 5, 7];
    let mut payload = [0; 77];
    for (bits, &tone) in payload.chunks_mut(3).zip(&tones[7..33]) {
        let symbol = *UNGRAY.get(tone as usize)?;
        for (bit, shift) in bits.iter_mut().zip((0..3).rev()) {
            *bit = (symbol >> shift) & 1;
        }
    }
    Some(payload)
}

fn filterable_message(payload: Option<&[u8; 77]>, message: &str) -> Option<FilterableMessage> {
    let message_type = payload.map(|bits| {
        let subtype = (bits[71] << 2) | (bits[72] << 1) | bits[73];
        let kind = (bits[74] << 2) | (bits[75] << 1) | bits[76];
        (kind, subtype)
    });
    match message_type {
        Some((0, 0)) => None, // Free text can contain either pattern literally.
        Some((0, 5)) => Some(FilterableMessage::Telemetry),
        Some((0, 3 | 4) | (3 | 5, _)) => Some(FilterableMessage::Contest),
        _ if matches_contest_filter(message) => Some(FilterableMessage::Contest),
        _ => None,
    }
}

fn matches_contest_filter(message: &str) -> bool {
    let mut words = message_body_tokens(message).peekable();
    while let Some(word) = words.next() {
        let call = word.trim_matches(['<', '>']);
        if (call.len() > 2 && call.ends_with("/R"))
            || (word == "R" && words.peek().is_some_and(|grid| is_grid4(grid)))
        {
            return true;
        }
    }
    false
}

pub(super) fn separator_label(utc_slot: u64, band: Option<&str>) -> String {
    let seconds = utc_slot * 15;
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    // Civil date from Unix days (Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let band = band.map_or_else(String::new, |band| format!("{band} - "));
    format!(
        "------ {year:04}-{month:02}-{day:02} - {:02}:{:02}:{:02} UTC - {band}FT8 ------",
        time / 3_600,
        time / 60 % 60,
        time % 60,
    )
}

/// Provenance tag shown as a trailing column after the message
/// (`a7`/`ap`/`a8`/deep/imported). Unknown provenance stays unknown internally,
/// but has no visible tag: it is not a low-confidence message annotation.
fn provenance_tag(provenance: StreamDecodeProvenance) -> &'static str {
    match provenance {
        StreamDecodeProvenance::Unknown => "",
        StreamDecodeProvenance::CooperativeRecovery => "sic",
        StreamDecodeProvenance::ContextRecovery => "ctx",
        StreamDecodeProvenance::PlusCompatibility => "Pc",
        StreamDecodeProvenance::CandidateRetry => "Pr",
        StreamDecodeProvenance::Experiment => "exp",
        StreamDecodeProvenance::A7Memory => "a7",
        StreamDecodeProvenance::ApMask => "ap",
        StreamDecodeProvenance::WsjtxAp(ap_type) => ap_type.annotation(),
        StreamDecodeProvenance::A8List => "a8",
        StreamDecodeProvenance::JtdxDeep => "dp",
        StreamDecodeProvenance::ImportedMemory => "im",
        StreamDecodeProvenance::Regular => "",
    }
}

/// Format a UTC time-of-day given in milliseconds since midnight as `HHMMSS`
/// (matching `SlotTimestamp::format_time`), for external decode rows.
fn format_ms_time(ms: u32) -> String {
    let s = ms / 1000;
    format!("{:02}{:02}{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// JTDX can remove brackets around resolved hashes. Unpack without a callbook
/// to find their positions, then keep the calls already resolved by the decoder.
fn local_display_message(message: &str, payload: Option<&[u8; 77]>) -> String {
    use ft8rs::decode::lib_jtdx::ft8v2::packjt77::unpack77;

    let body = clean_message(message);
    let Some(template) = payload.and_then(|bits| unpack77(bits, None)) else {
        return body;
    };
    if !template.contains("<...>") {
        return body;
    }

    let words: Vec<_> = body.split_whitespace().collect();
    let template: Vec<_> = template.split_whitespace().collect();
    if let Some(restored) = restore_hash_brackets(&words, &template) {
        return restored;
    }

    // JTDX can emit the two legs of a compound transmission as separate rows.
    let restored = match template.as_slice() {
        [first, "RR73;", second, hash, report] => {
            restore_hash_brackets(&words, &[*first, *hash, "RR73"])
                .or_else(|| restore_hash_brackets(&words, &[*second, *hash, *report]))
        }
        ["TU;", _, second, ..] => restore_hash_brackets(&words, &["DE", *second, "TU"])
            .or_else(|| restore_hash_brackets(&words, &template[1..])),
        _ => None,
    };
    restored.unwrap_or(body)
}

fn restore_hash_brackets(words: &[&str], template: &[&str]) -> Option<String> {
    if words.len() != template.len() {
        return None;
    }
    words
        .iter()
        .zip(template)
        .map(|(&word, &field)| match field {
            "<...>" if word.starts_with('<') && word.ends_with('>') => Some(word.into()),
            "<...>"
                if word.len() >= 3
                    && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/') =>
            {
                Some(format!("<{word}>"))
            }
            _ if word == field => Some(word.into()),
            _ => None,
        })
        .collect::<Option<Vec<String>>>()
        .map(|tokens| tokens.join(" "))
}

/// Keep all annotations supplied locally, with provenance in the trailing column.
fn local_decode_text(
    record: &ft8rs_engine::protocol::DecodeRecord,
    payload: Option<&[u8; 77]>,
) -> String {
    let row = &record.row;
    let mut annotations: Vec<_> = row
        .msg
        .split_whitespace()
        .filter(|t| is_annotation(t))
        .collect();
    let provenance = provenance_tag(record.provenance);
    if !provenance.is_empty() && !annotations.contains(&provenance) {
        annotations.push(provenance);
    }
    format!(
        "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
        record.timestamp.format_time(),
        row.snr.round() as i32,
        row.dt,
        row.freq.round() as i64,
        local_display_message(&row.msg, payload),
        annotations.join(" "),
    )
}

/// External rows display only the message body, without decoder annotations.
fn external_decode_text(decode: &crate::wsjtx_udp::ExternalDecode) -> String {
    format!(
        "{:<6} {:>3} {:>5.1} {:>5}  {:<20} ",
        format_ms_time(decode.ms_since_midnight),
        decode.snr,
        decode.dt,
        decode.freq,
        clean_message(&decode.message),
    )
}

/// One frame of input. The logical prefix preserves per-insertion eviction:
/// removed rows cannot participate in later folds, even before physical drain.
pub(super) struct DecodeBatch<'a> {
    rows: &'a mut Vec<Row>,
    display: &'a mut Vec<DisplayItem>,
    prefix: usize,
    rebuild: bool,
    tolerance: f64,
    filters: DisplayFilters,
}

impl<'a> DecodeBatch<'a> {
    fn new(
        rows: &'a mut Vec<Row>,
        display: &'a mut Vec<DisplayItem>,
        tolerance: f64,
        filters: DisplayFilters,
    ) -> Self {
        Self {
            rows,
            display,
            prefix: 0,
            rebuild: false,
            tolerance,
            filters,
        }
    }

    pub(super) fn insert(&mut self, mut incoming: Row) {
        let live = &mut self.rows[self.prefix..];
        let low = live.partition_point(|row| row.slot < incoming.slot);
        let high = live.partition_point(|row| row.slot <= incoming.slot);
        let slot_rows = &mut live[low..high];
        let hidden = self.filters.hides(&incoming);
        if let Some(filterable_changed) = fold_decode_row(slot_rows, &incoming, self.tolerance) {
            self.rebuild |= filterable_changed && self.filters != DisplayFilters::NONE;
            return;
        }
        if let Some(first) = slot_rows.first() {
            incoming.slot_band = first.slot_band;
        }
        let index = self.prefix + high;
        self.rebuild |= index != self.rows.len();
        self.rows.insert(index, incoming);
        if !self.rebuild && !hidden {
            push_display_row(self.display, self.rows, index);
        }
        let prefix = self.rows.len().saturating_sub(MAX_ROWS);
        if prefix > self.prefix {
            self.prefix = prefix;
            self.rebuild = true;
        }
    }
}

impl Drop for DecodeBatch<'_> {
    fn drop(&mut self) {
        if self.prefix != 0 {
            self.rows.drain(..self.prefix);
        }
        if self.rebuild {
            *self.display = display_items(self.rows, self.filters);
        }
    }
}

/// Map time-of-day to its nearest real UTC slot, including late midnight rows.
fn decode_row(
    slot_of_day: u32,
    freq: f64,
    dt: f64,
    text: String,
    tokens: Vec<String>,
    source: DecodeSource,
    filterable: Option<FilterableMessage>,
) -> Row {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Row {
        text,
        slot: SlotKey::utc(absolute_slot(slot_of_day, now)),
        slot_band: None,
        paired_external: None,
        freq,
        dt,
        tokens,
        source,
        filterable,
    }
}

pub(super) fn local_decode_row(record: ft8rs_engine::protocol::DecodeRecord) -> Row {
    let payload = ft8_payload(&record.row.itone);
    let mut row = decode_row(
        record.timestamp.milliseconds_since_midnight() / 15_000,
        record.row.freq,
        record.row.dt,
        local_decode_text(&record, payload.as_ref()),
        norm_tokens(&record.row.msg),
        DecodeSource::Local,
        filterable_message(payload.as_ref(), &record.row.msg),
    );
    if let Some(seconds) = record
        .timestamp
        .unix_seconds_utc()
        .filter(|&seconds| seconds >= 0)
    {
        row.slot.utc_slot = seconds as u64 / 15;
    }
    row
}

pub(super) fn external_decode_row(decode: crate::wsjtx_udp::ExternalDecode) -> Row {
    decode_row(
        decode.ms_since_midnight / 15_000,
        decode.freq as f64,
        decode.dt,
        external_decode_text(&decode),
        norm_tokens(&decode.message),
        DecodeSource::External,
        None,
    )
}

/// Merge repeated observations of the same transmission. SNR and annotation
/// changes do not create a new signal. DT keeps distinct same-message signals
/// apart; cross-source matching retains the configured frequency tolerance.
/// A match reports whether its display-filter category changed.
fn fold_decode_row(rows: &mut [Row], incoming: &Row, tolerance: f64) -> Option<bool> {
    let matches = |row: &Row, freq: f64, dt: f64, tolerance: f64| {
        row.slot == incoming.slot
            && (freq - incoming.freq).abs() <= tolerance
            && (dt - incoming.dt).abs() <= 0.3
            && tokens_compatible(&row.tokens, &incoming.tokens)
    };
    // Prefer repeats from the same source, even if a different row happens to
    // be in cross-source range. A pair already contains both observations.
    let same_source = rows.iter().position(|row| {
        let measurement = match (row.source, incoming.source) {
            (DecodeSource::Local | DecodeSource::Both, DecodeSource::Local)
            | (DecodeSource::External, DecodeSource::External) => Some((row.freq, row.dt)),
            (DecodeSource::Both, DecodeSource::External) => row.paired_external,
            _ => None,
        };
        measurement.is_some_and(|(freq, dt)| matches(row, freq, dt, tolerance.min(5.0)))
    });
    let index = same_source.or_else(|| {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| {
                row.source != DecodeSource::Both
                    && row.source != incoming.source
                    && matches(row, row.freq, row.dt, tolerance)
            })
            .min_by(|(_, a), (_, b)| {
                (a.freq - incoming.freq)
                    .abs()
                    .total_cmp(&(b.freq - incoming.freq).abs())
            })
            .map(|(index, _)| index)
    })?;
    let row = &mut rows[index];
    let previous_filterable = row.local_filterable();
    if same_source.is_none() {
        row.paired_external = Some(if row.source == DecodeSource::External {
            (row.freq, row.dt)
        } else {
            (incoming.freq, incoming.dt)
        });
    }
    if incoming.source == DecodeSource::Local && row.source == DecodeSource::External {
        let resolved = row.tokens.clone();
        row.text.clone_from(&incoming.text);
        row.freq = incoming.freq;
        row.dt = incoming.dt;
        row.tokens.clone_from(&incoming.tokens);
        retain_resolved_hash_calls(row, &resolved);
    } else {
        retain_resolved_hash_calls(row, &incoming.tokens);
        if incoming.source == DecodeSource::Local {
            for annotation in incoming
                .text
                .split_whitespace()
                .filter(|t| is_annotation(t))
            {
                if !row.text.split_whitespace().any(|token| token == annotation) {
                    row.text.push(' ');
                    row.text.push_str(annotation);
                }
            }
        }
    }
    if row.source != incoming.source {
        row.source = DecodeSource::Both;
    }
    row.filterable = row.filterable.or(incoming.filterable);
    Some(row.local_filterable() != previous_filterable)
}

/// Enrich only the displayed row, keeping its measurements and provenance.
/// Calls learned from UDP never feed back into a native decoder session.
fn retain_resolved_hash_calls(row: &mut Row, known: &[String]) {
    let mut placeholders = row.text.match_indices("<...>");
    let mut replacements = Vec::new();
    for (token, known) in row.tokens.iter_mut().zip(known) {
        if is_hash_token(token) {
            let position = placeholders.next().map(|(position, _)| position);
            if !is_hash_token(known) {
                token.clone_from(known);
                if let Some(position) = position {
                    replacements.push((position, format!("<{known}>")));
                }
            }
        }
    }
    // Replace from the end so longer calls do not shift earlier positions.
    for (position, call) in replacements.into_iter().rev() {
        row.text.replace_range(position..position + 5, &call);
    }
}

/// Normalize a decode message for merge matching: drop WSJT-X annotations, then
/// uppercase the remaining tokens (the actual message body).
fn norm_tokens(msg: &str) -> Vec<String> {
    message_body_tokens(msg)
        .map(|t| {
            if t == "<...>" {
                t.to_string()
            } else {
                t.trim_matches(['<', '>']).to_ascii_uppercase()
            }
        })
        .collect()
}

/// A 4-character Maidenhead grid locator (`AA00`..`RR99`).
pub(super) fn is_grid4(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
}

/// A report/marker token that is never a callsign: signal reports (`-08`, `R+00`),
/// acks (`RR73`/`RRR`/`73`/`R`), CQ directional/marker words, or a pure-digit
/// directed-CQ frequency like `CQ 072 ...`.
fn is_report_or_marker(s: &str) -> bool {
    matches!(
        s,
        "RR73" | "RRR" | "73" | "R" | "RR" | "TU" | "DX" | "QRZ" | "TEST" | "POTA" | "SOTA"
    ) || s.starts_with(['+', '-'])
        || (s.starts_with('R') && s.get(1..2).is_some_and(|c| c == "+" || c == "-"))
        || s.bytes().all(|b| b.is_ascii_digit())
}

/// Extract a callsign from a token (stripping `<>` hash braces), or `None` if it
/// is a grid, a report/marker, or too short to be a call.
pub(super) fn call_candidate(tok: &str) -> Option<String> {
    let t = tok.trim_matches(['<', '>']);
    if t.len() < 3 || t == "..." || is_grid4(t) || is_report_or_marker(t) {
        return None;
    }
    let has_digit = t.bytes().any(|b| b.is_ascii_digit());
    let has_alpha = t.bytes().any(|b| b.is_ascii_alphabetic());
    (has_digit && has_alpha).then(|| t.to_string())
}

/// The DX callsign and grid to load from a decode's tokens on double-click. For a
/// `CQ` message it is the first real callsign after `CQ` (skipping `DX`/directional
/// markers) and the grid following it; otherwise the message is directed
/// (`TO FROM ...`) and the DX is the sender — the second token — with a grid only
/// if the third token is a grid4.
fn dx_call_and_grid(tokens: &[String]) -> (Option<String>, Option<String>) {
    if tokens.is_empty() {
        return (None, None);
    }
    if tokens[0] == "CQ" {
        for i in 1..tokens.len() {
            if let Some(call) = call_candidate(&tokens[i]) {
                return (Some(call), grid_after(tokens, i + 1));
            }
        }
        (None, None)
    } else {
        let call = tokens.get(1).and_then(|t| call_candidate(t));
        (call, grid_after(tokens, 2))
    }
}

pub(super) fn fill_target_context(call: &mut String, grid: &mut String, tokens: &[String]) {
    let (new_call, new_grid) = dx_call_and_grid(tokens);
    if let Some(new_call) = new_call {
        if norm(call).as_ref() != Some(&new_call) {
            grid.clear();
        }
        *call = new_call;
        if let Some(new_grid) = new_grid {
            *grid = new_grid;
        }
    }
}

/// The token at `idx` as a grid, or `None`. `is_grid4` is purely structural, so
/// `RR73` (the ack) matches it — exclude report/marker tokens here.
fn grid_after(tokens: &[String], idx: usize) -> Option<String> {
    tokens
        .get(idx)
        .filter(|g| is_grid4(g) && !is_report_or_marker(g))
        .map(|g| g.to_string())
}

/// Use the same message body for local/external display and compare matching.
fn clean_message(msg: &str) -> String {
    message_body_tokens(msg).collect::<Vec<_>>().join(" ")
}

fn message_body_tokens(msg: &str) -> impl Iterator<Item = &str> {
    msg.split_whitespace().filter(|t| !is_annotation(t))
}

/// Decoder annotations are separate from the uppercase FT8 payload. Accept a
/// confidence marker adjoining an AP tag as well as whitespace-separated tags.
/// Local provenance normally lives in its own column; also ignore it if a
/// sender includes it in the message tail.
fn is_annotation(token: &str) -> bool {
    let tag = token.trim_matches('?');
    token == "?"
        || matches!(
            tag,
            "sic" | "ctx" | "exp" | "ap" | "dp" | "im" | "p" | "Pc" | "Pr" | "retry"
        )
        || (tag.len() >= 2 && tag.starts_with('a') && tag[1..].bytes().all(|b| b.is_ascii_digit()))
}

/// Whether two normalized messages describe the same transmission. Only an
/// unresolved `<...>` is a wildcard; two different resolved calls never match.
fn tokens_compatible(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x == y || is_hash_token(x) || is_hash_token(y))
}

fn is_hash_token(token: &str) -> bool {
    token == "<...>"
}

#[cfg(test)]
mod tests;
