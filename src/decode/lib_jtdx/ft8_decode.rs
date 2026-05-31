//! Mirrors JTDX `lib/ft8_decode.f90`.

use crate::stream::session::StreamDecodeConfig;

use super::sync8::Sync8Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtractPolicy {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug)]
pub struct JtdxPass {
    pub ipass: usize,
    pub syncmin: f32,
    pub subtract: SubtractPolicy,
    pub sync8: Sync8Config,
}

pub fn npass(config: &StreamDecodeConfig) -> usize {
    let cycles = if config.swl {
        config.nft8swlcycles
    } else {
        config.nft8cycles
    };
    match cycles {
        1 => 3,
        2 => 6,
        3 => 9,
        _ => 3,
    }
}

pub fn syncmin(config: &StreamDecodeConfig, ipass: usize) -> f32 {
    let mut syncmin = 1.5f32;
    if config.lft8lowth || config.swl {
        match ipass {
            1 | 4 | 7 => syncmin = 1.225,
            2 | 5 | 8 => syncmin = 1.5,
            3 | 6 | 9 => syncmin = 1.1,
            _ => {}
        }
    }
    syncmin
}

pub fn jtdx_napwid(frequency_hz: f64) -> f64 {
    if frequency_hz < 30_000_000.0 {
        5.0
    } else if frequency_hz < 100_000_000.0 {
        15.0
    } else {
        50.0
    }
}

pub fn decode_passes(config: &StreamDecodeConfig, avexdt: f32) -> Vec<JtdxPass> {
    let npass = npass(config);
    (1..=npass)
        .map(|ipass| {
            let syncmin = syncmin(config, ipass);
            JtdxPass {
                ipass,
                syncmin,
                subtract: subtract_policy(config, ipass, npass),
                sync8: Sync8Config::from_stream(config, ipass, syncmin, avexdt),
            }
        })
        .collect()
}

pub fn subtract_policy(config: &StreamDecodeConfig, ipass: usize, npass: usize) -> SubtractPolicy {
    if ipass > 5 || (ipass == 3 && npass == 3 && !config.swl) {
        SubtractPolicy::Disabled
    } else {
        SubtractPolicy::Enabled
    }
}
