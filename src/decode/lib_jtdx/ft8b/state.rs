use super::super::ft8_downsample::{DownsampleOutput, DownsampleWorkspace};
use super::super::ft8_mod1::{NUM_CQ_SIG, NUM_DEC_CQ, NUM_DEC_MYC, NUM_MYC_SIG};
use super::super::ft8v2::bpdecode174_91::N;

#[derive(Clone, Copy, Debug)]
pub struct LastRxMsgText {
    bytes: [u8; 37],
    len: usize,
}

impl LastRxMsgText {
    pub fn from_str(value: &str) -> Self {
        let mut bytes = [b' '; 37];
        let src = value.as_bytes();
        let len = src.len().min(bytes.len());
        bytes[..len].copy_from_slice(&src[..len]);
        Self { bytes, len }
    }

    pub(super) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ft8bCandidateContext {
    pub ipass: usize,
    pub npass: usize,
    pub lsubtract: bool,
    pub lhighsens: bool,
    pub lcqcand: bool,
    pub levenint: bool,
    pub loddint: bool,
    pub lqsomsgdcd: bool,
    pub lft8sdec: bool,
    pub stophint: bool,
    pub nlasttx: usize,
    pub call_dt_xdt: Option<f32>,
    pub sd_msg: Option<LastRxMsgText>,
    pub sd_lcq: bool,
    pub sd_index: Option<usize>,
    pub last_rx_msg: Option<LastRxMsgText>,
    pub last_rx_xdt: Option<f32>,
    pub last_rx_is_rrr: bool,
}

#[derive(Clone, Debug)]
pub struct Ft8bDecodeResult {
    pub msg37: String,
    pub msg37_2: String,
    pub l_free_text: bool,
    pub l_special: bool,
    pub snr: f32,
    pub freq: f32,
    pub dt: f32,
    pub iaptype: i32,
    pub i3: i32,
    pub n3: i32,
    pub itone: [i32; 79],
    pub source: DecodeSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    Regular,
    Ft8s,
    Ft8sd,
}

#[derive(Clone, Debug)]
pub(super) struct SymbolMetrics {
    #[allow(dead_code)]
    pub(super) s8: [[f32; 79]; 8],
    pub(super) cs_re: [[f32; 79]; 8],
    pub(super) cs_im: [[f32; 79]; 8],
    pub(super) csr_re: [[f32; 79]; 8],
    pub(super) csr_im: [[f32; 79]; 8],
    pub(super) cscs_re: [[f32; 79]; 8],
    pub(super) cscs_im: [[f32; 79]; 8],
    pub(super) s256: [f32; 27],
    #[allow(dead_code)]
    pub(super) syncavemax: f32,
    pub(super) nsync: usize,
    pub(super) nsync2: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CsMatrix {
    pub(super) re: [[f32; 79]; 8],
    pub(super) im: [[f32; 79]; 8],
}

#[derive(Clone, Debug)]
pub(super) struct BitMetrics {
    pub(super) bmeta: [f32; N],
    pub(super) bmetb: [f32; N],
    pub(super) bmetc: [f32; N],
    pub(super) bmetd: [f32; N],
}

#[derive(Clone, Debug, Default)]
pub(super) struct ToneHints {
    pub(super) idtone25_2: Option<[i32; 58]>,
    pub(super) idtonemyc: Option<[i32; 58]>,
    pub(super) idtone56: Vec<[i32; 58]>,
    pub(super) idtonecqdxcns: Option<[i32; 58]>,
    pub(super) idtonedxcns73: Option<[i32; 58]>,
    pub(super) idtonefox73: Option<[i32; 58]>,
    pub(super) idtonespec: Option<[i32; 58]>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SignalClassifier {
    pub(super) lcqsignal: bool,
    pub(super) lmycsignal: bool,
    pub(super) lqsosig: bool,
    pub(super) lqsosigtype3: bool,
    pub(super) lqsocandave: bool,
    pub(super) lqso73: bool,
    pub(super) lqsorr73: bool,
    pub(super) lqsorrr: bool,
    pub(super) ldxcsig: bool,
    pub(super) lcqdxcsig: bool,
    pub(super) lcqdxcnssig: bool,
    pub(super) nmic: usize,
    pub(super) nweak: usize,
    pub(super) nsubpasses: usize,
    pub(super) scqnr: f32,
    pub(super) smycnr: f32,
    pub(super) lfoxspecrpt: bool,
    pub(super) lfoxstdr73: bool,
    pub(super) nfoxspecrpt: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SyncGate {
    pub(super) lapcqonly: bool,
    pub(super) lskipnotap: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MetricSource {
    Cs,
    Csr,
    CscsCsrPower,
    CsCsoldPower,
    CsCsoldSum,
}

#[derive(Clone, Debug)]
struct SignalEntry {
    freq: f32,
    xdt: f32,
    cs: CsMatrix,
}

#[derive(Clone, Copy, Debug)]
struct DecodedSignalEntry {
    freq: f32,
    xdt: f32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SignalMemory {
    tmpcqdec: Vec<DecodedSignalEntry>,
    tmpmyc: Vec<DecodedSignalEntry>,
    evencq: Vec<SignalEntry>,
    oddcq: Vec<SignalEntry>,
    evenmyc: Vec<SignalEntry>,
    oddmyc: Vec<SignalEntry>,
    evenqso: Option<SignalEntry>,
    oddqso: Option<SignalEntry>,
    tmpcqsig: Vec<SignalEntry>,
    tmpmycsig: Vec<SignalEntry>,
    tmpqsosig: Option<SignalEntry>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum SignalKind {
    Cq,
    MyCall,
    Qso,
}

#[derive(Debug)]
pub struct Ft8bWorkspace {
    pub(super) downsample: DownsampleWorkspace,
    pub(super) downsample_out: DownsampleOutput,
    pub(super) freqsub: Vec<f32>,
    pub(super) npos: usize,
    pub(super) lsubtracted: bool,
    pub(super) signal_memory: SignalMemory,
}

impl Default for Ft8bWorkspace {
    fn default() -> Self {
        Self {
            downsample: DownsampleWorkspace::new(),
            downsample_out: DownsampleOutput::default(),
            freqsub: vec![0.0; 200],
            npos: 0,
            lsubtracted: false,
            signal_memory: SignalMemory::default(),
        }
    }
}

impl Ft8bWorkspace {
    pub fn begin_slot(&mut self) {
        self.signal_memory.tmpcqsig.clear();
        self.signal_memory.tmpmycsig.clear();
        self.signal_memory.tmpqsosig = None;
        self.signal_memory.tmpcqdec.clear();
        self.signal_memory.tmpmyc.clear();
    }

    pub fn finish_slot(&mut self, levenint: bool, loddint: bool, lapmyc: bool, lqsomsgdcd: bool) {
        self.signal_memory
            .finish_slot(levenint, loddint, lapmyc, lqsomsgdcd);
    }

    pub fn new_pass(&mut self) {
        // JTDX ft8_decode.f90 resets npos at each ipass, while lsubtracted
        // is initialized once before the pass loop and then carried across it.
        self.npos = 0;
    }

    pub(crate) fn remember_decoded_message(
        &mut self,
        msg37: &str,
        freq: f32,
        xdt: f32,
        mycall: &str,
        lmycallstd: bool,
    ) {
        self.signal_memory
            .remember_decoded_message(msg37, freq, xdt, mycall, lmycallstd);
    }
}

impl SignalMemory {
    pub(super) fn finish_slot(
        &mut self,
        levenint: bool,
        loddint: bool,
        lapmyc: bool,
        lqsomsgdcd: bool,
    ) {
        if levenint {
            copy_signal_prefix(&mut self.evencq, &self.tmpcqsig);
            if lapmyc {
                copy_signal_prefix(&mut self.evenmyc, &self.tmpmycsig);
                if !lqsomsgdcd && self.tmpqsosig.is_some() {
                    self.evenqso = self.tmpqsosig.clone();
                }
            }
        } else if loddint {
            copy_signal_prefix(&mut self.oddcq, &self.tmpcqsig);
            if lapmyc {
                copy_signal_prefix(&mut self.oddmyc, &self.tmpmycsig);
                if !lqsomsgdcd && self.tmpqsosig.is_some() {
                    self.oddqso = self.tmpqsosig.clone();
                }
            }
        }
        self.tmpcqsig.clear();
        self.tmpmycsig.clear();
        self.tmpqsosig = None;
    }

    pub(super) fn find_old(
        &self,
        kind: SignalKind,
        context: Ft8bCandidateContext,
        freq: f64,
        xdt: f64,
    ) -> Option<CsMatrix> {
        let entries: &[SignalEntry] = match (kind, context.levenint, context.loddint) {
            (SignalKind::Cq, true, _) => &self.evencq,
            (SignalKind::Cq, _, true) => &self.oddcq,
            (SignalKind::MyCall, true, _) => &self.evenmyc,
            (SignalKind::MyCall, _, true) => &self.oddmyc,
            (SignalKind::Qso, true, _) => return self.match_one(&self.evenqso, freq, xdt),
            (SignalKind::Qso, _, true) => return self.match_one(&self.oddqso, freq, xdt),
            _ => return None,
        };
        entries
            .iter()
            .find(|entry| {
                (entry.freq as f64 - freq).abs() < 2.0 && (entry.xdt as f64 - xdt).abs() < 0.05
            })
            .map(|entry| entry.cs.clone())
    }

    pub(super) fn has_decoded_tmp(&self, kind: SignalKind, freq: f64, xdt: f64) -> bool {
        let entries = match kind {
            SignalKind::Cq => &self.tmpcqdec,
            SignalKind::MyCall => &self.tmpmyc,
            SignalKind::Qso => return false,
        };
        entries.iter().any(|entry| {
            (entry.freq as f64 - freq).abs() < 5.0 && (entry.xdt as f64 - xdt).abs() < 0.05
        })
    }

    fn remember_decoded_message(
        &mut self,
        msg37: &str,
        freq: f32,
        xdt: f32,
        mycall: &str,
        lmycallstd: bool,
    ) {
        let msg37 = msg37.trim();
        if msg37.starts_with("CQ ") && self.tmpcqdec.len() < NUM_DEC_CQ {
            self.tmpcqdec.push(DecodedSignalEntry { freq, xdt });
        }

        if lmycallstd
            && !mycall.trim().is_empty()
            && msg37
                .split_whitespace()
                .next()
                .is_some_and(|call| call == mycall.trim())
            && self.tmpmyc.len() < NUM_DEC_MYC
        {
            self.tmpmyc.push(DecodedSignalEntry { freq, xdt });
        }
    }

    fn match_one(&self, entry: &Option<SignalEntry>, freq: f64, xdt: f64) -> Option<CsMatrix> {
        entry
            .as_ref()
            .filter(|entry| {
                (entry.freq as f64 - freq).abs() < 2.0 && (entry.xdt as f64 - xdt).abs() < 0.05
            })
            .map(|entry| entry.cs.clone())
    }

    pub(super) fn remember_tmp(&mut self, kind: SignalKind, freq: f64, xdt: f64, cs: CsMatrix) {
        let entry = SignalEntry {
            freq: freq as f32,
            xdt: xdt as f32,
            cs,
        };
        match kind {
            SignalKind::Cq => {
                if self.tmpcqsig.len() < NUM_CQ_SIG {
                    self.tmpcqsig.push(entry);
                }
            }
            SignalKind::MyCall => {
                if self.tmpmycsig.len() < NUM_MYC_SIG {
                    self.tmpmycsig.push(entry);
                }
            }
            SignalKind::Qso => {
                self.tmpqsosig = Some(entry);
            }
        }
    }
}

fn copy_signal_prefix(dst: &mut Vec<SignalEntry>, src: &[SignalEntry]) {
    for (idx, entry) in src.iter().cloned().enumerate() {
        if let Some(slot) = dst.get_mut(idx) {
            *slot = entry;
        } else {
            dst.push(entry);
        }
    }
}
