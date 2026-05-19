/// FFT utilities: hand-coded Radix-2 Cooley-Tukey (matching ft8ts) + Bluestein's for non-power-of-2.

/// Next power of 2 >= n
pub fn next_pow2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    1 << (usize::BITS - n.leading_zeros())
}

// ── Thread-local FFT cache (avoids borrow-checker issues with static Mutex) ──

thread_local! {
    static RADIX2_CACHE: std::cell::RefCell<std::collections::HashMap<usize, Vec<u32>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static BLUESTEIN_CACHE: std::cell::RefCell<std::collections::HashMap<String, BluesteinPlan>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

// ── Radix-2 plan ─────────────────────────────────────────────────────

fn get_radix2_plan(n: usize) -> Vec<u32> {
    RADIX2_CACHE.with_borrow(|cache| cache.get(&n).cloned()).unwrap_or_else(|| {
        let bits = (usize::BITS - n.leading_zeros()) as usize - 1;
        let mut bit_reversed = vec![0u32; n];
        for i in 1..n {
            bit_reversed[i] = (bit_reversed[i >> 1] >> 1) | (((i & 1) as u32) << (bits as u32 - 1));
        }
        RADIX2_CACHE.with_borrow_mut(|cache| {
            cache.insert(n, bit_reversed.clone());
        });
        bit_reversed
    })
}

// ── Bluestein plan (immutable parts cached, a_re/a_im allocated per-call) ──

#[derive(Clone)]
struct BluesteinPlan {
    chirp_re: Vec<f64>,
    chirp_im: Vec<f64>,
    b_fft_re: Vec<f64>,
    b_fft_im: Vec<f64>,
    m: usize,
}

fn get_bluestein_plan(n: usize, inverse: bool) -> Option<BluesteinPlan> {
    let key = format!("{}:{}", n, inverse as u8);
    if let Some(plan) = BLUESTEIN_CACHE.with_borrow(|cache| cache.get(&key).cloned()) {
        return Some(plan);
    }
    let plan = make_bluestein_plan(n, inverse);
    BLUESTEIN_CACHE.with_borrow_mut(|cache| {
        cache.insert(key, plan.clone());
    });
    Some(plan)
}

fn make_bluestein_plan(n: usize, inverse: bool) -> BluesteinPlan {
    let m = next_pow2(2 * n - 1);
    let s: f64 = if inverse { 1.0 } else { -1.0 };

    let mut chirp_re = vec![0.0f64; n];
    let mut chirp_im = vec![0.0f64; n];
    for i in 0..n {
        let angle = s * std::f64::consts::PI * ((i * i) % (2 * n)) as f64 / n as f64;
        chirp_re[i] = angle.cos();
        chirp_im[i] = angle.sin();
    }

    let mut b_fft_re = vec![0.0f64; m];
    let mut b_fft_im = vec![0.0f64; m];
    for i in 0..n {
        b_fft_re[i] = chirp_re[i];
        b_fft_im[i] = -chirp_im[i];
    }
    for i in 1..n {
        b_fft_re[m - i] = b_fft_re[i];
        b_fft_im[m - i] = b_fft_im[i];
    }
    // Use raw FFT during plan construction (m is power of 2 for Bluestein)
    radix2_fft_raw(&mut b_fft_re, &mut b_fft_im, false);

    BluesteinPlan {
        m,
        chirp_re,
        chirp_im,
        b_fft_re,
        b_fft_im,
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Complex FFT (in-place). Uses Radix-2 Cooley-Tukey for power-of-2, Bluestein for others.
pub fn fft_complex(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    if n <= 1 {
        return;
    }

    if (n & (n - 1)) == 0 {
        radix2_fft_cached(re, im, inverse);
    } else if let Some((p, q)) = factor_for_mixed_radix(n) {
        mixed_radix_fft(re, im, inverse, p, q);
    } else {
        bluestein_fft_cached(re, im, inverse);
    }
}

// ── Mixed-radix FFT for N = P × Q (Q = power of 2, P ≤ 50) ────────

/// Factor N as P × Q where Q is the largest power of 2 dividing N.
/// Returns Some((p, q)) if p ≤ 50 (small enough for direct DFT).
fn factor_for_mixed_radix(n: usize) -> Option<(usize, usize)> {
    // Find largest power of 2 dividing n
    let q = 1 << n.trailing_zeros();
    let p = n / q;
    if p <= 1 || p > 50 {
        return None;
    }
    Some((p, q))
}

/// Cooley-Tukey mixed-radix FFT: N = P × Q, Q is power of 2.
/// DIT algorithm: groups input by residue mod P, natural-order output.
fn mixed_radix_fft(re: &mut [f64], im: &mut [f64], inverse: bool, p: usize, q: usize) {
    let n = re.len();
    assert_eq!(n, p * q);

    let two_pi = 2.0 * std::f64::consts::PI;
    let sign = if inverse { 1.0 } else { -1.0 };

    // Allocate scratch for reordering
    let mut scratch_re = vec![0.0; n];
    let mut scratch_im = vec![0.0; n];

    // Step 0: Reorder input by residue mod P (DIT input layout).
    // x[p + P·q] → block p, position q (stride P grouping)
    // Original: natural order re[0..N-1]
    // Reordered: block 0 = re[0], re[P], re[2P], ...; block 1 = re[1], re[P+1], re[2P+1], ...
    for p_idx in 0..p {
        for q_idx in 0..q {
            let src = p_idx + p * q_idx;     // natural order index
            let dst = p_idx * q + q_idx;     // DIT order: block p_idx, pos q_idx
            scratch_re[dst] = re[src];
            scratch_im[dst] = im[src];
        }
    }

    // Step 1: Q-point radix-2 FFT on each of the P blocks
    for block in 0..p {
        let start = block * q;
        radix2_fft_cached(&mut scratch_re[start..start + q], &mut scratch_im[start..start + q], inverse);
    }

    // Step 2: Multiply by twiddle factors exp(sign·2πi·p_idx·q_idx/N)
    for p_idx in 0..p {
        for q_idx in 0..q {
            let idx = p_idx * q + q_idx;
            let angle = sign * two_pi * (p_idx as f64) * (q_idx as f64) / (n as f64);
            let (tw_re, tw_im) = (angle.cos(), angle.sin());
            let val_re = scratch_re[idx];
            let val_im = scratch_im[idx];
            scratch_re[idx] = val_re * tw_re - val_im * tw_im;
            scratch_im[idx] = val_re * tw_im + val_im * tw_re;
        }
    }

    // Step 3: Allocate second scratch, transpose, do P-point DFTs
    let mut scratch2_re = vec![0.0; n];
    let mut scratch2_im = vec![0.0; n];

    // Transpose: P×Q → Q×P
    for i in 0..p {
        for j in 0..q {
            let src = i * q + j;
            let dst = j * p + i;
            scratch2_re[dst] = scratch_re[src];
            scratch2_im[dst] = scratch_im[src];
        }
    }

    // Step 4: P-point direct DFT on each column (Q columns of P elements)
    for col in 0..q {
        let start = col * p;
        for k in 0..p {
            let mut sum_re = 0.0;
            let mut sum_im = 0.0;
            for j in 0..p {
                let angle = sign * two_pi * (j as f64) * (k as f64) / (p as f64);
                let (tw_re, tw_im) = (angle.cos(), angle.sin());
                let val_re = scratch2_re[start + j];
                let val_im = scratch2_im[start + j];
                sum_re += val_re * tw_re - val_im * tw_im;
                sum_im += val_re * tw_im + val_im * tw_re;
            }
            // Output in natural order: k₁ + Q·k₂ → col + q·k
            let dst = col + q * k;
            re[dst] = sum_re;
            im[dst] = sum_im;
        }
    }

    // Radix-2 applies 1/Q for inverse in step 1.
    // Direct DFT in step 4 needs 1/P to reach 1/N = 1/(P×Q) total.
    if inverse {
        let scale = 1.0 / (p as f64);
        for i in 0..n {
            re[i] *= scale;
            im[i] *= scale;
        }
    }
}

// ── Radix-2 Cooley-Tukey ───────────────────────────────────────────

fn radix2_fft_raw(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    if n <= 1 {
        return;
    }

    let bits = (usize::BITS - n.leading_zeros()) - 1;
    for i in 1..n {
        let j = bit_reverse(i as u32, bits) as usize;
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let sign = if inverse { 1.0 } else { -1.0 };
    let mut size = 2usize;
    while size <= n {
        let halfsize = size >> 1;
        let step = sign * std::f64::consts::PI / halfsize as f64;
        let w_re = step.cos();
        let w_im = step.sin();

        let mut i = 0;
        while i < n {
            let mut cur_re = 1.0;
            let mut cur_im = 0.0;
            for k in 0..halfsize {
                let even_idx = i + k;
                let odd_idx = i + k + halfsize;
                let t_re = cur_re * re[odd_idx] - cur_im * im[odd_idx];
                let t_im = cur_re * im[odd_idx] + cur_im * re[odd_idx];
                re[odd_idx] = re[even_idx] - t_re;
                im[odd_idx] = im[even_idx] - t_im;
                re[even_idx] += t_re;
                im[even_idx] += t_im;
                let new_cur_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = new_cur_re;
            }
            i += size;
        }
        size <<= 1;
    }

    if inverse {
        let scale = 1.0 / n as f64;
        for i in 0..n {
            re[i] *= scale;
            im[i] *= scale;
        }
    }
}

fn radix2_fft_cached(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    if n <= 1 {
        return;
    }

    let bit_reversed = get_radix2_plan(n);
    for i in 0..n {
        let j = bit_reversed[i] as usize;
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let sign = if inverse { 1.0 } else { -1.0 };
    let mut size = 2usize;
    while size <= n {
        let halfsize = size >> 1;
        let step = sign * std::f64::consts::PI / halfsize as f64;
        let w_re = step.cos();
        let w_im = step.sin();

        let mut i = 0;
        while i < n {
            let mut cur_re = 1.0;
            let mut cur_im = 0.0;
            for k in 0..halfsize {
                let even_idx = i + k;
                let odd_idx = i + k + halfsize;
                let t_re = cur_re * re[odd_idx] - cur_im * im[odd_idx];
                let t_im = cur_re * im[odd_idx] + cur_im * re[odd_idx];
                re[odd_idx] = re[even_idx] - t_re;
                im[odd_idx] = im[even_idx] - t_im;
                re[even_idx] += t_re;
                im[even_idx] += t_im;
                let new_cur_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = new_cur_re;
            }
            i += size;
        }
        size <<= 1;
    }

    if inverse {
        let scale = 1.0 / n as f64;
        for i in 0..n {
            re[i] *= scale;
            im[i] *= scale;
        }
    }
}

fn bit_reverse(mut x: u32, bits: u32) -> u32 {
    let mut result = 0;
    for _ in 0..bits {
        result = (result << 1) | (x & 1);
        x >>= 1;
    }
    result
}

// ── Bluestein (cached) ─────────────────────────────────────────────

fn bluestein_fft_cached(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let plan = get_bluestein_plan(n, inverse);
    if plan.is_none() {
        // Fallback: do it without caching
        bluestein_raw(re, im, inverse);
        return;
    }
    let plan = plan.unwrap();
    let m = plan.m;

    // Allocate working arrays per-call (a_re/a_im are mutable scratch)
    let mut a_re = vec![0.0f64; m];
    let mut a_im = vec![0.0f64; m];

    for i in 0..n {
        let cos_a = plan.chirp_re[i];
        let sin_a = plan.chirp_im[i];
        a_re[i] = re[i] * cos_a - im[i] * sin_a;
        a_im[i] = re[i] * sin_a + im[i] * cos_a;
    }

    radix2_fft_raw(&mut a_re, &mut a_im, false);

    for i in 0..m {
        let ar = a_re[i];
        let ai = a_im[i];
        a_re[i] = ar * plan.b_fft_re[i] - ai * plan.b_fft_im[i];
        a_im[i] = ar * plan.b_fft_im[i] + ai * plan.b_fft_re[i];
    }

    radix2_fft_raw(&mut a_re, &mut a_im, true);

    let scale = if inverse { 1.0 / n as f64 } else { 1.0 };
    for i in 0..n {
        re[i] = (a_re[i] * plan.chirp_re[i] - a_im[i] * plan.chirp_im[i]) * scale;
        im[i] = (a_re[i] * plan.chirp_im[i] + a_im[i] * plan.chirp_re[i]) * scale;
    }
}

fn bluestein_raw(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let m = next_pow2(2 * n - 1);
    let s: f64 = if inverse { 1.0 } else { -1.0 };

    let mut chirp_re = vec![0.0f64; n];
    let mut chirp_im = vec![0.0f64; n];
    for i in 0..n {
        let angle = s * std::f64::consts::PI * ((i * i) % (2 * n)) as f64 / n as f64;
        chirp_re[i] = angle.cos();
        chirp_im[i] = angle.sin();
    }

    let mut b_re = vec![0.0f64; m];
    let mut b_im = vec![0.0f64; m];
    for i in 0..n {
        b_re[i] = chirp_re[i];
        b_im[i] = -chirp_im[i];
    }
    for i in 1..n {
        b_re[m - i] = b_re[i];
        b_im[m - i] = b_im[i];
    }
    radix2_fft_raw(&mut b_re, &mut b_im, false);

    let mut a_re = vec![0.0f64; m];
    let mut a_im = vec![0.0f64; m];
    for i in 0..n {
        a_re[i] = re[i] * chirp_re[i] - im[i] * chirp_im[i];
        a_im[i] = re[i] * chirp_im[i] + im[i] * chirp_re[i];
    }

    radix2_fft_raw(&mut a_re, &mut a_im, false);

    for i in 0..m {
        let ar = a_re[i];
        let ai = a_im[i];
        a_re[i] = ar * b_re[i] - ai * b_im[i];
        a_im[i] = ar * b_im[i] + ai * b_re[i];
    }

    radix2_fft_raw(&mut a_re, &mut a_im, true);

    let scale = if inverse { 1.0 / n as f64 } else { 1.0 };
    for i in 0..n {
        re[i] = (a_re[i] * chirp_re[i] - a_im[i] * chirp_im[i]) * scale;
        im[i] = (a_re[i] * chirp_im[i] + a_im[i] * chirp_re[i]) * scale;
    }
}

/// Real-to-complex FFT. Input: n real values. Output: n/2+1 complex values.
pub fn fft_real(input: &[f64], out_re: &mut [f64], out_im: &mut [f64]) {
    let n = input.len();
    let half = n >> 1;

    let mut re = vec![0.0; half];
    let mut im = vec![0.0; half];
    for i in 0..half {
        re[i] = input[i * 2];
        im[i] = input[i * 2 + 1];
    }
    fft_complex(&mut re, &mut im, false);

    out_re[0] = re[0] + im[0];
    out_im[0] = 0.0;
    out_re[half] = re[0] - im[0];
    out_im[half] = 0.0;

    for k in 1..half {
        let nk = half - k;
        let e_re = 0.5 * (re[k] + re[nk]);
        let e_im = 0.5 * (im[k] - im[nk]);
        let angle = (-2.0 * std::f64::consts::PI * k as f64) / n as f64;
        let tw_re = angle.cos();
        let tw_im = angle.sin();
        let o_re = 0.5 * (im[k] + im[nk]);
        let o_im = -0.5 * (re[k] - re[nk]);
        let to_re = tw_re * o_re - tw_im * o_im;
        let to_im = tw_re * o_im + tw_im * o_re;
        out_re[k] = e_re + to_re;
        out_im[k] = e_im + to_im;
        out_re[n - k] = e_re - to_re;
        out_im[n - k] = -(e_im - to_im);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mixed_radix_3840() {
        let n = 3840;
        let (p, q) = factor_for_mixed_radix(n).unwrap();
        assert_eq!(p, 15);
        assert_eq!(q, 256);

        // Generate a test signal: impulse
        let mut re_mr = vec![0.0; n];
        let mut im_mr = vec![0.0; n];
        re_mr[100] = 1.0;
        let mut re_bl = re_mr.clone();
        let mut im_bl = im_mr.clone();

        // Mixed-radix forward
        mixed_radix_fft(&mut re_mr, &mut im_mr, false, p, q);
        // Bluestein forward
        bluestein_fft_cached(&mut re_bl, &mut im_bl, false);

        // Compare: should be very close (within 1e-10)
        for i in 0..n {
            let diff_re = (re_mr[i] - re_bl[i]).abs();
            let diff_im = (im_mr[i] - im_bl[i]).abs();
            assert!(diff_re < 1e-10, "re[{}]: mr={}, bl={}", i, re_mr[i], re_bl[i]);
            assert!(diff_im < 1e-10, "im[{}]: mr={}, bl={}", i, im_mr[i], im_bl[i]);
        }

        // Test roundtrip: FFT → IFFT → original
        let mut re_rt = vec![0.0; n];
        let mut im_rt = vec![0.0; n];
        re_rt[100] = 1.0;
        mixed_radix_fft(&mut re_rt, &mut im_rt, false, p, q); // forward
        mixed_radix_fft(&mut re_rt, &mut im_rt, true, p, q);  // inverse

        for i in 0..n {
            let expected = if i == 100 { 1.0 } else { 0.0 };
            assert!((re_rt[i] - expected).abs() < 1e-10, "roundtrip re[{}]: {}", i, re_rt[i]);
            assert!(im_rt[i].abs() < 1e-10, "roundtrip im[{}]: {}", i, im_rt[i]);
        }
    }

    #[test]
    fn test_mixed_radix_3200() {
        let n = 3200;
        let (p, q) = factor_for_mixed_radix(n).unwrap();
        assert_eq!(p, 25);
        assert_eq!(q, 128);

        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[50] = 1.0;

        // Forward then inverse
        mixed_radix_fft(&mut re, &mut im, false, p, q);
        mixed_radix_fft(&mut re, &mut im, true, p, q);

        for i in 0..n {
            let expected = if i == 50 { 1.0 } else { 0.0 };
            assert!((re[i] - expected).abs() < 1e-10, "roundtrip re[{}]: {}", i, re[i]);
            assert!(im[i].abs() < 1e-10);
        }
    }
}

    #[test]
    fn test_mixed_radix_small() {
        let n = 12; // 3 × 4
        let (p, q) = factor_for_mixed_radix(n).unwrap();
        assert_eq!(p, 3);
        assert_eq!(q, 4);
        
        let mut re_mr = vec![0.0; n];
        let mut im_mr = vec![0.0; n];
        re_mr[1] = 1.0;
        let mut re_bl = re_mr.clone();
        let mut im_bl = im_mr.clone();
        
        mixed_radix_fft(&mut re_mr, &mut im_mr, false, p, q);
        bluestein_fft_cached(&mut re_bl, &mut im_bl, false);
        
        for i in 0..n {
            let diff_re = (re_mr[i] - re_bl[i]).abs();
            let diff_im = (im_mr[i] - im_bl[i]).abs();
            assert!(diff_re < 1e-10, "re[{}]: mr={}, bl={}", i, re_mr[i], re_bl[i]);
            assert!(diff_im < 1e-10, "im[{}]: mr={}, bl={}", i, im_mr[i], im_bl[i]);
        }
    }
