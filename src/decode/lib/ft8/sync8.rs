//! FT8 Costas sync candidate search.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/sync8.f90`

use super::SAMPLE_RATE;
use super::{get_spectrum_baseline, nint_wsjtx_f32, Candidate, COSTAS_BLOCKS, NHSYM, NSPS, NSTEP};
use crate::decode::indexx::indexx_ascending;
use crate::util::{four2a_r2c, sync8_fft_size};

const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];

/// Thread-local reusable buffers for sync8 to avoid allocations.
struct Sync8Buffers {
    s: Vec<f64>,
    savg: Vec<f64>,
    x_re: Vec<f64>,
    x_im: Vec<f64>,
    sync2d: Vec<f64>,
    candidate0: Vec<Candidate>,
}

thread_local! {
    static SYNC8_BUFS: std::cell::RefCell<Sync8Buffers> = const {
        std::cell::RefCell::new(Sync8Buffers {
            s: Vec::new(), savg: Vec::new(), x_re: Vec::new(),
            x_im: Vec::new(), sync2d: Vec::new(), candidate0: Vec::new(),
        })
    };
}

impl Sync8Buffers {
    #[inline]
    fn ensure(&mut self, half_size: usize, width: usize) {
        // s = half_size * NHSYM
        let sz_s = half_size * NHSYM;
        let sz_x = half_size * 2;
        // sync2d can be up to half_size * width (full frequency range)
        let sz_sync2d = width * half_size;
        if self.s.len() < sz_s {
            self.s.resize(sz_s, 0.0);
        }
        if self.savg.len() < half_size {
            self.savg.resize(half_size, 0.0);
        }
        if self.x_re.len() < sz_x {
            self.x_re.resize(sz_x, 0.0);
        }
        if self.x_im.len() < sz_x {
            self.x_im.resize(sz_x, 0.0);
        }
        if self.sync2d.len() < sz_sync2d {
            self.sync2d.resize(sz_sync2d, 0.0);
        }
        // zero buffers
        self.s[..sz_s].fill(0.0);
        self.savg[..half_size].fill(0.0);
    }
}

pub(super) fn sync8(
    dd: &[f64],
    nfa: f64,
    nfb: f64,
    syncmin: f64,
    nfqso: f64,
    maxcand: usize,
) -> (Vec<Candidate>, Vec<f64>) {
    let jz = 62;
    let fft_size = sync8_fft_size();
    let half_size = fft_size / 2;
    let tstep = NSTEP as f64 / SAMPLE_RATE as f64;
    let df = SAMPLE_RATE as f64 / fft_size as f64;
    let fac = 1.0 / 300.0;
    let width = 2 * jz + 1;
    let nssy = NSPS / NSTEP;
    // WSJT-X sync8.f90: `nfos=NFFT1/NSPS`.
    let nfos = fft_size / NSPS;
    // WSJT-X sync8.f90 uses implicit-integer jstrt. Assigning 0.5/tstep
    // therefore truncates 12.5 to 12 rather than rounding to 13.
    let jstrt = (0.5 / tstep) as usize;

    SYNC8_BUFS.with_borrow_mut(|b| {
        b.ensure(half_size, width);
        let s = &mut b.s[..half_size * NHSYM];
        let savg = &mut b.savg[..half_size];
        let x_re = &mut b.x_re[..fft_size];
        let x_im = &mut b.x_im[..fft_size];
        let sync2d = &mut b.sync2d[..];
        let candidate0 = &mut b.candidate0;
        candidate0.clear();
        savg.fill(0.0);

        for j in 0..NHSYM {
            let ia = j * NSTEP;
            x_re[..fft_size].fill(0.0);
            x_im[..fft_size].fill(0.0);
            let end = (ia + NSPS).min(dd.len());
            for i in ia..end {
                x_re[i - ia] = fac * dd[i];
            }
            four2a_r2c(x_re, x_im);
            let row_offset = j;
            for i in 0..half_size {
                let val = x_re[i] * x_re[i] + x_im[i] * x_im[i];
                s[i * NHSYM + row_offset] = val;
                savg[i] += val;
            }
        }

        let sbase = get_spectrum_baseline(dd, nfa, nfb);

        let ia = nint_wsjtx_f32(nfa / df).max(1) as usize;
        let ib = (nint_wsjtx_f32(nfb / df).max(0) as usize).min(half_size - 14);

        // Zero sync2d region we'll use
        let sync2d_len = (ib - ia + 1) * width;
        sync2d[..sync2d_len].fill(0.0);

        // WSJT-X sync8.f90:150: scale spectrum to reference level
        let mut s_max = 0.0;
        for i in 0..half_size {
            for j in 0..NHSYM {
                if s[i * NHSYM + j] > s_max {
                    s_max = s[i * NHSYM + j];
                }
            }
        }
        if s_max > 1e-30 {
            let fac = 20.0 / s_max;
            for v in s.iter_mut() {
                *v *= fac;
            }
            for v in savg.iter_mut() {
                *v *= fac;
            }
        }

        for i in ia..=ib {
            for jj in (-(jz as isize)..=(jz as isize)).step_by(1) {
                let mut ta = 0.0;
                let mut tb = 0.0;
                let mut tc = 0.0;
                let mut t0a = 0.0;
                let mut t0b = 0.0;
                let mut t0c = 0.0;

                for n in 0..COSTAS_BLOCKS {
                    // WSJT-X sync8.f90 keeps `m` as a 1-based time-bin index
                    // into s(:,m). Convert only at the Rust access boundary.
                    let m: isize = jj + jstrt as isize + nssy as isize * n as isize;
                    let m0 = m - 1;
                    let i_costas = i + nfos * COSTAS[n] as usize;

                    if m >= 1 && m <= NHSYM as isize && i_costas < half_size {
                        ta += s[i_costas * NHSYM + m0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0a += s[idx * NHSYM + m0 as usize];
                            }
                        }
                    }

                    let m36 = m + nssy as isize * 36;
                    let m36_0 = m36 - 1;
                    if m36 >= 1 && m36 <= NHSYM as isize && i_costas < half_size {
                        tb += s[i_costas * NHSYM + m36_0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0b += s[idx * NHSYM + m36_0 as usize];
                            }
                        }
                    }

                    let m72 = m + nssy as isize * 72;
                    let m72_0 = m72 - 1;
                    if m72 >= 1 && m72 <= NHSYM as isize && i_costas < half_size {
                        tc += s[i_costas * NHSYM + m72_0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0c += s[idx * NHSYM + m72_0 as usize];
                            }
                        }
                    }
                }

                let t = ta + tb + tc;
                let t0 = (t0a + t0b + t0c - t) / 6.0;
                let sync_val = if t0 > 0.0 { t / t0 } else { 0.0 };

                let tbc = tb + tc;
                let t0bc = (t0b + t0c - tbc) / 6.0;
                let sync_bc = if t0bc > 0.0 { tbc / t0bc } else { 0.0 };

                sync2d[(i - ia) * width + (jj + jz as isize) as usize] = sync_val.max(sync_bc);
            }
        }

        // Red + red2 per frequency bin (matching WSJT-X)
        let mlag: isize = 13;
        let mlag2: isize = jz as isize;
        let iz = ib - ia + 1;
        let mut red = vec![0.0f64; iz];
        let mut red2 = vec![0.0f64; iz];
        let mut jpeak = vec![0isize; iz];
        let mut jpeak2 = vec![0isize; iz];
        for i in ia..=ib {
            let idx = i - ia;
            let mut best = -1.0;
            for j in (-mlag..=mlag).step_by(1) {
                let v = sync2d[idx * width + (j + jz as isize) as usize];
                if v > best {
                    best = v;
                    jpeak[idx] = j;
                }
            }
            red[idx] = best;
            let mut best2 = -1.0;
            for j in (-mlag2..=mlag2).step_by(1) {
                let v = sync2d[idx * width + (j + jz as isize) as usize];
                if v > best2 {
                    best2 = v;
                    jpeak2[idx] = j;
                }
            }
            red2[idx] = best2;
        }

        // WSJT-X style: normalize red by 40th percentile, then normalize red2 similarly.
        // sync8.f90 uses indexx for both percentile selection and candidate ordering.
        let npctile = nint_wsjtx_f32(0.40 * iz as f64).max(1) as usize;
        {
            let indx = indexx_ascending(&red);
            let base = red[indx[npctile - 1]].max(1e-30);
            for v in red.iter_mut() {
                *v /= base;
            }
        }
        {
            let indx2 = indexx_ascending(&red2);
            let base2 = red2[indx2[npctile - 1]].max(1e-30);
            for v in red2.iter_mut() {
                *v /= base2;
            }
        }

        let order = indexx_ascending(&red);
        let maxprecand = 1000usize;
        for &idx in order.iter().rev().take(iz.min(maxprecand)) {
            if candidate0.len() >= maxprecand {
                break;
            }
            if red[idx] >= syncmin && red[idx].is_finite() {
                let i = ia + idx;
                candidate0.push(Candidate {
                    freq: i as f64 * df,
                    dt: (jpeak[idx] as f64 - 0.5) * tstep,
                    sync: red[idx],
                });
            }
            if jpeak2[idx] == jpeak[idx] || candidate0.len() >= maxprecand {
                continue;
            }
            if red2[idx] >= syncmin && red2[idx].is_finite() {
                let i = ia + idx;
                candidate0.push(Candidate {
                    freq: i as f64 * df,
                    dt: (jpeak2[idx] as f64 - 0.5) * tstep,
                    sync: red2[idx],
                });
            }
        }

        let candidates = finalize_sync8_candidates(candidate0, syncmin, nfqso, maxcand);

        (candidates, sbase)
    })
}

pub(super) fn finalize_sync8_candidates(
    candidate0: &mut [Candidate],
    syncmin: f64,
    nfqso: f64,
    maxcand: usize,
) -> Vec<Candidate> {
    for i in 0..candidate0.len() {
        for j in 0..i {
            let fdiff = candidate0[i].freq.abs() - candidate0[j].freq.abs();
            let tdiff = (candidate0[i].dt - candidate0[j].dt).abs();
            // WSJT-X uses single-precision REAL values here. Adjacent 0.04 s
            // time-grid candidates land just outside the strict `tdiff < 0.04`
            // duplicate window; avoid f64 roundoff suppressing that boundary.
            if fdiff.abs() < 4.0 && tdiff < 0.04 - 1e-12 {
                if candidate0[i].sync >= candidate0[j].sync {
                    candidate0[j].sync = 0.0;
                } else {
                    candidate0[i].sync = 0.0;
                }
            }
        }
    }

    let sync_values: Vec<f64> = candidate0.iter().map(|c| c.sync).collect();
    let sorted_idx = indexx_ascending(&sync_values);

    let mut candidates = Vec::with_capacity(maxcand);
    for c in candidate0.iter_mut() {
        if (c.freq - nfqso).abs() <= 10.0 && c.sync >= syncmin {
            candidates.push(c.clone());
            c.sync = 0.0;
            if candidates.len() >= maxcand {
                return candidates;
            }
        }
    }

    for &idx in sorted_idx.iter().rev() {
        let c = &candidate0[idx];
        if c.sync >= syncmin {
            candidates.push(Candidate {
                freq: c.freq.abs(),
                dt: c.dt,
                sync: c.sync,
            });
            if candidates.len() >= maxcand {
                break;
            }
        }
    }
    candidates
}
