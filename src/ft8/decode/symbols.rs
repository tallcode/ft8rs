use super::{COSTAS_SYMBOL_LEN, NP2};
use crate::util::four2a_c2c;

/// Shared WSJT-X-shaped 32-sample symbol FFT.
pub(crate) fn extract_symbol_spectrum(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i1: isize,
    symb_re: &mut [f64],
    symb_im: &mut [f64],
) {
    debug_assert!(symb_re.len() >= COSTAS_SYMBOL_LEN);
    debug_assert!(symb_im.len() >= COSTAS_SYMBOL_LEN);

    symb_re[..COSTAS_SYMBOL_LEN].fill(0.0);
    symb_im[..COSTAS_SYMBOL_LEN].fill(0.0);

    if i1 >= 0 && (i1 + COSTAS_SYMBOL_LEN as isize - 1) < NP2 as isize {
        let i1 = i1 as usize;
        symb_re[..COSTAS_SYMBOL_LEN].copy_from_slice(&cd0_re[i1..i1 + COSTAS_SYMBOL_LEN]);
        symb_im[..COSTAS_SYMBOL_LEN].copy_from_slice(&cd0_im[i1..i1 + COSTAS_SYMBOL_LEN]);
    }

    four2a_c2c(
        &mut symb_re[..COSTAS_SYMBOL_LEN],
        &mut symb_im[..COSTAS_SYMBOL_LEN],
        -1,
    );
}
