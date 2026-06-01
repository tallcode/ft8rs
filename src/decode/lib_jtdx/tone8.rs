//! Mirrors JTDX `lib/tone8.f90`.

use super::ft8v2::packjt77sd::genft8sd;
use super::gen_ft8wave::gen_ft8wave;
use crate::stream::session::StreamDecodeConfig;

#[derive(Clone, Debug)]
pub struct CsyncE {
    pub re: [[f64; 32]; 19],
    pub im: [[f64; 32]; 19],
}

impl Default for CsyncE {
    fn default() -> Self {
        Self {
            re: [[0.0; 32]; 19],
            im: [[0.0; 32]; 19],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Tone8Tables {
    pub(crate) csynce: Option<CsyncE>,
    pub(crate) idtone25_2: Option<[i32; 58]>,
    pub(crate) idtonemyc: Option<[i32; 58]>,
    pub(crate) idtone56: Vec<[i32; 58]>,
    pub(crate) msg56: Vec<String>,
    pub(crate) msgbits56: Vec<[u8; 77]>,
    pub(crate) itone56: Vec<[i32; 79]>,
    pub(crate) idtonecqdxcns: Option<[i32; 58]>,
    pub(crate) idtonedxcns73: Option<[i32; 58]>,
    pub(crate) idtonefox73: Option<[i32; 58]>,
    pub(crate) idtonespec: Option<[i32; 58]>,
}

pub(crate) fn tone8(config: &StreamDecodeConfig) -> Tone8Tables {
    let mut tables = Tone8Tables {
        idtone25_2: tones58_from_sd_message("CQ 2E0DLA IO92"),
        ..Tone8Tables::default()
    };

    let Some(mycall) = normalized_call(config.mycall.as_deref()) else {
        return tables;
    };
    tables.idtonemyc = tones58_from_sd_message(&format!("{mycall} AA1AAA FN25"));

    let Some(hiscall) = normalized_call(config.hiscall.as_deref()) else {
        return tables;
    };

    if config.lhound {
        if let (Some(mybcall), Some(hisbcall)) = (base_call(&mycall), base_call(&hiscall)) {
            tables.idtonefox73 = tones58_from_sd_message(&format!("{mybcall} {hisbcall} RR73"));
            tables.idtonespec =
                tones58_from_sd_message(&format!("{mybcall} RR73; {mybcall} <{hiscall}> -12"));
        }
    }

    if is_nonstandard_call(&hiscall) {
        let hiscall_raw = config
            .hiscall
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        tables.idtonecqdxcns = tones58_from_sd_message(&format!("CQ {hiscall_raw}"));
        tables.idtonedxcns73 = tones58_from_sd_message(&format!("<AA1AAA> {hiscall_raw} 73"));
    }

    if is_nonstandard_call(&mycall) && is_nonstandard_call(&hiscall) {
        return tables;
    }

    fill_idtone56(&mut tables, &mycall, &hiscall);
    tables.csynce = build_csynce(&mycall, &hiscall);
    tables
}

fn fill_idtone56(tables: &mut Tone8Tables, mycall: &str, hiscall: &str) {
    const RPT: [&str; 56] = [
        "-01", "-02", "-03", "-04", "-05", "-06", "-07", "-08", "-09", "-10", "-11", "-12", "-13",
        "-14", "-15", "-16", "-17", "-18", "-19", "-20", "-21", "-22", "-23", "-24", "-25", "-26",
        "R-01", "R-02", "R-03", "R-04", "R-05", "R-06", "R-07", "R-08", "R-09", "R-10", "R-11",
        "R-12", "R-13", "R-14", "R-15", "R-16", "R-17", "R-18", "R-19", "R-20", "R-21", "R-22",
        "R-23", "R-24", "R-25", "R-26", "AA00", "RRR", "RR73", "73",
    ];

    tables.idtone56.clear();
    tables.msg56.clear();
    tables.msgbits56.clear();
    tables.itone56.clear();

    for rpt in RPT {
        let msg = format!("{} {} {}", mycall.trim(), hiscall.trim(), rpt);
        let Some((msg37, msgbits, itone)) = genft8sd(&msg) else {
            continue;
        };
        tables.idtone56.push(tones58_from_itone(&itone));
        tables.msg56.push(msg37);
        tables.msgbits56.push(msgbits);
        tables.itone56.push(itone);
    }
}

fn build_csynce(mycall: &str, hiscall: &str) -> Option<CsyncE> {
    let msg = format!("{} {} -01", mycall.trim(), hiscall.trim());
    let (_, _, itone) = genft8sd(&msg)?;
    let (wave_re, wave_im) = gen_ft8wave(&itone, 0.0);
    let mut out = CsyncE::default();
    let mut m = 7 * 1920;
    for j in 0..19 {
        for k in 0..32 {
            out.re[j][k] = wave_re[m];
            out.im[j][k] = wave_im[m];
            m += 60;
        }
    }
    Some(out)
}

fn tones58_from_sd_message(msg: &str) -> Option<[i32; 58]> {
    let (_, _, itone) = genft8sd(msg)?;
    Some(tones58_from_itone(&itone))
}

fn tones58_from_itone(itone: &[i32; 79]) -> [i32; 58] {
    let mut out = [0i32; 58];
    out[..29].copy_from_slice(&itone[7..36]);
    out[29..].copy_from_slice(&itone[43..72]);
    out
}

fn normalized_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim().trim_start_matches('<').trim_end_matches('>');
    if call.len() < 3 {
        return None;
    }
    Some(call.to_ascii_uppercase())
}

fn base_call(call: &str) -> Option<String> {
    let call = call.trim().trim_start_matches('<').trim_end_matches('>');
    let base = call
        .split('/')
        .filter(|part| part.len() >= 3)
        .max_by_key(|part| (part.as_bytes().iter().any(u8::is_ascii_digit), part.len()))?;
    Some(base.to_ascii_uppercase())
}

fn is_nonstandard_call(call: &str) -> bool {
    let call = call.trim();
    if call.is_empty() {
        return false;
    }
    call.starts_with('<')
        || call.ends_with('>')
        || call.contains('/')
        || call.len() > 6
        || !call
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}
