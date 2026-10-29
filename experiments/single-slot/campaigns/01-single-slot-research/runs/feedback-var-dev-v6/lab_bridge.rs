// Unchanged native functions exposed only by the isolated harness.
pub fn pack(s: &str) -> Vec<u8> { crate::decode::packjt77::pack77(s) }
pub fn unpack(b: &[u8], book: Option<&crate::HashCallBook>) -> Option<String> { crate::decode::packjt77::unpack77(b, book) }
pub fn tones(b: &[u8]) -> [i32;79] { crate::decode::genft8::get_ft8_tones_from_77bits(b) }
pub fn wave(t: &[i32;79], f: f64) -> (Vec<f64>, Vec<f64>) { crate::decode::gen_ft8wave::gen_ft8wave(t, f) }
pub fn fec(l: &[f64], a: &[i8], m: isize) -> Option<crate::decode::decode174_91::DecodeResult> { crate::decode::decode174_91::decode174_91(l,a,m) }
pub fn baseline(a: &[f64], o: crate::DecodeOptions) -> Vec<crate::DecodedMessage> { crate::decode::ft8_decode::decode_f64(a,o) }
pub fn subtract(a: &mut Vec<f64>, t: &[i32;79], f:f64, dt:f64, refine:bool) { crate::decode::subtractft8::subtract_ft8_refined(a,t,f,dt,refine) }
pub fn parity(i:usize) -> &'static [usize] { crate::decode::ldpc_174_91_c_parity::nm(i) }
