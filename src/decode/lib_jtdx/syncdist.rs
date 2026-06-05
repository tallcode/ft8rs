//! Mirrors JTDX `lib/syncdist.f90`.

use super::ft8_mod1::ICOS7;

pub(crate) fn sync_rank_distribution(s8: &[[f32; 79]; 8]) -> [usize; 8] {
    let mut nsmax = [0usize; 8];
    for k in 0..7 {
        syncdist(&mut nsmax, s8_column(s8, k), ICOS7[k] as usize);
        syncdist(&mut nsmax, s8_column(s8, k + 36), ICOS7[k] as usize);
        syncdist(&mut nsmax, s8_column(s8, k + 72), ICOS7[k] as usize);
    }
    nsmax
}

fn syncdist(nsmax: &mut [usize; 8], mut s81: [f32; 8], target: usize) {
    let ip = maxloc(&s81);
    if target == ip {
        nsmax[0] += 1;
    } else {
        s81[ip] = 0.0;
        let ip = maxloc(&s81);
        if target == ip {
            nsmax[1] += 1;
        } else {
            s81[ip] = 0.0;
            let ip = maxloc(&s81);
            if target == ip {
                nsmax[2] += 1;
            } else {
                s81[ip] = 0.0;
                let ip = maxloc(&s81);
                if target == ip {
                    nsmax[3] += 1;
                } else {
                    s81[ip] = 0.0;
                    let ip = maxloc(&s81);
                    if target == ip {
                        nsmax[4] += 1;
                    } else {
                        s81[ip] = 0.0;
                        let ip = maxloc(&s81);
                        if target == ip {
                            nsmax[5] += 1;
                        } else {
                            s81[ip] = 0.0;
                            let ip = maxloc(&s81);
                            if target == ip {
                                nsmax[6] += 1;
                            } else {
                                s81[ip] = 0.0;
                                let ip = maxloc(&s81);
                                if target == ip {
                                    nsmax[7] += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn s8_column(s8: &[[f32; 79]; 8], sym: usize) -> [f32; 8] {
    let mut s81 = [0.0f32; 8];
    for tone in 0..8 {
        s81[tone] = s8[tone][sym];
    }
    s81
}

fn maxloc(values: &[f32; 8]) -> usize {
    let mut best = 0usize;
    let mut best_value = values[0];
    for (idx, value) in values.iter().copied().enumerate().skip(1) {
        if value > best_value {
            best = idx;
            best_value = value;
        }
    }
    best
}
