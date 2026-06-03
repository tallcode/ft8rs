use super::super::sync8::SyncCandidate;
use super::state::Ft8bCandidateContext;
use crate::stream::session::StreamDecodeConfig;

#[derive(Clone, Copy, Debug)]
pub(super) struct QsoPlan {
    pub(super) nqso: usize,
    pub(super) xdt0: f32,
    pub(super) lvirtual2: bool,
    pub(super) lvirtual3: bool,
}

pub(super) fn qso_attempts(plan: QsoPlan) -> Vec<usize> {
    if plan.lvirtual2 {
        return vec![2];
    }
    if plan.lvirtual3 {
        return vec![2, 3];
    }
    match plan.nqso {
        2 => vec![1, 2],
        3 => vec![1, 2, 3],
        4 => vec![1, 4],
        _ => vec![1],
    }
}

pub(super) fn jtdx_qso_plan(
    config: &StreamDecodeConfig,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
) -> QsoPlan {
    let mut plan = QsoPlan {
        nqso: 1,
        xdt0: candidate.dt,
        lvirtual2: false,
        lvirtual3: false,
    };
    let lqsothread = config.nfqso >= config.nfa && config.nfqso <= config.nfb;
    let qso_thread_active = lqsothread && !context.lft8sdec;
    let qso_thread_has_hiscall =
        qso_thread_active && config.hiscall.as_deref().unwrap_or("").trim().len() >= 3;

    let fdelta = (candidate.freq as f64 - config.nfqso).abs();
    if qso_thread_active {
        if !context.lqsomsgdcd
            && !context.stophint
            && (1..=4).contains(&context.nlasttx)
            && fdelta < 2.51
        {
            if let Some(last_xdt) = context.last_rx_xdt {
                if (last_xdt - candidate.dt).abs() < 0.18 {
                    plan.nqso = 2;
                }
            } else {
                plan.nqso = 2;
            }
        }

        if !qso_thread_has_hiscall || context.lqsomsgdcd || context.stophint || fdelta >= 0.1 {
            if context.sd_msg.is_some() && plan.nqso == 1 {
                plan.nqso = 4;
            }
            return plan;
        }

        let mut maxlasttx = 4;
        if candidate.dt.abs() > 4.9 && context.last_rx_is_rrr {
            maxlasttx = 5;
        }
        if !(1..=maxlasttx).contains(&context.nlasttx) {
            if context.sd_msg.is_some() && plan.nqso == 1 {
                plan.nqso = 4;
            }
            return plan;
        }

        if candidate.dt > 4.9 {
            if let Some(last_xdt) = context.last_rx_xdt {
                plan.xdt0 = last_xdt;
                plan.nqso = 2;
                plan.lvirtual2 = true;
            } else if let Some(call_dt) = context.call_dt_xdt {
                plan.xdt0 = call_dt;
                plan.nqso = 3;
                plan.lvirtual2 = true;
            }
        } else if candidate.dt < -4.9 {
            if let Some(last_xdt) = context.last_rx_xdt {
                plan.xdt0 = last_xdt;
                plan.nqso = 3;
                plan.lvirtual3 = true;
            } else if let Some(call_dt) = context.call_dt_xdt {
                plan.xdt0 = call_dt;
                plan.nqso = 3;
                plan.lvirtual3 = true;
            }
        }
    }

    if !context.levenint && !context.loddint {
        plan.lvirtual2 = false;
        plan.lvirtual3 = false;
    }
    if context.sd_msg.is_some() && plan.nqso == 1 {
        plan.nqso = 4;
    }

    plan
}

#[allow(dead_code)]
pub(super) fn jtdx_nqso(
    config: &StreamDecodeConfig,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
) -> usize {
    jtdx_qso_plan(config, candidate, context).nqso
}
