//! Independent receiver scheduled exclusively as a supplemental Hybrid branch.
//! No standalone profile, public decoder API or persistent native state.
mod receiver;

use crate::stream::session::StreamDecodedMessage;
use std::time::Instant;

pub use receiver::ExperimentDiagnostics;

/// Immutable slot inputs. Baseline rows supply original tones, never re-encoded
/// display text. They are nuisance models, not independent confirmation of exp.
pub(crate) struct ExperimentInput<'a> {
    pub samples: &'a [f32],
    pub baseline: &'a [StreamDecodedMessage],
    pub low: f64,
    pub high: f64,
    pub deadline: Instant,
}

pub(crate) fn decode(
    input: ExperimentInput<'_>,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> ExperimentDiagnostics {
    receiver::run(input, emit)
}
