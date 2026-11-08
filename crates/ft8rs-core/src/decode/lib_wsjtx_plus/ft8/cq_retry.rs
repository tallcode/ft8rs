//! Bounded CQ AP recovery on the Plus retry spectrum.
use super::super::{create_decode_workspace, near_reported, CqSpectrum, FinalRetry};
use super::*;
use crate::decode::lib_wsjtx_plus::osd174_91::osd_decode174_91;
use crate::decode::packjt77::{is_stdcall, unpack77};
use crate::StreamDecodedMessage;
use std::time::Instant;

const MAX_SCREENED: usize = 256;
const MAX_CANDIDATES: usize = 16;
const MIN_SYNC: usize = 7;
const MIN_SNR: f64 = -25.0;
const OFFSETS: [isize; 9] = [0, -1, 1, -2, 2, -3, 3, -4, 4];

pub(in crate::decode::lib_wsjtx_plus::ft8_decode) fn run(
    retry: &FinalRetry,
    previous: Option<&CqSpectrum>,
    sbase: &[f64],
    base: &[StreamDecodedMessage],
    deadline: Option<Instant>,
    mut emit: impl FnMut(Ft8bResult, f64) -> Result<(), String>,
) -> Result<(), String> {
    let within_budget = || deadline.is_none_or(|end| Instant::now() < end);
    let mut workspace = create_decode_workspace();
    let mut fft = SymbolFft::new();
    let mut candidates = Vec::new();
    for candidate in retry
        .peaks
        .iter()
        .filter(|peak| !near_reported(peak, base))
        .take(MAX_SCREENED)
        .take_while(|_| within_budget())
    {
        let (freq, time) = align_candidate(
            &retry.cx_re,
            &retry.cx_im,
            candidate.freq,
            candidate.dt,
            &mut workspace,
        );
        screen_symbols(&mut fft, time, &mut workspace);
        let quality = cq_quality(&workspace);
        if compute_nsync(&workspace.s8) >= MIN_SYNC && quality > 0.0 {
            candidates.push((quality, freq, time, candidate.sync));
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut prior_workspace = previous.map(|_| create_decode_workspace());
    for (_, center, center_time, sync) in candidates.into_iter().take(MAX_CANDIDATES) {
        if !within_budget() {
            break;
        }
        let mut history = None;
        let mut provisional: Option<Ft8bResult> = None;
        'points: for frequency in [center, center - 0.5, center + 0.5] {
            let Some(noise_power) = spectrum_noise_power(sbase, frequency) else {
                continue;
            };
            ft8_downsample(&retry.cx_re, &retry.cx_im, frequency, &mut workspace);
            for offset in OFFSETS.into_iter().take_while(|_| within_budget()) {
                let time = center_time + offset;
                screen_symbols(&mut fft, time, &mut workspace);
                if compute_nsync(&workspace.s8) < MIN_SYNC || cq_quality(&workspace) <= 0.0 {
                    continue;
                }
                fill_symbols(&mut fft, time, &mut workspace, (16..36).chain(43..72));
                if best_possible_snr(&workspace.s8, noise_power) < MIN_SNR {
                    continue;
                }
                let current = current_metric(&workspace);
                let mut result = decode_cq(
                    retry,
                    &mut workspace,
                    noise_power,
                    frequency,
                    time,
                    &current,
                );
                if let Some((previous, prior_workspace)) = previous
                    .zip(prior_workspace.as_mut())
                    .filter(|_| result.is_none())
                {
                    let history = history.get_or_insert_with(|| {
                        previous_magnitudes(
                            previous,
                            center,
                            center_time,
                            prior_workspace,
                            &mut fft,
                            &within_budget,
                        )
                    });
                    let mut current = None;
                    result = history
                        .iter()
                        .take_while(|_| within_budget())
                        .find_map(|prior| {
                            let current =
                                current.get_or_insert_with(|| triple_magnitudes(&workspace));
                            let combined = combined_metric(current, prior);
                            decode_cq(
                                retry,
                                &mut workspace,
                                noise_power,
                                frequency,
                                time,
                                &combined,
                            )
                        });
                }
                let Some(row) = result else { continue };
                if let Some(first) = provisional.take() {
                    if first.itone == row.itone {
                        emit(first, sync)?;
                    }
                    break 'points;
                }
                provisional = Some(row);
            }
        }
    }
    Ok(())
}

fn screen_symbols(fft: &mut SymbolFft, time: isize, workspace: &mut DecodeWorkspace) {
    // Screening reads the three Costas blocks and the nine known CQ symbols.
    fill_symbols(fft, time, workspace, (0..16).chain(36..43).chain(72..79));
}

fn fill_symbols(
    fft: &mut SymbolFft,
    time: isize,
    workspace: &mut DecodeWorkspace,
    symbols: impl Iterator<Item = usize>,
) {
    for symbol in symbols {
        let start = time + (symbol * COSTAS_SYMBOL_LEN) as isize;
        let bins = extract_symbol_spectrum(fft, &workspace.cd0_re, &workspace.cd0_im, start);
        for (tone, bin) in bins.iter().enumerate() {
            let idx = tone * NN + symbol;
            let re = bin.re as f32;
            let im = bin.im as f32;
            workspace.cs_re[idx] = f64::from(re / 1000.0);
            workspace.cs_im[idx] = f64::from(im / 1000.0);
            workspace.s8[idx] = f64::from(wsjtx_cabs(re, im));
        }
    }
}

fn cq_quality(workspace: &DecodeWorkspace) -> f64 {
    // The first nine data symbols are fully specified by the ordinary CQ mask.
    (7..16)
        .map(|symbol| {
            let expected = usize::from(symbol == 15);
            let on = workspace.s8[expected * NN + symbol];
            let average = (0..8)
                .map(|tone| workspace.s8[tone * NN + symbol])
                .sum::<f64>()
                / 8.0;
            (on - average) / (on + average).max(1e-20)
        })
        .sum()
}

fn best_possible_snr(s8: &[f64], noise_power: f64) -> f64 {
    // Every codeword uses the fixed Costas tones. Giving it the strongest
    // tone at each data symbol can only raise the spectrum-based SNR.
    let tones: [u8; NN] = std::array::from_fn(|symbol| {
        let block_offset = symbol % 36;
        if let Some(&tone) = COSTAS.get(block_offset) {
            return tone;
        }
        let mut best = -1.0;
        let mut tone = 0;
        for trial in 0..8 {
            let amplitude = s8[trial * NN + symbol];
            if amplitude > best {
                best = amplitude;
                tone = trial as u8;
            }
        }
        tone
    });
    compute_snr(s8, &tones, noise_power).1
}

type TripleMagnitudes = Vec<[f32; 512]>;

fn previous_magnitudes(
    previous: &CqSpectrum,
    center: f64,
    time: isize,
    workspace: &mut DecodeWorkspace,
    fft: &mut SymbolFft,
    within_budget: &impl Fn() -> bool,
) -> Vec<TripleMagnitudes> {
    // A single Costas maximum in the older slot can align to a neighboring
    // lobe. Rank a small grid using the CQ prefix as well as sync.
    let mut points = Vec::new();
    for freq in [
        center,
        center - 0.5,
        center + 0.5,
        center - 1.0,
        center + 1.0,
    ] {
        if !within_budget() {
            return Vec::new();
        }
        ft8_downsample(&previous.0, &previous.1, freq, workspace);
        for offset in -8..=8 {
            let start = time + offset;
            screen_symbols(fft, start, workspace);
            let score = cq_quality(workspace);
            let nsync = compute_nsync(&workspace.s8);
            if score > 0.0 && nsync >= 5 {
                points.push((score + nsync as f64 * 0.15, freq, start));
            }
        }
    }
    points.sort_by(|a, b| b.0.total_cmp(&a.0));
    points
        .into_iter()
        .take(4)
        .map(|(_, freq, start)| {
            ft8_downsample(&previous.0, &previous.1, freq, workspace);
            fill_symbols(fft, start, workspace, 0..NN);
            triple_magnitudes(workspace)
        })
        .collect()
}

fn current_metric(workspace: &DecodeWorkspace) -> [f64; N_LDPC] {
    let mut soft = [0.0; N_LDPC];
    for half in 0..2 {
        for k in (0..29).step_by(2) {
            let symbol = 7 + half * 36 + k;
            let mut scores: [f32; 64] = std::array::from_fn(|pattern| {
                let a = usize::from(GRAY_MAP[pattern >> 3]) * NN + symbol;
                let b = usize::from(GRAY_MAP[pattern & 7]) * NN + symbol + 1;
                wsjtx_cabs(
                    workspace.cs_re[a] as f32 + workspace.cs_re[b] as f32,
                    workspace.cs_im[a] as f32 + workspace.cs_im[b] as f32,
                )
            });
            let first = half * 87 + 3 * k;
            let len = 6.min(87 - 3 * k);
            write_bit_metrics(&mut scores, &mut soft[first..first + len], 6, -1e30);
        }
    }
    normalize_bmet(&mut soft);
    soft
}

fn triple_magnitudes(workspace: &DecodeWorkspace) -> TripleMagnitudes {
    let phases: [Complex<f32>; 8] = std::array::from_fn(|tone| {
        Complex::from_polar(1.0, std::f32::consts::TAU * tone as f32 / 32.0)
    });
    let spectra: [Complex<f32>; 8 * NN] = std::array::from_fn(|i| {
        phases[i / NN] * Complex::new(workspace.cs_re[i] as f32, -(workspace.cs_im[i] as f32))
    });
    let mut magnitudes = Vec::with_capacity(20);
    for half in 0..2 {
        for k in (0..29).step_by(3) {
            let symbol = 7 + half * 36 + k;
            magnitudes.push(std::array::from_fn(|pattern| {
                let [a, b, c] =
                    [6, 3, 0].map(|shift| usize::from(GRAY_MAP[(pattern >> shift) & 7]));
                (spectra[a * NN + symbol]
                    + spectra[b * NN + symbol + 1]
                    + spectra[c * NN + symbol + 2])
                    .norm()
            }));
        }
    }
    magnitudes
}

fn combined_metric(current: &[[f32; 512]], previous: &[[f32; 512]]) -> [f64; N_LDPC] {
    let mut soft = [0.0; N_LDPC];
    for (group, (current, previous)) in current.iter().zip(previous).enumerate() {
        let mut scores: [f32; 512] =
            std::array::from_fn(|pattern| (current[pattern] + previous[pattern]).powi(3));
        let half = group / 10;
        let k = 9 * (group % 10);
        let first = half * 87 + k;
        let len = 9.min(87 - k);
        write_bit_metrics(
            &mut scores,
            &mut soft[first..first + len],
            9,
            f32::NEG_INFINITY,
        );
    }
    normalize_bmet(&mut soft);
    soft
}

fn write_bit_metrics(scores: &mut [f32], soft: &mut [f64], bits: usize, floor: f32) {
    // After measuring one bit, collapse adjacent patterns that differ only in
    // that bit. Their larger score suffices for all remaining bit maxima.
    for bit in (0..bits).rev() {
        let pairs = 1 << bit;
        let mut zero = floor;
        let mut one = floor;
        for pair in 0..pairs {
            let a = scores[2 * pair];
            let b = scores[2 * pair + 1];
            zero = zero.max(a);
            one = one.max(b);
            scores[pair] = a.max(b);
        }
        if let Some(value) = soft.get_mut(bit) {
            *value = f64::from(one - zero);
        }
    }
}

fn decode_cq(
    retry: &FinalRetry,
    workspace: &mut DecodeWorkspace,
    noise_power: f64,
    freq: f64,
    time: isize,
    soft: &[f64; N_LDPC],
) -> Option<Ft8bResult> {
    for (dst, &llr) in workspace.llr.iter_mut().zip(soft) {
        *dst = f64::from(2.83 * llr as f32);
    }
    let magnitude = workspace
        .llr
        .iter()
        .map(|llr| llr.abs())
        .fold(0.0, f64::max);
    workspace.apmask.fill(0);
    if !apply_wsjt_ap_mask(workspace, &retry.ap_options, 1, magnitude * 1.1, freq) {
        return None;
    }
    let llr: [f32; N_LDPC] = std::array::from_fn(|i| workspace.llr[i] as f32);
    let result = osd_decode174_91(&llr, &workspace.apmask, 3)?;
    let distance: f32 = llr
        .iter()
        .zip(&result.cw)
        .filter(|(v, bit)| (**v >= 0.0) != (**bit != 0))
        .map(|(v, _)| v.abs())
        .sum();
    if result.nharderrors > 36 || result.nharderrors as f32 + distance >= 60.0 {
        return None;
    }
    let msg = unpack77(&result.message91[..77], None)?;
    let mut words = msg.split_whitespace();
    if words.next()? != "CQ" || !is_stdcall(words.next()?) {
        return None;
    }
    let grid = words.next()?.as_bytes();
    if !matches!(grid, [b'A'..=b'R', b'A'..=b'R', b'0'..=b'9', b'0'..=b'9'])
        || words.next().is_some()
    {
        return None;
    }
    let tones = get_tones(&result.cw);
    let (_, snr) = compute_snr(&workspace.s8, &tones, noise_power);
    if snr < MIN_SNR {
        return None;
    }
    Some(Ft8bResult {
        msg,
        ap_type: 1,
        freq,
        dt: (time as f64 - 1.0) * DT2,
        snr,
        itone: std::array::from_fn(|i| i32::from(tones[i])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cq_snr_bound_preserves_a_codeword_near_the_cutoff() {
        let codeword: [u8; N_LDPC] = std::array::from_fn(|i| u8::from(i % 5 < 2));
        let tones = get_tones(&codeword);
        let mut s8 = [0.0; 8 * NN];
        for (symbol, &tone) in tones.iter().enumerate() {
            s8[usize::from(tone) * NN + symbol] = 1000.0;
        }

        let actual_snr = compute_snr(&s8, &tones, 10.0).1;
        assert!((MIN_SNR..MIN_SNR + 0.2).contains(&actual_snr));
        assert_eq!(best_possible_snr(&s8, 10.0), actual_snr);
        assert!(best_possible_snr(&s8, 11.0) < MIN_SNR);
    }
}
