//! Mirrors JTDX `lib/sync8.f90`.

use crate::decode::lib_jtdx::four2a::four2a_r2c;
use crate::stream::session::StreamDecodeConfig;

use super::ft8_mod1::ICOS7;
use super::ft8_params::{NFFT1, NH1, NHSYM, NSPS, NSTEP};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMetricMode {
    Amplitude,
    Power,
    AbsSum,
}

impl SyncMetricMode {
    pub fn for_ipass(ipass: usize) -> Self {
        match ipass {
            1 | 4 | 7 => Self::Amplitude,
            2 | 5 | 8 => Self::Power,
            3 | 6 | 9 => Self::AbsSum,
            _ => Self::Amplitude,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SyncCandidate {
    pub freq: f32,
    pub dt: f32,
    pub sync: f32,
    pub lcq: bool,
    pub sort_metric: f32,
}

impl SyncCandidate {
    pub fn lcq(self) -> bool {
        self.lcq
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sync8Config {
    pub nfa: i32,
    pub nfb: i32,
    pub syncmin: f32,
    pub nfqso: i32,
    pub jzb: i32,
    pub jzt: i32,
    pub swl: bool,
    pub ipass: usize,
    pub lqsothread: bool,
    pub ncandthin: usize,
    pub filter: bool,
    pub ndtcenter: i32,
    pub lagcc: bool,
    pub lagccbail: bool,
    pub nfawide: i32,
    pub nfbwide: i32,
}

impl Sync8Config {
    pub fn from_stream(
        config: &StreamDecodeConfig,
        ipass: usize,
        syncmin: f32,
        avexdt: f32,
    ) -> Self {
        let avexdt_bins = (avexdt * 25.0) as i32;
        let (jzb, jzt) = if config.swl {
            (-86 + avexdt_bins, 86 + avexdt_bins)
        } else {
            (-62 + avexdt_bins, 62 + avexdt_bins)
        };

        let (nfa, nfb) = active_decode_band(config);

        Self {
            nfa,
            nfb,
            syncmin,
            nfqso: config.nfqso.round() as i32,
            jzb,
            jzt,
            swl: config.swl,
            ipass,
            lqsothread: config.nfqso >= nfa as f64 && config.nfqso <= nfb as f64,
            ncandthin: config.ncandthin,
            filter: config.filter,
            ndtcenter: 0,
            lagcc: config.nagcc,
            lagccbail: false,
            nfawide: config.nfa.round() as i32,
            nfbwide: config.nfb.round() as i32,
        }
    }

    pub fn metric_mode(self) -> SyncMetricMode {
        SyncMetricMode::for_ipass(self.ipass)
    }

    pub fn rcandthin(self) -> f32 {
        let rcandthin = self.ncandthin as f32 / 100.0;
        if self.filter {
            (rcandthin * 3.0).min(1.0)
        } else {
            rcandthin
        }
    }
}

fn active_decode_band(config: &StreamDecodeConfig) -> (i32, i32) {
    let mut nfa = config.nfa.round() as i32;
    let mut nfb = config.nfb.round() as i32;
    let nfqso = config.nfqso.round() as i32;

    if config.filter && nfqso >= nfa && nfqso <= nfb {
        let half_width = if config.lhound { 290 } else { 60 };
        nfa = nfa.max(nfqso - half_width);
        nfb = nfb.min(nfqso + half_width);
    }
    if config.nagain && nfqso >= nfa && nfqso <= nfb {
        nfa = nfa.max(nfqso - 25);
        nfb = nfb.min(nfqso + 25);
    }

    (nfa, nfb)
}

pub fn candidate_sort_metric(candidate: &SyncCandidate, config: Sync8Config) -> f32 {
    let rcandthin = config.rcandthin();
    if rcandthin > 0.99 {
        candidate.sync
    } else {
        let dtcenter = config.ndtcenter as f32 / 100.0;
        let dt_weight = (candidate.dt - dtcenter).abs() + 1.0;
        match config.ipass {
            2 | 5 | 8 => candidate.sync / (dt_weight * dt_weight),
            _ => candidate.sync / dt_weight,
        }
    }
}

pub fn sync8(dd8: &[f32], config: Sync8Config) -> Vec<SyncCandidate> {
    let mut workspace = Sync8Workspace::new(config.jzb, config.jzt);
    workspace.sync8(dd8, config)
}

struct Sync8Workspace {
    s: Vec<f32>,
    x_re: Vec<f64>,
    x_im: Vec<f64>,
    sync2d: Vec<f32>,
    syncq: Vec<bool>,
    red: Vec<f32>,
    redcq: Vec<bool>,
    jpeak: Vec<i32>,
    jzb: i32,
    jzt: i32,
}

impl Sync8Workspace {
    fn new(jzb: i32, jzt: i32) -> Self {
        let width = (jzt - jzb + 1).max(0) as usize;
        Self {
            s: vec![0.0; NH1 * NHSYM],
            x_re: vec![0.0; NFFT1],
            x_im: vec![0.0; NFFT1],
            sync2d: vec![0.0; NH1 * width],
            syncq: vec![false; NH1 * width],
            red: vec![0.0; NH1],
            redcq: vec![false; NH1],
            jpeak: vec![0; NH1],
            jzb,
            jzt,
        }
    }

    fn sync8(&mut self, dd8: &[f32], config: Sync8Config) -> Vec<SyncCandidate> {
        self.compute_symbol_spectra(dd8, config.metric_mode());
        self.compute_sync2d(config);
        self.extract_candidates(config)
    }

    fn compute_symbol_spectra(&mut self, dd8: &[f32], mode: SyncMetricMode) {
        let facx = 1.0 / 300.0f32;
        let windowx = windowx();
        for j in 1..=NHSYM {
            let ia = (j - 1) * NSTEP;
            let ib = ia + NSPS - 1;
            self.x_re.fill(0.0);
            self.x_im.fill(0.0);

            if j != 1 {
                for i in 0..=200 {
                    let src = ia.saturating_sub(201) + i;
                    self.x_re[759 + i] =
                        (dd8.get(src).copied().unwrap_or(0.0) * windowx[200 - i]) as f64;
                }
            }
            for i in 0..NSPS {
                self.x_re[960 + i] = (facx * dd8.get(ia + i).copied().unwrap_or(0.0)) as f64;
            }
            self.x_re[960] *= 1.9;
            self.x_re[2879] *= 1.9;
            if j != NHSYM {
                for i in 0..=200 {
                    self.x_re[2880 + i] =
                        (dd8.get(ib + 1 + i).copied().unwrap_or(0.0) * windowx[i]) as f64;
                }
            }

            four2a_r2c(&mut self.x_re, &mut self.x_im);
            for i in 1..=NH1 {
                let re = self.x_re[i];
                let im = self.x_im[i];
                let value = match mode {
                    SyncMetricMode::Amplitude => (re * re + im * im).sqrt() as f32,
                    SyncMetricMode::Power => (re * re + im * im) as f32,
                    SyncMetricMode::AbsSum => (re.abs() + im.abs()) as f32,
                };
                self.s[s_idx(i, j)] = value;
            }
        }
    }

    fn compute_sync2d(&mut self, config: Sync8Config) {
        self.sync2d.fill(0.0);
        self.syncq.fill(false);
        let df = 3.125f32;
        let iaw = (config.nfawide as f32 / df).round().max(1.0) as usize;
        let ibw = (config.nfbwide as f32 / df)
            .round()
            .max(1.0)
            .min((NH1 - 16) as f32) as usize;
        let nssy = 4isize;
        let nssy36 = 144isize;
        let nssy72 = 288isize;
        let nfos = 2usize;
        let nfos6 = 16usize;
        let jstrt = 12isize;

        if config.lagcc && !config.lagccbail {
            self.compute_sync2d_agc(config, iaw, ibw, nssy, nssy36, nssy72, nfos, jstrt);
            return;
        }

        for j in config.jzb..=config.jzt {
            for i in iaw..=ibw {
                let mut ta = 0.0f32;
                let mut tb = 0.0f32;
                let mut tc = 0.0f32;
                let mut tcq = 0.0f32;
                let mut t0a = 0.0f32;
                let mut t0b = 0.0f32;
                let mut t0c = 0.0f32;
                let mut t0cq = 0.0f32;

                for n in 0..7 {
                    let k = j as isize + jstrt + nssy * n as isize;
                    let i_costas = i + nfos * ICOS7[n] as usize;
                    if k > 0 && k <= NHSYM as isize {
                        let k = k as usize;
                        ta += self.s[s_idx(i_costas, k)];
                        t0a += sum_s(&self.s, i, i + nfos6, k) - self.s[s_idx(i_costas + 1, k)];
                    }
                    let k36 = k + nssy36;
                    if k36 > 0 && k36 <= NHSYM as isize {
                        let k36 = k36 as usize;
                        tb += self.s[s_idx(i_costas, k36)];
                        t0b += sum_s(&self.s, i, i + nfos6, k36) - self.s[s_idx(i_costas + 1, k36)];
                    }
                    let k72 = k + nssy72;
                    if k72 > 0 && k72 <= NHSYM as isize {
                        let k72 = k72 as usize;
                        tc += self.s[s_idx(i_costas, k72)];
                        t0c += sum_s(&self.s, i, i + nfos6, k72) - self.s[s_idx(i_costas + 1, k72)];
                    }
                }

                for n in 7..=15 {
                    let k = j as isize + jstrt + nssy * n as isize;
                    if k >= 1 && k <= NHSYM as isize {
                        let k = k as usize;
                        if n < 15 {
                            tcq += self.s[s_idx(i, k)];
                            t0cq += sum_s(&self.s, i, i + nfos6, k) - self.s[s_idx(i, k + 1)];
                        } else {
                            tcq += self.s[s_idx(i + 2, k)];
                            t0cq += sum_s(&self.s, i, i + nfos6, k) - self.s[s_idx(i, k + 3)];
                        }
                    }
                }

                let (syncf, lcq) = sync_pair(ta + tb + tc, t0a + t0b + t0c, tcq, t0cq, 42.0, 60.0);
                let (syncs, lcq2) = sync_pair(tb + tc, t0b + t0c, tcq, t0cq, 28.0, 46.0);
                let sync = syncf.max(syncs);
                let cq = if syncf > syncs { lcq } else { lcq2 };
                let idx = self.j_idx(i, j);
                self.sync2d[idx] = sync;
                self.syncq[idx] = cq;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_sync2d_agc(
        &mut self,
        config: Sync8Config,
        iaw: usize,
        ibw: usize,
        nssy: isize,
        nssy36: isize,
        nssy72: isize,
        nfos: usize,
        jstrt: isize,
    ) {
        let nfos6 = 12usize;
        let mut tall = [0.0f32; 30];
        for j in config.jzb..=config.jzt {
            for i in iaw..=ibw {
                tall.fill(0.0);
                for n in 0..7 {
                    let k = j as isize + jstrt + nssy * n as isize;
                    let i_costas = i + nfos * ICOS7[n] as usize;
                    if k > 0 && k <= NHSYM as isize {
                        let k = k as usize;
                        let ta = self.s[s_idx(i_costas, k)];
                        tall[n] = if ta > 1e-9 {
                            ta * 6.0 / (sum_s_stride(&self.s, i, i + nfos6, nfos, k) - ta)
                        } else {
                            0.0
                        };
                    }
                    let k36 = k + nssy36;
                    if k36 > 0 && k36 <= NHSYM as isize {
                        let k36 = k36 as usize;
                        let tb = self.s[s_idx(i_costas, k36)];
                        tall[n + 16] = if tb > 1e-9 {
                            tb * 6.0 / (sum_s_stride(&self.s, i, i + nfos6, nfos, k36) - tb)
                        } else {
                            0.0
                        };
                    }
                    let k72 = k + nssy72;
                    if k72 > 0 && k72 <= NHSYM as isize {
                        let k72 = k72 as usize;
                        let tc = self.s[s_idx(i_costas, k72)];
                        tall[n + 23] = if tc > 1e-9 {
                            tc * 6.0 / (sum_s_stride(&self.s, i, i + nfos6, nfos, k72) - tc)
                        } else {
                            0.0
                        };
                    }
                }

                let mut lcq = false;
                let (sync_abc, sync_bc) = if config.ipass > 1 {
                    for n in 7..=15 {
                        let k = j as isize + jstrt + nssy * n as isize;
                        if k > 0 && k <= NHSYM as isize {
                            let k = k as usize;
                            let tone_i = if n < 15 { i } else { i + 2 };
                            let sig = self.s[s_idx(tone_i, k)];
                            tall[n] =
                                sig * 6.0 / (sum_s_stride(&self.s, i, i + nfos6, nfos, k) - sig);
                        }
                    }
                    let sya: f32 = tall[0..7].iter().sum();
                    let sycq: f32 = tall[7..16].iter().sum();
                    let sybc: f32 = tall[16..30].iter().sum();
                    let sy1 = (sya + sycq + sybc) / 30.0;
                    let sy2 = (sya + sybc) / 21.0;
                    let sync_abc = sy1.max(sy2);
                    let sy1 = (sycq + sybc) / 23.0;
                    let sy2 = sybc / 14.0;
                    lcq = sy1 > sy2;
                    (sync_abc, sy1.max(sy2))
                } else {
                    let sybc: f32 = tall[16..30].iter().sum();
                    ((tall[0..7].iter().sum::<f32>() + sybc) / 21.0, sybc / 14.0)
                };

                let idx = self.j_idx(i, j);
                self.sync2d[idx] = sync_abc.max(sync_bc);
                if lcq {
                    self.syncq[idx] = true;
                }
            }
        }
    }

    fn extract_candidates(&mut self, config: Sync8Config) -> Vec<SyncCandidate> {
        let df = 3.125f32;
        let ia = (config.nfa as f32 / df).round().max(1.0) as usize;
        let ib = (config.nfb as f32 / df)
            .round()
            .max(1.0)
            .min((NH1 - 16) as f32) as usize;
        let iaw = (config.nfawide as f32 / df).round().max(1.0) as usize;
        let ibw = (config.nfbwide as f32 / df)
            .round()
            .max(1.0)
            .min((NH1 - 16) as f32) as usize;

        self.red.fill(0.0);
        self.redcq.fill(false);
        for i in iaw..=ibw {
            let mut best = -1.0f32;
            let mut best_j = config.jzb;
            let mut best_cq = false;
            for j in config.jzb..=config.jzt {
                let idx = self.j_idx(i, j);
                let value = self.sync2d[idx];
                if value > best {
                    best = value;
                    best_j = j;
                    best_cq = self.syncq[idx];
                }
            }
            self.jpeak[i] = best_j;
            self.red[i] = best;
            self.redcq[i] = best_cq;
        }

        let mut base_values: Vec<f32> = self.red[iaw..=ibw].to_vec();
        base_values.sort_by(|a, b| a.total_cmp(b));
        let iz = ibw - iaw + 1;
        let base_idx = ((0.40 * iz as f32).round() as usize).max(1) - 1;
        let mut base = base_values[base_idx];
        if base < 1e-8 {
            base = 1.0;
        }
        for i in iaw..=ibw {
            self.red[i] /= base;
        }

        let mut order: Vec<usize> = (ia..=ib).collect();
        order.sort_by(|a, b| self.red[*b].total_cmp(&self.red[*a]));

        let mut candidate0 = Vec::new();
        for n in order {
            let freq = n as f32 * df;
            let red = self.red[n];
            if (freq - config.nfqso as f32).abs() > 3.0 {
                if red < config.syncmin {
                    continue;
                }
            } else if red < 1.1 {
                continue;
            }
            let jpeak = self.jpeak[n];
            if config.swl {
                if !(-74..=101).contains(&jpeak) {
                    continue;
                }
            } else if !(-49..=76).contains(&jpeak) {
                continue;
            }
            if candidate0.len() >= 450 {
                break;
            }
            let mut candidate = SyncCandidate {
                freq,
                dt: (jpeak - 1) as f32 * 0.04,
                sync: red,
                lcq: self.redcq[n],
                sort_metric: 0.0,
            };
            if config.rcandthin() < 0.99 {
                candidate.sort_metric = candidate_sort_metric(&candidate, config);
            }
            candidate0.push(candidate);
        }

        suppress_near_dupes(&mut candidate0, config);
        order_candidates(candidate0, config)
    }

    fn j_idx(&self, i: usize, j: i32) -> usize {
        let width = (self.jzt - self.jzb + 1) as usize;
        (i - 1) * width + (j - self.jzb) as usize
    }
}

fn sync_pair(t1: f32, t01: f32, tcq: f32, t0cq: f32, den1: f32, den2: f32) -> (f32, bool) {
    let mut noise1 = (t01 - t1 * 2.0) / den1;
    if noise1 < 1e-8 {
        noise1 = 1.0;
    }
    let mut noise2 = (t01 + t0cq - (t1 + tcq) * 2.0) / den2;
    if noise2 < 1e-8 {
        noise2 = 1.0;
    }
    let sync01 = t1 / (7.0 * noise1);
    let sync02 = (t1 / 7.0 + tcq / 9.0) / noise2;
    (sync01.max(sync02), sync02 > sync01)
}

fn order_candidates(mut candidate0: Vec<SyncCandidate>, config: Sync8Config) -> Vec<SyncCandidate> {
    if config.rcandthin() > 0.99 {
        candidate0.sort_by(|a, b| b.sync.total_cmp(&a.sync));
    } else {
        candidate0.sort_by(|a, b| b.sort_metric.total_cmp(&a.sort_metric));
    }

    let mut out = Vec::new();
    let mut fprev = 5004.0f32;
    for candidate in &candidate0 {
        if (candidate.freq - config.nfqso as f32).abs() <= 3.0
            && candidate.sync >= 1.1
            && (candidate.freq - fprev).abs() > 3.0
        {
            out.push(*candidate);
            fprev = candidate.freq;
        }
    }
    let mut ncandfqso = out.len();
    if config.lqsothread {
        out.push(SyncCandidate {
            freq: config.nfqso as f32,
            dt: 5.0,
            sync: 0.0,
            lcq: false,
            sort_metric: 0.0,
        });
        ncandfqso += 1;
        out.push(SyncCandidate {
            freq: config.nfqso as f32,
            dt: -5.0,
            sync: 0.0,
            lcq: false,
            sort_metric: 0.0,
        });
        ncandfqso += 1;
    }

    for candidate in &candidate0 {
        let syncmin1 = if (candidate.freq - config.nfqso as f32).abs() > 3.0 {
            config.syncmin
        } else {
            1.1
        };
        if candidate.sync >= syncmin1 {
            out.push(*candidate);
            if out.len() >= 460 {
                break;
            }
        }
    }

    if out.len().saturating_sub(ncandfqso) > 1 && config.rcandthin() < 0.99 {
        let keep = ncandfqso
            + ((out.len() - ncandfqso) as f32 * adjusted_rcandthin(config)).round() as usize;
        out.truncate(keep.min(out.len()));
    }
    out
}

fn adjusted_rcandthin(config: Sync8Config) -> f32 {
    let mut rcandthin = config.rcandthin();
    match config.ipass {
        1 | 4 | 7 => rcandthin = (rcandthin * 1.27).min(1.0),
        2 | 5 | 8 => {
            if rcandthin > 0.79 {
                rcandthin *= rcandthin;
            } else {
                rcandthin *= 0.79;
            }
        }
        _ => rcandthin = (rcandthin * 5.0).min(1.0),
    }
    rcandthin
}

fn suppress_near_dupes(candidates: &mut [SyncCandidate], config: Sync8Config) {
    let fdif0 = if config.swl { 3.0 } else { 4.0 };
    for i in 0..candidates.len() {
        for j in 0..i {
            let fdiff = (candidates[i].freq - candidates[j].freq).abs();
            let xdtdelta = (candidates[i].dt - candidates[j].dt).abs();
            if fdiff < fdif0
                && (candidates[i].freq - config.nfqso as f32).abs() > 3.0
                && xdtdelta < 0.1
            {
                if candidates[i].sync >= candidates[j].sync {
                    candidates[j].sync = 0.0;
                } else {
                    candidates[i].sync = 0.0;
                }
            }
        }
    }
}

fn sum_s(s: &[f32], ia: usize, ib: usize, j: usize) -> f32 {
    (ia..=ib).map(|i| s[s_idx(i, j)]).sum()
}

fn sum_s_stride(s: &[f32], ia: usize, ib: usize, step: usize, j: usize) -> f32 {
    (ia..=ib).step_by(step).map(|i| s[s_idx(i, j)]).sum()
}

fn s_idx(i: usize, j: usize) -> usize {
    debug_assert!((1..=NH1).contains(&i));
    debug_assert!((1..=NHSYM).contains(&j));
    (i - 1) * NHSYM + (j - 1)
}

fn windowx() -> [f32; 201] {
    let mut out = [0.0; 201];
    for (i, slot) in out.iter_mut().enumerate() {
        let facx = 1.0 / 300.0f32;
        *slot = facx * (1.0 + ((i as f32 * std::f32::consts::PI) / 200.0).cos()) / 2.0;
    }
    out
}
