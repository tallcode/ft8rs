//! Mirrors JTDX `lib/ft8_mod1.f90`.

use super::ft8_params::KK;

pub const NPS: usize = 180_000;
pub const NFR: usize = 151_680;
pub const NFILT1: usize = 4000;
pub const NFILT2: usize = 3400;
pub const NUM_CQ_SIG: usize = 20;
pub const NUM_DEC_CQ: usize = 40;
pub const NUM_MYC_SIG: usize = 5;
pub const NUM_DEC_MYC: usize = 25;
pub const NMAX_THREADS: usize = 24;

pub const ICOS7: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
pub const GRAYMAP: [i32; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
pub const MASK_INCALL_THR: [i32; 25] = [
    0, 30, 45, 55, 65, 75, 85, 90, 95, 100, 105, 110, 115, 120, 125, 130, 135, 140, 145, 150, 155,
    160, 165, 170, 175,
];
pub const MCQ: [i32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
];
pub const MRRR: [i32; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1];
pub const M73: [i32; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 1];
pub const MRR73: [i32; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0, 1];
pub const NAPPASSES: [usize; 6] = [27, 27, 27, 27, 27, 27];
pub const N_LDPC: usize = 174;

pub const NAPTYPES: [[i32; 27]; 6] = [
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        3, 3, 3, 6, 6, 6, 5, 5, 5, 4, 4, 4, 0, 0, 0, 0, 0, 0, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        3, 3, 3, 6, 6, 6, 5, 5, 5, 4, 4, 4, 2, 2, 2, 0, 0, 0, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
];

pub const NMYCNSAPTYPES: [[i32; 27]; 6] = [
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 40, 40, 40, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        41, 41, 41, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        41, 41, 41, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        41, 41, 41, 44, 44, 44, 43, 43, 43, 42, 42, 42, 0, 0, 0, 0, 0, 0, 31, 31, 31, 36, 36, 36,
        35, 35, 35,
    ],
    [
        41, 41, 41, 44, 44, 44, 43, 43, 43, 42, 42, 42, 40, 40, 40, 0, 0, 0, 31, 31, 31, 36, 36,
        36, 35, 35, 35,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 40, 40, 40, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
];

pub const NHAPTYPES: [[i32; 27]; 6] = [
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 111, 111, 111,
    ],
    [
        21, 21, 21, 22, 22, 22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 31, 31, 31, 0, 0, 0, 36, 36, 36, 0, 0, 0,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        21, 21, 21, 22, 22, 22, 23, 23, 23, 24, 24, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

pub const NDXNSAPTYPES: [[i32; 27]; 6] = [
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        11, 11, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        11, 11, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
    [
        11, 11, 11, 14, 14, 14, 13, 13, 13, 12, 12, 12, 0, 0, 0, 0, 0, 0, 31, 31, 31, 36, 36, 36,
        35, 35, 35,
    ],
    [
        11, 11, 11, 14, 14, 14, 13, 13, 13, 12, 12, 12, 2, 2, 2, 0, 0, 0, 31, 31, 31, 36, 36, 36,
        35, 35, 35,
    ],
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 1, 1, 1, 31, 31, 31, 36, 36, 36, 35, 35, 35,
    ],
];

/// JTDX FT8 decoder state.
///
/// This replaces the Fortran module-global state with an owned Rust state
/// object. Fields will be added as the JTDX source slices are ported.
#[derive(Debug)]
pub struct Ft8Mod1 {
    pub dd8: Vec<f32>,
    pub allmessages: Vec<String>,
    pub allsnrs: Vec<i32>,
    pub allfreq: Vec<f32>,
    pub mycall: String,
    pub hiscall: String,
    pub hisgrid4: String,
    pub nft8cycles: usize,
    pub nft8swlcycles: usize,
    pub nft8rxfsens: usize,
    pub ndecodes: usize,
    pub nmsg: usize,
    pub nfawide: i32,
    pub nfbwide: i32,
    pub avexdt: f32,
    pub forcedt: f32,
    pub lagcc: bool,
    pub lagccbail: bool,
    pub lhound: bool,
    pub lqsomsgdcd: bool,
    pub lft8sdec: bool,
    pub lasthcall: String,
    pub lastrxmsg: LastRxMsg,
    pub calldteven: Vec<CallSignDt>,
    pub calldtodd: Vec<CallSignDt>,
    pub incall: Vec<InCall>,
    pub evencopy: Vec<OddEvenMessage>,
    pub oddcopy: Vec<OddEvenMessage>,
    pub even: Vec<OddEvenMessage>,
    pub odd: Vec<OddEvenMessage>,
    pub msgsrcvd: Vec<String>,
    pub lrepliedother: bool,
    pub first_osd: bool,
    pub nintcount: i32,
    pub gen: Vec<i8>,
}

impl Default for Ft8Mod1 {
    fn default() -> Self {
        Self {
            dd8: vec![0.0; NPS],
            allmessages: vec![String::new(); 200],
            allsnrs: vec![0; 200],
            allfreq: vec![0.0; 200],
            mycall: String::new(),
            hiscall: String::new(),
            hisgrid4: String::new(),
            nft8cycles: 3,
            nft8swlcycles: 3,
            nft8rxfsens: 3,
            ndecodes: 0,
            nmsg: 0,
            nfawide: 0,
            nfbwide: 0,
            avexdt: 0.0,
            forcedt: 0.0,
            lagcc: false,
            lagccbail: false,
            lhound: false,
            lqsomsgdcd: false,
            lft8sdec: false,
            lasthcall: String::new(),
            lastrxmsg: LastRxMsg::default(),
            calldteven: vec![CallSignDt::default(); 150],
            calldtodd: vec![CallSignDt::default(); 150],
            incall: vec![InCall::default(); 30],
            evencopy: vec![OddEvenMessage::default(); 130],
            oddcopy: vec![OddEvenMessage::default(); 130],
            even: vec![OddEvenMessage::default(); 130],
            odd: vec![OddEvenMessage::default(); 130],
            msgsrcvd: vec![String::new(); 130],
            lrepliedother: false,
            first_osd: true,
            nintcount: 0,
            gen: vec![0; KK * N_LDPC],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LastRxMsg {
    pub xdt: f32,
    pub lstate: bool,
    pub lastmsg: String,
}

#[derive(Clone, Debug, Default)]
pub struct CallSignDt {
    pub dt: f32,
    pub call2: String,
}

#[derive(Clone, Debug, Default)]
pub struct InCall {
    pub xdt: f32,
    pub msg: String,
}

#[derive(Clone, Debug, Default)]
pub struct OddEvenMessage {
    pub freq: f32,
    pub dt: f32,
    pub lstate: bool,
    pub msg: String,
}
