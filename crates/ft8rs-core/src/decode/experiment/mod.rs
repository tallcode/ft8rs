//! Independent experimental receiver foundation, currently scheduled only by Hybrid.
//! No standalone profile or public decoder API is registered.
mod receiver;
pub(crate) use receiver::run as decode;
pub use receiver::ExperimentDiagnostics;
