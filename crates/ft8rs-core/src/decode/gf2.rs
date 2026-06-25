//! GF(2) row primitives — ft8rs-specific, **not** a mirror of any upstream source.
//!
//! Shared by both decoder mirrors (`lib_jtdx` and `lib_wsjtx`). The OSD decoder
//! spends a large share of decode time
//! XOR-ing length-N GF(2) rows (Gaussian elimination + `mrbencode`). Routing those
//! through a single non-aliasing slice kernel lets each `osd174_91.rs` stay
//! byte-for-byte equivalent while the implementation is swapped underneath.
//!
//! GF(2) addition is exact (integer XOR), so this kernel is **bit-identical** to
//! the inline byte loop it replaces — unlike float SIMD, it keeps the WSJT-X/JTDX
//! baselines byte-identical.
//!
//! Bits are stored one-per-`u8` (values 0/1), matching the OSD representation. The
//! kernel is plain slice XOR; with clean non-aliasing `&mut`/`&` slices LLVM
//! auto-vectorizes it (SSE/AVX2/NEON) with no intrinsics, no `unsafe`, and no
//! runtime feature detection — so it runs identically on every machine.

/// `dst ^= src` over a GF(2) row (one bit per byte). `dst` and `src` must be the
/// same length and must not alias.
#[inline]
pub(crate) fn gf2_row_xor(dst: &mut [u8], src: &[u8]) {
    debug_assert_eq!(dst.len(), src.len());
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d ^= s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_xor_matches_scalar_reference() {
        // Exhaustive-ish small check that gf2_row_xor equals the element-wise
        // reference it replaces, for the kinds of 0/1 rows OSD produces.
        let a0: Vec<u8> = (0..174u32).map(|i| (i % 2) as u8).collect();
        let b: Vec<u8> = (0..174u32).map(|i| ((i / 3) % 2) as u8).collect();

        let mut reference = a0.clone();
        for i in 0..reference.len() {
            reference[i] ^= b[i];
        }

        let mut got = a0.clone();
        gf2_row_xor(&mut got, &b);

        assert_eq!(got, reference);
    }
}
