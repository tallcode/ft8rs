// Measurement-only protocol helpers; original routines unchanged.
pub fn pack(s: &str) -> Vec<u8> { crate::decode::packjt77::pack77(s) }
pub fn unpack(b: &[u8], book: Option<&crate::HashCallBook>) -> Option<String> { crate::decode::packjt77::unpack77(b, book) }
pub fn tones(b: &[u8]) -> [i32;79] { crate::decode::genft8::get_ft8_tones_from_77bits(b) }
pub fn wave(t: &[i32;79], f:f64) -> (Vec<f64>,Vec<f64>) { crate::decode::gen_ft8wave::gen_ft8wave(t,f) }
