/// Hash call table for resolving hashed FT8 callsigns.

use std::cell::RefCell;
use crate::util::constants::C38;

const MAGIC: u64 = 47055833459;
const MAX_HASH22_ENTRIES: usize = 1000;

fn ihashcall(c0: &str, m: usize) -> usize {
    let s = format!("{:<11}", c0).to_uppercase();
    let mut n8: u64 = 0;
    for c in s.chars().take(11) {
        let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
    }
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> (64 - m as u32)) & ((1u64 << m as u32) - 1)) as usize
}

pub struct HashCallBook {
    calls10: RefCell<Vec<Option<String>>>,
    calls12: RefCell<Vec<Option<String>>>,
    hash22_entries: RefCell<Vec<(usize, String)>>,
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
        let mut cw = callsign.trim().to_uppercase();
        if cw.is_empty() || cw == "<...>" {
            return;
        }
        if cw.starts_with('<') {
            cw = cw[1..].to_string();
        }
        if let Some(gt) = cw.find('>') {
            cw = cw[..gt].to_string();
        }
        cw = cw.trim().to_string();
        if cw.len() < 3 {
            return;
        }

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
}
