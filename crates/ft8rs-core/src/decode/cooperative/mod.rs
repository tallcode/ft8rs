//! Shared orchestration primitives. No decoder-private state crosses this boundary.
pub(crate) mod ledger;
pub(crate) mod message;
pub(crate) mod recovery;

pub(crate) use ledger::{same_signal, OutputLedger, SignalSource};
pub(crate) use message::{is_grid4, normalize_message, normalize_word, ParsedMessage};
