/// FT4 decoder – Rust port of ft4/decode.ts

use crate::ft4::constants::GRAYMAP;
use crate::ft4::scramble::xor_with_scrambler;
use crate::util::constants::SAMPLE_RATE;
use crate::util::decode174_91::decode174_91;
use crate::util::fft::fft_complex;
use crate::util::hashcall::HashCallBook;
use crate::util::unpack_jt77::unpack77;

const COSTAS_A: [u8; 4] = [0, 1, 3, 2];
const COSTAS_B: [u8; 4] = [1, 0, 2, 3];
const COSTAS_C: [u8; 4] = [2, 3, 1, 0];
const COSTAS_D: [u8; 4] = [3, 2, 0, 1];
const NSPS: usize = 576;
const NFFT1: usize = 4 * NSPS;
const NH1: usize = NFFT1 / 2;
const NMAX: usize = 21 * 3456;
const NHSYM: usize = (NMAX - NFFT1) / NSPS;
const NDOWN: usize = 18;
const NN: usize = 103;
const NFFT2: usize = NMAX / NDOWN;
const NSS: usize = NSPS / NDOWN;
const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
const COSTAS_BLOCKS: usize = 4;
const FT4_SYNC_STRIDE: usize = 33 * NSS;
const FT4_MAX_TWEAK: isize = 16;
const LDPC_BITS: usize = 174;
const BITMETRIC_LEN: usize = 2 * NN;
const FRAME_LEN: usize = NN * NSS;
const SYNC_PASS_MIN: f64 = 1.2;
const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

#[derive(Clone)]
pub struct DecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
}

pub struct DecodeOptions {
    pub sample_rate: Option<usize>,
    pub freq_low: Option<f64>,
    pub freq_high: Option<f64>,
    pub sync_min: Option<f64>,
    pub depth: Option<usize>,
    pub max_candidates: Option<usize>,
    pub hash_call_book: Option<HashCallBook>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions {
            sample_rate: None,
            freq_low: None,
            freq_high: None,
            sync_min: None,
            depth: None,
            max_candidates: None,
            hash_call_book: None,
        }
    }
}

struct Candidate {
    freq: f64,
    sync: f64,
}

#[derive(Clone)]
struct SyncTemplate {
    re: Vec<f64>,
    im: Vec<f64>,
}

struct DecodeWorkspace {
    coarse_re: Vec<f64>,
    coarse_im: Vec<f64>,
    fine_re: Vec<f64>,
    fine_im: Vec<f64>,
    frame_re: Vec<f64>,
    frame_im: Vec<f64>,
    symb_re: Vec<f64>,
    symb_im: Vec<f64>,
    cs_re: Vec<f64>,
    cs_im: Vec<f64>,
    s4: Vec<f64>,
    s2: Vec<f64>,
    bitmetrics1: Vec<f64>,
    bitmetrics2: Vec<f64>,
    bitmetrics3: Vec<f64>,
    llra: Vec<f64>,
    llrb: Vec<f64>,
    llrc: Vec<f64>,
    llr: Vec<f64>,
    apmask: Vec<i8>,
}

fn create_decode_workspace() -> DecodeWorkspace {
    DecodeWorkspace {
        coarse_re: vec![0.0; NFFT2],
        coarse_im: vec![0.0; NFFT2],
        fine_re: vec![0.0; NFFT2],
        fine_im: vec![0.0; NFFT2],
        frame_re: vec![0.0; FRAME_LEN],
        frame_im: vec![0.0; FRAME_LEN],
        symb_re: vec![0.0; NSS],
        symb_im: vec![0.0; NSS],
        cs_re: vec![0.0; 4 * NN],
        cs_im: vec![0.0; 4 * NN],
        s4: vec![0.0; 4 * NN],
        s2: vec![0.0; 1 << 8],
        bitmetrics1: vec![0.0; BITMETRIC_LEN],
        bitmetrics2: vec![0.0; BITMETRIC_LEN],
        bitmetrics3: vec![0.0; BITMETRIC_LEN],
        llra: vec![0.0; LDPC_BITS],
        llrb: vec![0.0; LDPC_BITS],
        llrc: vec![0.0; LDPC_BITS],
        llr: vec![0.0; LDPC_BITS],
        apmask: vec![0; LDPC_BITS],
    }
}

pub fn decode(samples: &[f32], options: DecodeOptions) -> Vec<DecodedMessage> {
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);
    let freq_low = options.freq_low.unwrap_or(200.0);
    let freq_high = options.freq_high.unwrap_or(3000.0);
    let sync_min = options.sync_min.unwrap_or(1.2);
    let depth = options.depth.unwrap_or(2);
    let max_candidates = options.max_candidates.unwrap_or(100);
    let book = options.hash_call_book;

    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };

    let mut cx_re = vec![0.0; NMAX];
    let mut cx_im = vec![0.0; NMAX];
    for i in 0..NMAX {
        cx_re[i] = dd[i];
    }
    fft_complex(&mut cx_re, &mut cx_im, false);

    let candidates = get_candidates4(&dd, freq_low, freq_high, sync_min, max_candidates);
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut workspace = create_decode_workspace();
    let mut decoded: Vec<DecodedMessage> = Vec::new();
    let mut seen_messages = std::collections::HashSet::new();

    for candidate in candidates {
        if let Some(one) = decode_candidate(
            &candidate,
            &cx_re,
            &cx_im,
            depth,
            &book,
            &mut workspace,
        ) {
            if seen_messages.contains(&one.msg) {
                continue;
            }
            seen_messages.insert(one.msg.clone());
            decoded.push(one);
        }
    }

    decoded
}

fn copy_samples_to_decode_window(samples: &[f32]) -> Vec<f64> {
    let len = samples.len().min(NMAX);
    samples[..len].iter().map(|&x| x as f64).collect()
}

fn resample(input: &[f32], from_rate: usize, to_rate: usize, out_len: usize) -> Vec<f64> {
    let ratio = from_rate as f64 / to_rate as f64;
    (0..out_len)
        .map(|i| {
            let src_idx = i as f64 * ratio;
            let lo = src_idx.floor() as usize;
            let frac = src_idx - lo as f64;
            let v0 = if lo < input.len() { input[lo] as f64 } else { 0.0 };
            let v1 = if lo + 1 < input.len() {
                input[lo + 1] as f64
            } else {
                0.0
            };
            v0 * (1.0 - frac) + v1 * frac
        })
        .collect()
}

fn decode_candidate(
    candidate: &Candidate,
    cx_re: &[f64],
    cx_im: &[f64],
    depth: usize,
    book: &Option<HashCallBook>,
    workspace: &mut DecodeWorkspace,
) -> Option<DecodedMessage> {
    ft4_downsample(
        cx_re,
        cx_im,
        candidate.freq,
        &mut workspace.coarse_re,
        &mut workspace.coarse_im,
    );
    normalize_complex_power(
        &mut workspace.coarse_re,
        &mut workspace.coarse_im,
        NMAX / NDOWN,
    );

    for segment in 1..=3 {
        let coarse = find_best_sync_location(
            &workspace.coarse_re,
            &workspace.coarse_im,
            segment,
        );
        if coarse.smax < SYNC_PASS_MIN {
            continue;
        }

        let f1 = candidate.freq + coarse.idfbest as f64;
        if f1 <= 10.0 || f1 >= 4990.0 {
            continue;
        }

        ft4_downsample(
            cx_re,
            cx_im,
            f1,
            &mut workspace.fine_re,
            &mut workspace.fine_im,
        );
        normalize_complex_power(
            &mut workspace.fine_re,
            &mut workspace.fine_im,
            NSS * NN,
        );
        extract_frame(
            &workspace.fine_re,
            &workspace.fine_im,
            coarse.ibest,
            &mut workspace.frame_re,
            &mut workspace.frame_im,
        );

        if build_bit_metrics(workspace) {
            continue;
        }
        if !passes_hard_sync_quality(&workspace.bitmetrics1) {
            continue;
        }

        build_llrs(workspace);
        let result = try_decode_passes(workspace, depth)?;

        let message77_scrambled: Vec<u8> = result.message91[..77].to_vec();
        if !has_non_zero_bit(&message77_scrambled) {
            continue;
        }

        let message77 = xor_with_scrambler(&message77_scrambled);
        let msg = unpack77(&message77, book.as_ref())?;
        if msg.trim().is_empty() {
            continue;
        }

        return Some(DecodedMessage {
            freq: f1,
            dt: coarse.ibest as f64 / FS2 - 0.5,
            snr: to_ft4_snr(candidate.sync - 1.0),
            msg,
            sync: coarse.smax,
        });
    }

    None
}

struct CandidateSearchResult {
    ibest: isize,
    idfbest: isize,
    smax: f64,
}

fn find_best_sync_location(
    cd_re: &[f64],
    cd_im: &[f64],
    segment: usize,
) -> CandidateSearchResult {
    let mut ibest: isize = -1;
    let mut idfbest: isize = 0;
    let mut smax = -99.0;

    for isync in 1..=2 {
        let (idfmin, idfmax, idfstp, ibmin, ibmax, ibstp) = if isync == 1 {
            let (ibmin, ibmax) = match segment {
                1 => (108, 560),
                2 => (560, 1012),
                _ => (-344, 108),
            };
            (-12, 12, 3, ibmin, ibmax, 4)
        } else {
            (
                idfbest - 4,
                idfbest + 4,
                1,
                ibest - 5,
                ibest + 5,
                1,
            )
        };

        let templates = create_tweaked_sync_templates();
        for idf in (idfmin..=idfmax).step_by(idfstp as usize) {
            if let Some(tpls) = templates.get(&(idf)) {
                for istart in (ibmin..=ibmax).step_by(ibstp as usize) {
                    let sync = sync4d(cd_re, cd_im, istart, tpls);
                    if sync > smax {
                        smax = sync;
                        ibest = istart;
                        idfbest = idf;
                    }
                }
            }
        }
    }

    CandidateSearchResult {
        ibest,
        idfbest,
        smax,
    }
}

fn create_tweaked_sync_templates(
) -> std::collections::HashMap<isize, [SyncTemplate; 4]> {
    static T: std::sync::OnceLock<std::collections::HashMap<isize, [SyncTemplate; 4]>> =
        std::sync::OnceLock::new();
    T.get_or_init(|| {
        let base = create_base_sync_templates();
        let fsample = FS2 / 2.0;
        let mut out = std::collections::HashMap::new();

        for idf in -FT4_MAX_TWEAK..=FT4_MAX_TWEAK {
            let tweak = create_frequency_tweak(idf, 2 * NSS, fsample);
            out.insert(
                idf,
                [
                    apply_tweak(&base[0], &tweak),
                    apply_tweak(&base[1], &tweak),
                    apply_tweak(&base[2], &tweak),
                    apply_tweak(&base[3], &tweak),
                ],
            );
        }
        out
    })
    .clone()
}

fn create_base_sync_templates() -> [SyncTemplate; 4] {
    [
        build_sync_template(&COSTAS_A),
        build_sync_template(&COSTAS_B),
        build_sync_template(&COSTAS_C),
        build_sync_template(&COSTAS_D),
    ]
}

fn build_sync_template(tones: &[u8; 4]) -> SyncTemplate {
    let mut re = vec![0.0; 2 * NSS];
    let mut im = vec![0.0; 2 * NSS];
    let mut k = 0;
    let mut phi: f64 = 0.0;

    for &tone in tones {
        let dphi = (TWO_PI * tone as f64 * 2.0) / NSS as f64;
        for _j in 0..NSS / 2 {
            re[k] = phi.cos();
            im[k] = phi.sin();
            phi = (phi + dphi) % TWO_PI;
            k += 1;
        }
    }

    SyncTemplate { re, im }
}

fn create_frequency_tweak(idf: isize, npts: usize, fsample: f64) -> SyncTemplate {
    let mut re = vec![0.0; npts];
    let mut im = vec![0.0; npts];
    let dphi = TWO_PI * idf as f64 / fsample;
    let step_re = dphi.cos();
    let step_im = dphi.sin();
    let mut w_re = 1.0;
    let mut w_im = 0.0;

    for i in 0..npts {
        let new_re = w_re * step_re - w_im * step_im;
        let new_im = w_re * step_im + w_im * step_re;
        w_re = new_re;
        w_im = new_im;
        re[i] = w_re;
        im[i] = w_im;
    }

    SyncTemplate { re, im }
}

fn apply_tweak(template: &SyncTemplate, tweak: &SyncTemplate) -> SyncTemplate {
    let mut re = vec![0.0; template.re.len()];
    let mut im = vec![0.0; template.im.len()];
    for i in 0..template.re.len() {
        let sr = template.re[i];
        let si = template.im[i];
        let tr = tweak.re[i];
        let ti = tweak.im[i];
        re[i] = tr * sr - ti * si;
        im[i] = tr * si + ti * sr;
    }
    SyncTemplate { re, im }
}

fn sync4d(
    cd_re: &[f64],
    cd_im: &[f64],
    i0: isize,
    templates: &[SyncTemplate; 4],
) -> f64 {
    let mut sync = 0.0;
    for i in 0..COSTAS_BLOCKS {
        let start = i0 + i as isize * FT4_SYNC_STRIDE as isize;
        let z = correlate_stride2(
            cd_re,
            cd_im,
            start,
            &templates[i].re,
            &templates[i].im,
        );
        if z.2 <= 16 {
            continue;
        }
        sync += z.0.hypot(z.1) / (2.0 * NSS as f64);
    }
    sync
}

fn correlate_stride2(
    cd_re: &[f64],
    cd_im: &[f64],
    start: isize,
    template_re: &[f64],
    template_im: &[f64],
) -> (f64, f64, usize) {
    let mut z_re = 0.0;
    let mut z_im = 0.0;
    let mut count = 0;
    for i in 0..template_re.len() {
        let idx = start + 2 * i as isize;
        if idx < 0 || idx >= cd_re.len() as isize {
            continue;
        }
        let s_re = template_re[i];
        let s_im = template_im[i];
        let d_re = cd_re[idx as usize];
        let d_im = cd_im[idx as usize];
        z_re += d_re * s_re + d_im * s_im;
        z_im += d_im * s_re - d_re * s_im;
        count += 1;
    }
    (z_re, z_im, count)
}

fn ft4_downsample(
    cx_re: &[f64],
    cx_im: &[f64],
    f0: f64,
    out_re: &mut [f64],
    out_im: &mut [f64],
) {
    out_re.fill(0.0);
    out_im.fill(0.0);
    let df = SAMPLE_RATE as f64 / NMAX as f64;
    let baud = SAMPLE_RATE as f64 / NSPS as f64;
    let bw_transition = 0.5 * baud;
    let bw_flat = 4.0 * baud;
    let iwt = 1.max((bw_transition / df).trunc() as usize);
    let iwf = 1.max((bw_flat / df).trunc() as usize);
    let iws = (baud / df).trunc() as usize;

    let mut raw = vec![0.0; NFFT2];
    for i in 0..iwt.min(raw.len()) {
        raw[i] = 0.5 * (1.0 + ((iwt - 1 - i) as f64 * std::f64::consts::PI / iwt as f64).cos());
    }
    for i in iwt..(iwt + iwf).min(raw.len()) {
        raw[i] = 1.0;
    }
    for i in (iwt + iwf)..(2 * iwt + iwf).min(raw.len()) {
        raw[i] = 0.5
            * (1.0
                + (((i - (iwt + iwf)) as f64 * std::f64::consts::PI) / iwt as f64).cos());
    }

    let mut window = vec![0.0; NFFT2];
    for i in 0..NFFT2 {
        let src = (i + iws) % NFFT2;
        window[i] = raw[src];
    }

    let i0 = (f0 / df).round() as usize;
    if i0 <= NMAX / 2 {
        out_re[0] = cx_re[i0];
        out_im[0] = cx_im[i0];
    }

    for i in 1..=NFFT2 / 2 {
        let hi = i0 + i;
        if hi <= NMAX / 2 {
            out_re[i] = cx_re[hi];
            out_im[i] = cx_im[hi];
        }
        let lo = i0.saturating_sub(i);
        if lo <= NMAX / 2 {
            let idx = NFFT2 - i;
            out_re[idx] = cx_re[lo];
            out_im[idx] = cx_im[lo];
        }
    }

    let scale = 1.0 / NFFT2 as f64;
    for i in 0..NFFT2 {
        let w = window[i] * scale;
        out_re[i] *= w;
        out_im[i] *= w;
    }

    fft_complex(out_re, out_im, true);
}

fn normalize_complex_power(re: &mut [f64], im: &mut [f64], denom: usize) {
    let mut sum = 0.0;
    for i in 0..re.len() {
        sum += re[i] * re[i] + im[i] * im[i];
    }
    if sum <= 0.0 {
        return;
    }
    let scale = 1.0 / (sum / denom as f64).sqrt();
    for i in 0..re.len() {
        re[i] *= scale;
        im[i] *= scale;
    }
}

fn extract_frame(
    cb_re: &[f64],
    cb_im: &[f64],
    ibest: isize,
    out_re: &mut [f64],
    out_im: &mut [f64],
) {
    for i in 0..out_re.len() {
        let src = ibest + i as isize;
        if src >= 0 && src < cb_re.len() as isize {
            out_re[i] = cb_re[src as usize];
            out_im[i] = cb_im[src as usize];
        } else {
            out_re[i] = 0.0;
            out_im[i] = 0.0;
        }
    }
}

fn get_candidates4(
    dd: &[f64],
    freq_low: f64,
    freq_high: f64,
    sync_min: f64,
    max_candidates: usize,
) -> Vec<Candidate> {
    let df = SAMPLE_RATE as f64 / NFFT1 as f64;
    let fac = 1.0 / 300.0;
    let mut savg = vec![0.0; NH1];
    let mut s = vec![0.0; NH1 * NHSYM];
    let mut savsm = vec![0.0; NH1];

    let mut x_re = vec![0.0; NFFT1];
    let mut x_im = vec![0.0; NFFT1];

    // Nuttall window
    let a0 = 0.3635819;
    let a1 = -0.4891775;
    let a2 = 0.1365995;
    let a3 = -0.0106411;
    let nuttall: Vec<f64> = (0..NFFT1)
        .map(|i| {
            a0 + a1 * (2.0 * std::f64::consts::PI * i as f64 / NFFT1 as f64).cos()
                + a2 * (4.0 * std::f64::consts::PI * i as f64 / NFFT1 as f64).cos()
                + a3 * (6.0 * std::f64::consts::PI * i as f64 / NFFT1 as f64).cos()
        })
        .collect();

    for j in 0..NHSYM {
        let ia = j * NSPS;
        let ib = ia + NFFT1;
        if ib > NMAX {
            break;
        }

        x_im.fill(0.0);
        for i in 0..NFFT1 {
            x_re[i] = fac * dd[ia + i] * nuttall[i];
        }
        fft_complex(&mut x_re, &mut x_im, false);

        for bin in 1..=NH1 {
            let idx = bin - 1;
            let re = x_re[bin];
            let im = x_im[bin];
            let power = re * re + im * im;
            s[idx * NHSYM + j] = power;
            savg[idx] += power;
        }
    }

    for i in 0..NH1 {
        savg[i] /= NHSYM as f64;
    }

    for i in 7..NH1 - 7 {
        let mut sum = 0.0;
        for j in (i - 7)..=(i + 7) {
            sum += savg[j];
        }
        savsm[i] = sum / 15.0;
    }

    let mut nfa = (freq_low / df).round() as usize;
    if nfa < (200.0 / df).round() as usize {
        nfa = (200.0 / df).round() as usize;
    }
    let max_freq = 4910.0;
    let mut nfb = (freq_high / df).round() as usize;
    if nfb > (max_freq / df).round() as usize {
        nfb = (max_freq / df).round() as usize;
    }

    let sbase = ft4_baseline(&savg, nfa, nfb, df);
    for bin in nfa..=nfb {
        if sbase[bin - 1] <= 0.0 {
            return Vec::new();
        }
    }

    for bin in nfa..=nfb {
        let idx = bin - 1;
        savsm[idx] /= sbase[idx];
    }

    let f_offset = (-1.5 * SAMPLE_RATE as f64) / NSPS as f64;
    let mut candidates: Vec<Candidate> = Vec::new();

    for i in (nfa + 1)..=nfb - 1 {
        let left = savsm[i - 2];
        let center = savsm[i - 1];
        let right = savsm[i];
        if center >= left && center >= right && center >= sync_min {
            let den = left - 2.0 * center + right;
            let del = if den != 0.0 {
                0.5 * (left - right) / den
            } else {
                0.0
            };
            let fpeak = (i as f64 + del) * df + f_offset;
            if fpeak < 200.0 || fpeak > max_freq {
                continue;
            }
            let speak = center - 0.25 * (left - right) * del;
            candidates.push(Candidate {
                freq: fpeak,
                sync: speak,
            });
        }
    }

    candidates.sort_by(|a, b| b.sync.partial_cmp(&a.sync).unwrap());
    candidates.into_iter().take(max_candidates).collect()
}

fn ft4_baseline(savg: &[f64], _nfa: usize, nfb: usize, df: f64) -> Vec<f64> {
    let mut sbase = vec![1.0; NH1];

    let ia = (200.0 / df).round() as usize;
    let ib = NH1.min(nfb);
    if ib <= ia {
        return sbase;
    }

    let mut s_db = vec![0.0; NH1];
    for i in ia..=ib {
        s_db[i - 1] = 10.0 * 1e-30f64.max(savg[i - 1]).log10();
    }

    let nseg = 10;
    let npct = 10;
    let nlen = ((ib - ia + 1) / nseg).max(1);
    let i0 = (ib - ia + 1) / 2;

    let mut x: Vec<f64> = Vec::new();
    let mut y: Vec<f64> = Vec::new();
    for seg in 0..nseg {
        let ja = ia + seg * nlen;
        if ja > ib {
            break;
        }
        let jb = ib.min(ja + nlen - 1);

        let vals: Vec<f64> = s_db[ja..=jb].to_vec();
        let base = percentile(&vals, npct);

        for i in ja..=jb {
            let v = s_db[i - 1];
            if v <= base {
                x.push(i as f64 - i0 as f64);
                y.push(v);
            }
        }
    }

    let coeff = if x.len() >= 5 {
        polyfit_least_squares(&x, &y, 4)
    } else {
        None
    };

    if let Some(coeff) = coeff {
        for i in ia..=ib {
            let t = i as f64 - i0 as f64;
            let db = coeff[0]
                + t * (coeff[1] + t * (coeff[2] + t * (coeff[3] + t * coeff[4])))
                + 0.65;
            sbase[i - 1] = 10f64.powf(db / 10.0);
        }
    } else {
        let half_window = 25;
        for i in ia..=ib {
            let lo = ia.max(if i > half_window {
                i - half_window
            } else {
                0
            });
            let hi = ib.min(i + half_window);
            let mut sum = 0.0;
            let mut count = 0;
            for j in lo..=hi {
                sum += savg[j - 1];
                count += 1;
            }
            sbase[i - 1] = if count > 0 {
                sum / count as f64
            } else {
                1.0
            };
        }
    }

    sbase
}

fn percentile(values: &[f64], pct: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((pct as f64 / 100.0) * (sorted.len() - 1) as f64)
        .floor()
        .max(0.0)
        .min(sorted.len() as f64 - 1.0) as usize;
    sorted[idx]
}

fn polyfit_least_squares(x: &[f64], y: &[f64], degree: usize) -> Option<Vec<f64>> {
    let n = degree + 1;
    let mut mat = vec![vec![0.0; n + 1]; n];

    let mut x_pows = vec![0.0; 2 * degree + 1];
    for p in 0..=2 * degree {
        let mut sum = 0.0;
        for i in 0..x.len() {
            sum += x[i].powi(p as i32);
        }
        x_pows[p] = sum;
    }

    for row in 0..n {
        for col in 0..n {
            mat[row][col] = x_pows[row + col];
        }
        let mut rhs = 0.0;
        for i in 0..x.len() {
            rhs += y[i] * x[i].powi(row as i32);
        }
        mat[row][n] = rhs;
    }

    for col in 0..n {
        let mut pivot = col;
        let mut max_abs = mat[col][col].abs();
        for row in (col + 1)..n {
            let a = mat[row][col].abs();
            if a > max_abs {
                max_abs = a;
                pivot = row;
            }
        }
        if max_abs < 1e-12 {
            return None;
        }
        if pivot != col {
            mat.swap(col, pivot);
        }

        let pivot_val = mat[col][col];
        for c in col..=n {
            mat[col][c] /= pivot_val;
        }

        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = mat[row][col];
            if factor == 0.0 {
                continue;
            }
            for c in col..=n {
                mat[row][c] -= factor * mat[col][c];
            }
        }
    }

    let coeff = (0..n).map(|i| mat[i][n]).collect();
    Some(coeff)
}

fn build_bit_metrics(workspace: &mut DecodeWorkspace) -> bool {
    for k in 0..NN {
        let i1 = k * NSS;
        for i in 0..NSS {
            workspace.symb_re[i] = workspace.frame_re[i1 + i];
            workspace.symb_im[i] = workspace.frame_im[i1 + i];
        }
        fft_complex(
            &mut workspace.symb_re,
            &mut workspace.symb_im,
            false,
        );

        for tone in 0..4 {
            let idx = tone * NN + k;
            let re = workspace.symb_re[tone];
            let im = workspace.symb_im[tone];
            workspace.cs_re[idx] = re;
            workspace.cs_im[idx] = im;
            workspace.s4[idx] = re.hypot(im);
        }
    }

    let mut nsync = 0;
    for k in 0..4 {
        if max_tone(&workspace.s4, k) == COSTAS_A[k] as usize {
            nsync += 1;
        }
        if max_tone(&workspace.s4, 33 + k) == COSTAS_B[k] as usize {
            nsync += 1;
        }
        if max_tone(&workspace.s4, 66 + k) == COSTAS_C[k] as usize {
            nsync += 1;
        }
        if max_tone(&workspace.s4, 99 + k) == COSTAS_D[k] as usize {
            nsync += 1;
        }
    }

    workspace.bitmetrics1.fill(0.0);
    workspace.bitmetrics2.fill(0.0);
    workspace.bitmetrics3.fill(0.0);
    if nsync < 6 {
        return true;
    }

    for nseq in 1..=3 {
        let (nsym, nt, ibmax) = match nseq {
            1 => (1, 1 << 2, 1),
            2 => (2, 1 << 4, 3),
            _ => (4, 1 << 8, 7),
        };

        for ks in (1..=NN - nsym + 1).step_by(nsym) {
            for i in 0..nt {
                let i1 = i / 64;
                let i2 = (i & 63) / 16;
                let i3 = (i & 15) / 4;
                let i4 = i & 3;

                let val = if nsym == 1 {
                    let t = GRAYMAP[i4] as usize;
                    let idx = t * NN + (ks - 1);
                    workspace.cs_re[idx].hypot(workspace.cs_im[idx])
                } else if nsym == 2 {
                    let t3 = GRAYMAP[i3] as usize;
                    let t4 = GRAYMAP[i4] as usize;
                    let i_a = t3 * NN + (ks - 1);
                    let i_b = t4 * NN + ks;
                    let re = workspace.cs_re[i_a] + workspace.cs_re[i_b];
                    let im = workspace.cs_im[i_a] + workspace.cs_im[i_b];
                    re.hypot(im)
                } else {
                    let t1 = GRAYMAP[i1] as usize;
                    let t2 = GRAYMAP[i2] as usize;
                    let t3 = GRAYMAP[i3] as usize;
                    let t4 = GRAYMAP[i4] as usize;
                    let i_a = t1 * NN + (ks - 1);
                    let i_b = t2 * NN + ks;
                    let i_c = t3 * NN + (ks + 1);
                    let i_d = t4 * NN + (ks + 2);
                    let re = workspace.cs_re[i_a]
                        + workspace.cs_re[i_b]
                        + workspace.cs_re[i_c]
                        + workspace.cs_re[i_d];
                    let im = workspace.cs_im[i_a]
                        + workspace.cs_im[i_b]
                        + workspace.cs_im[i_c]
                        + workspace.cs_im[i_d];
                    re.hypot(im)
                };
                workspace.s2[i] = val;
            }

            let ipt = 1 + (ks - 1) * 2;
            for ib in 0..=ibmax {
                let mask = 1 << (ibmax - ib);
                let mut max1 = -1e30;
                let mut max0 = -1e30;
                for i in 0..nt {
                    let v = workspace.s2[i];
                    if (i & mask) != 0 {
                        if v > max1 {
                            max1 = v;
                        }
                    } else if v > max0 {
                        max0 = v;
                    }
                }

                let idx = ipt + ib;
                if idx > BITMETRIC_LEN {
                    continue;
                }

                let bm = max1 - max0;
                let target = match nseq {
                    1 => &mut workspace.bitmetrics1,
                    2 => &mut workspace.bitmetrics2,
                    _ => &mut workspace.bitmetrics3,
                };
                target[idx - 1] = bm;
            }
        }
    }

    workspace.bitmetrics2[208] = workspace.bitmetrics1[208];
    workspace.bitmetrics2[209] = workspace.bitmetrics1[209];
    workspace.bitmetrics3[208] = workspace.bitmetrics1[208];
    workspace.bitmetrics3[209] = workspace.bitmetrics1[209];

    normalize_bit_metrics(&mut workspace.bitmetrics1);
    normalize_bit_metrics(&mut workspace.bitmetrics2);
    normalize_bit_metrics(&mut workspace.bitmetrics3);
    false
}

fn max_tone(s4: &[f64], symbol_index: usize) -> usize {
    let mut best_tone = 0;
    let mut best_value = -1.0;
    for tone in 0..4 {
        let v = s4[tone * NN + symbol_index];
        if v > best_value {
            best_value = v;
            best_tone = tone;
        }
    }
    best_tone
}

fn normalize_bit_metrics(bmet: &mut [f64]) {
    let mut sum = 0.0;
    let mut sum2 = 0.0;
    for i in 0..bmet.len() {
        sum += bmet[i];
        sum2 += bmet[i] * bmet[i];
    }
    let avg = sum / bmet.len() as f64;
    let avg2 = sum2 / bmet.len() as f64;
    let variance = avg2 - avg * avg;
    let sigma = if variance > 0.0 {
        variance.sqrt()
    } else {
        avg2.sqrt()
    };
    if sigma <= 0.0 {
        return;
    }
    for i in 0..bmet.len() {
        bmet[i] /= sigma;
    }
}

fn passes_hard_sync_quality(bitmetrics1: &[f64]) -> bool {
    const HARD_SYNC_PATTERNS: [(isize, [u8; 8]); 4] = [
        (0, [0, 0, 0, 1, 1, 0, 1, 1]),
        (66, [0, 1, 0, 0, 1, 1, 1, 0]),
        (132, [1, 1, 1, 0, 0, 1, 0, 0]),
        (198, [1, 0, 1, 1, 0, 0, 0, 1]),
    ];

    let hard: Vec<u8> = bitmetrics1.iter().map(|&x| if x >= 0.0 { 1 } else { 0 }).collect();

    let mut score = 0;
    for (offset, pattern) in &HARD_SYNC_PATTERNS {
        for i in 0..8 {
            if hard[*offset as usize + i] == pattern[i] {
                score += 1;
            }
        }
    }
    score >= 10
}

fn build_llrs(workspace: &mut DecodeWorkspace) {
    for i in 0..58 {
        workspace.llra[i] = workspace.bitmetrics1[8 + i];
        workspace.llra[58 + i] = workspace.bitmetrics1[74 + i];
        workspace.llra[116 + i] = workspace.bitmetrics1[140 + i];

        workspace.llrb[i] = workspace.bitmetrics2[8 + i];
        workspace.llrb[58 + i] = workspace.bitmetrics2[74 + i];
        workspace.llrb[116 + i] = workspace.bitmetrics2[140 + i];

        workspace.llrc[i] = workspace.bitmetrics3[8 + i];
        workspace.llrc[58 + i] = workspace.bitmetrics3[74 + i];
        workspace.llrc[116 + i] = workspace.bitmetrics3[140 + i];
    }
}

fn try_decode_passes(
    workspace: &mut DecodeWorkspace,
    depth: usize,
) -> Option<crate::util::decode174_91::DecodeResult> {
    let maxosd: isize = if depth >= 3 {
        2
    } else if depth >= 2 {
        0
    } else {
        -1
    };
    let scalefac = 2.83;
    let sources = [
        &workspace.llra,
        &workspace.llrb,
        &workspace.llrc,
    ];

    workspace.apmask.fill(0);
    for src in sources {
        for i in 0..LDPC_BITS {
            workspace.llr[i] = scalefac * src[i];
        }
        if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd) {
            return Some(result);
        }
    }

    None
}

fn has_non_zero_bit(bits: &[u8]) -> bool {
    bits.iter().any(|&b| b != 0)
}

fn to_ft4_snr(sync_minus_one: f64) -> f64 {
    if sync_minus_one > 0.0 {
        (-21.0f64).max(10.0 * sync_minus_one.log10() - 14.8).round()
    } else {
        -21.0
    }
}
