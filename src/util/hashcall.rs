/// Hash call table for resolving hashed FT8 callsigns.

use std::cell::RefCell;
use crate::util::constants::C38;

const MAGIC: u64 = 47055833459;
const MAX_HASH22_ENTRIES: usize = 1000;

fn ihashcall(c0: &str, m: usize) -> usize {
    let mut n8: u64 = 0;
    let mut count = 0;
    for c in c0.chars() {
        if count >= 11 { break; }
        let uc = c.to_ascii_uppercase();
        let j = C38.iter().position(|&x| x == uc as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    // Pad with spaces
    while count < 11 {
        let j = C38.iter().position(|&x| x == b' ').unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> (64 - m as u32)) & ((1u64 << m as u32) - 1)) as usize
}

pub struct HashCallBook {
    calls10: RefCell<Vec<Option<String>>>,
    calls12: RefCell<Vec<Option<String>>>,
    hash22_entries: RefCell<Vec<(usize, String)>>,
}

impl Default for HashCallBook {
    fn default() -> Self {
        Self::new()
    }
}

impl HashCallBook {
    pub fn new() -> Self {
        HashCallBook {
            calls10: RefCell::new(vec![None; 1024]),
            calls12: RefCell::new(vec![None; 4096]),
            hash22_entries: RefCell::new(Vec::new()),
        }
    }

    pub fn save(&self, callsign: &str) {
        let trimmed = callsign.trim();
        if trimmed.is_empty() || trimmed == "<...>" {
            return;
        }
        
        // Strip < > if present
        let clean = if trimmed.starts_with('<') && trimmed.ends_with('>') {
            &trimmed[1..trimmed.len()-1]
        } else if trimmed.starts_with('<') {
            if let Some(gt) = trimmed.find('>') {
                &trimmed[1..gt]
            } else {
                &trimmed[1..]
            }
        } else {
            trimmed
        };
        
        if clean.len() < 3 {
            return;
        }
        
        let cw = clean.to_uppercase();

        let n10 = ihashcall(&cw, 10);
        if n10 <= 1023 {
            self.calls10.borrow_mut()[n10] = Some(cw.clone());
        }

        let n12 = ihashcall(&cw, 12);
        if n12 <= 4095 {
            self.calls12.borrow_mut()[n12] = Some(cw.clone());
        }

        let n22 = ihashcall(&cw, 22);
        let mut entries = self.hash22_entries.borrow_mut();
        if let Some(pos) = entries.iter().position(|(h, _)| *h == n22) {
            entries[pos].1 = cw;
        } else {
            if entries.len() >= MAX_HASH22_ENTRIES {
                entries.pop();
            }
            entries.insert(0, (n22, cw));
        }
    }

    pub fn lookup10(&self, n10: usize) -> Option<String> {
        if n10 <= 1023 {
            self.calls10.borrow()[n10].clone()
        } else {
            None
        }
    }

    pub fn lookup12(&self, n12: usize) -> Option<String> {
        if n12 <= 4095 {
            self.calls12.borrow()[n12].clone()
        } else {
            None
        }
    }

    pub fn lookup22(&self, n22: usize) -> Option<String> {
        self.hash22_entries
            .borrow()
            .iter()
            .find(|(h, _)| *h == n22)
            .map(|(_, c)| c.clone())
    }

    pub fn size(&self) -> usize {
        self.hash22_entries.borrow().len()
    }

    pub fn clear(&self) {
        self.calls10.borrow_mut().iter_mut().for_each(|c| *c = None);
        self.calls12.borrow_mut().iter_mut().for_each(|c| *c = None);
        self.hash22_entries.borrow_mut().clear();
    }

    pub fn get_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = Vec::new();
        for entry in self.hash22_entries.borrow().iter() {
            if !calls.contains(&entry.1) {
                calls.push(entry.1.clone());
            }
        }
        calls
    }


    pub fn clone_book(&self) -> HashCallBook {
        HashCallBook {
            calls10: RefCell::new(self.calls10.borrow().clone()),
            calls12: RefCell::new(self.calls12.borrow().clone()),
            hash22_entries: RefCell::new(self.hash22_entries.borrow().clone()),
        }
    }
}
