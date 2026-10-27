#[derive(Clone, Debug)]
pub(super) struct DxTarget {
    call: String,
}

impl DxTarget {
    pub(super) fn new(call: &str) -> Self {
        Self {
            call: normalize_call(call),
        }
    }

    pub(super) fn matches_message(&self, msg: &str) -> bool {
        if self.call.is_empty() {
            return false;
        }
        msg.split_whitespace()
            .map(normalize_message_word)
            .any(|word| word == self.call)
    }

    pub(super) fn matches_word(&self, word: &str) -> bool {
        !self.call.is_empty() && normalize_message_word(word) == self.call
    }
}

fn normalize_call(call: &str) -> String {
    call.trim().to_ascii_uppercase()
}

use crate::decode::cooperative::normalize_word as normalize_message_word;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_matches_plain_and_resolved_braced_call() {
        let target = DxTarget::new("ea5/dh0yah");

        assert!(target.matches_message("RK4FF EA5/DH0YAH 73"));
        assert!(target.matches_message("<EA5/DH0YAH> RK4FF RR73"));
        assert!(target.matches_message("EA5/DH0YAH, RK4FF RR73"));
    }

    #[test]
    fn target_does_not_match_substrings_or_unresolved_hash() {
        let target = DxTarget::new("K1ABC");

        assert!(!target.matches_message("CQ K1ABCDE FN42"));
        assert!(!target.matches_message("<...> K1XYZ -10"));
    }
}
