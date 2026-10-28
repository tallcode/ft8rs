//! Test-only adapters: no production visibility changes or modified mirrors.
use crate::decode::{self, lib_jtdx::ft8v2 as jtdx};

#[derive(Clone, Copy, Debug)]
pub enum Backend {
    Wsjtx,
    Jtdx,
}

pub const BACKENDS: [Backend; 2] = [Backend::Wsjtx, Backend::Jtdx];

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wsjtx => "wsjtx",
            Self::Jtdx => "jtdx",
        }
    }

    pub fn pack(self, message: &str) -> Vec<u8> {
        match self {
            Self::Wsjtx => decode::packjt77::pack77(message),
            Self::Jtdx => jtdx::packjt77::pack77(message),
        }
    }

    pub fn unpack(self, bits: &[u8], calls: &[&str]) -> Option<String> {
        match self {
            Self::Wsjtx => {
                let book = decode::packjt77::HashCallBook::new();
                for call in calls {
                    book.save(call);
                }
                decode::packjt77::unpack77(bits, Some(&book))
            }
            Self::Jtdx => {
                let book = jtdx::packjt77::HashCallBook::new();
                for call in calls {
                    book.save(call);
                }
                jtdx::packjt77::unpack77(bits, Some(&book))
            }
        }
    }

    pub fn encode(self, bits: &[u8]) -> Vec<u8> {
        match self {
            Self::Wsjtx => decode::encode174_91::encode174_91(bits),
            Self::Jtdx => jtdx::encode174_91::encode174_91(bits).to_vec(),
        }
    }

    pub fn crc_valid(self, bits: &[u8]) -> bool {
        match self {
            Self::Wsjtx => decode::chkcrc14a::check_crc14(bits),
            Self::Jtdx => jtdx::chkcrc14a::chkcrc14a(bits),
        }
    }

    pub fn bp(self, llr: &[f64; 174], mask: &[i8; 174]) -> Option<Vec<u8>> {
        match self {
            Self::Wsjtx => decode::decode174_91::decode174_91(llr, mask, -1).map(|r| r.cw),
            Self::Jtdx => jtdx::bpdecode174_91::bpdecode174_91(&llr.map(|x| x as f32), mask, 30)
                .map(|r| r.cw.to_vec()),
        }
    }

    pub fn osd(self, llr: &[f64; 174], mask: &[i8; 174]) -> Option<Vec<u8>> {
        let llr = llr.map(|x| x as f32);
        match self {
            Self::Wsjtx => decode::osd174_91::osd_decode174_91(&llr, mask, 2).map(|r| r.cw),
            // The native JTDX API returns its candidate even on CRC failure;
            // negative nharderror is the caller's rejection flag.
            Self::Jtdx => jtdx::osd174_91::osd174_91(&llr, mask, 2)
                .filter(|r| r.nharderror >= 0)
                .map(|r| r.cw.to_vec()),
        }
    }

    pub fn hash_matches(self, call: &str, hashes: [usize; 3]) -> bool {
        match self {
            Self::Wsjtx => [10, 12, 22]
                .into_iter()
                .zip(hashes)
                .all(|(width, expected)| decode::packjt77::ihashcall(call, width) == expected),
            Self::Jtdx => {
                // JTDX intentionally keeps ihashcall private. Verify all widths
                // through the real receive hash book, using a fresh book per case.
                let book = jtdx::packjt77::HashCallBook::new();
                book.save(call);
                if call.trim().is_empty() {
                    book.size() == 0 && hashes == [0; 3]
                } else {
                    [
                        book.lookup10(hashes[0]),
                        book.lookup12(hashes[1]),
                        book.lookup22(hashes[2]),
                    ]
                    .iter()
                    .all(|value| value.as_deref() == Some(call))
                }
            }
        }
    }
}
