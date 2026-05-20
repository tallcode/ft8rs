/// Cross-slot memory for streaming FT8 decode.
/// Matches WSJT-X ndec_early and ndec(jseq,0/1) arrays.
use std::collections::HashSet;

/// A saved decode result from a slot.
#[derive(Clone, Debug)]
pub struct SavedDecode {
    pub freq: f64,
    pub dt: f64,
    pub msg: String,
    pub itone: [i32; 79],
    pub snr: f64,
    pub sync: f64,
    pub subtracted: bool,
}

/// Cross-slot memory that tracks decodes across audio windows.
pub struct CrossSlotMemory {
    /// Early decodes accumulated during current slot (nzhsym < 50).
    early_decodes: Vec<SavedDecode>,
    /// Previous slot decodes (used for AP in ft8_a7d).
    previous_slot: Vec<SavedDecode>,
    /// Dedup tracking for current slot.
    seen_msgs: HashSet<String>,
}

impl CrossSlotMemory {
    pub fn new() -> Self {
        Self {
            early_decodes: Vec::new(),
            previous_slot: Vec::new(),
            seen_msgs: HashSet::new(),
        }
    }

    /// Save a new decode result. Returns false if duplicate.
    pub fn save(&mut self, decode: SavedDecode) -> bool {
        let key = normalize_message(&decode.msg);
        if self.seen_msgs.contains(&key) {
            return false;
        }
        self.seen_msgs.insert(key);
        self.early_decodes.push(decode);
        true
    }

    /// Get all early decodes (for subtraction at nzhsym=47).
    pub fn get_early_decodes(&self) -> &[SavedDecode] {
        &self.early_decodes
    }

    /// Get unsubtracted decodes (for subtraction pass).
    pub fn get_unsubtracted(&self) -> Vec<&SavedDecode> {
        self.early_decodes.iter().filter(|d| !d.subtracted).collect()
    }

    /// Mark a decode as subtracted.
    pub fn mark_subtracted(&mut self, freq: f64, dt: f64) {
        for d in &mut self.early_decodes {
            if (d.freq - freq).abs() < 1.0 && (d.dt - dt).abs() < 0.1 {
                d.subtracted = true;
                break;
            }
        }
    }

    /// Previous slot decodes (for AP).
    pub fn get_previous_slot(&self) -> &[SavedDecode] {
        &self.previous_slot
    }

    /// Combine all decoded messages from current slot.
    pub fn get_all_messages(&self) -> Vec<&SavedDecode> {
        self.early_decodes.iter().collect()
    }

    /// Rotate: current slot → previous slot, clear current.
    /// Called at end of 15s window (nzhsym=50 processing complete).
    pub fn rotate_slot(&mut self) {
        self.previous_slot = std::mem::take(&mut self.early_decodes);
        self.seen_msgs.clear();
    }

    /// Reset completely (new transmission).
    pub fn reset(&mut self) {
        self.early_decodes.clear();
        self.previous_slot.clear();
        self.seen_msgs.clear();
    }

    /// Number of decodes in current slot.
    pub fn count(&self) -> usize {
        self.early_decodes.len()
    }

    /// Number of decodes in previous slot.
    pub fn previous_count(&self) -> usize {
        self.previous_slot.len()
    }
}

fn normalize_message(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.trim().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}
