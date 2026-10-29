//! Mirrors JTDX `lib/tone8myc.f90`.

use super::genft8::genft8;

pub(crate) fn tone8myc(mycall: &str) -> Option<[i32; 58]> {
    let (_, _, itone) = genft8(&format!("{} AA1AAA FN25", mycall.trim()))?;
    let mut idtonemyc = [0i32; 58];
    idtonemyc[..29].copy_from_slice(&itone[7..36]);
    idtonemyc[29..].copy_from_slice(&itone[43..72]);
    Some(idtonemyc)
}
