//! Interpret display messages conservatively; text does not prove a wire type.

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedMessage {
    pub sender: Option<String>,
    pub recipients: Vec<String>,
    pub grid: Option<String>,
    pub unresolved: bool,
}

impl ParsedMessage {
    pub fn parse(message: &str) -> Self {
        let words: Vec<_> = message.split_whitespace().map(normalize_word).collect();
        let mut parsed = Self {
            unresolved: words.iter().any(|w| w == "<...>"),
            ..Self::default()
        };
        let Some(first) = words.first() else {
            return parsed;
        };
        if matches!(first.as_str(), "CQ" | "DE" | "QRZ") {
            let sender = if words.get(1).is_some_and(|w| is_call(w)) {
                1
            } else {
                2
            };
            if let Some(call) = words.get(sender).filter(|w| is_call(w)) {
                parsed.sender = Some(call.clone());
                parsed.grid = words.get(sender + 1).filter(|w| is_grid4(w)).cloned();
            }
        } else if words.len() >= 5
            && words[1] == "RR73"
            && is_call(&words[0])
            && is_call(&words[2])
            && is_call(&words[3])
        {
            // WSJT-X DXpedition display: CALL1 RR73; CALL2 <FOX> REPORT.
            parsed.recipients = vec![words[0].clone(), words[2].clone()];
            parsed.sender = Some(words[3].clone());
        } else if words.len() >= 2 && is_call(first) && is_call(&words[1]) {
            parsed.recipients.push(first.clone());
            parsed.sender = Some(words[1].clone());
            parsed.grid = words.get(2).filter(|w| is_grid4(w)).cloned();
        }
        parsed
    }
}

pub(crate) fn normalize_word(word: &str) -> String {
    let word = word
        .trim_matches(|c: char| matches!(c, ';' | ','))
        .to_ascii_uppercase();
    if word.starts_with('<') && word.ends_with('>') && word != "<...>" && word.len() > 2 {
        word[1..word.len() - 1].to_string()
    } else {
        word
    }
}

pub(crate) fn normalize_message(message: &str) -> String {
    message
        .split_whitespace()
        .map(normalize_word)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_grid4(word: &str) -> bool {
    let b = word.as_bytes();
    word != "RR73"
        && b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
}

pub(crate) fn is_call(word: &str) -> bool {
    (3..=11).contains(&word.len())
        && !is_grid4(word)
        && !matches!(word, "RR73" | "RRR" | "CQ" | "QRZ" | "<...>")
        && word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'/')
        && word.bytes().any(|c| c.is_ascii_alphabetic())
        && word.bytes().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_and_rr73_are_never_grid() {
        for tail in ["RR73", "RRR", "73", "+05", "R-10"] {
            let p = ParsedMessage::parse(&format!("K1JT BG5ATV {tail}"));
            assert_eq!(p.sender.as_deref(), Some("BG5ATV"));
            assert!(p.grid.is_none());
        }
        assert!(!is_grid4("ZZ99"));
        assert!(!is_grid4("RR73"));
    }

    #[test]
    fn cq_modifiers_and_fox_roles_are_explicit() {
        let cq = ParsedMessage::parse("CQ DX <BG5ATV> PM00");
        assert_eq!(cq.sender.as_deref(), Some("BG5ATV"));
        assert_eq!(cq.grid.as_deref(), Some("PM00"));
        let fox = ParsedMessage::parse("K1ABC RR73; W9XYZ <DX1AAA> -10");
        assert_eq!(fox.sender.as_deref(), Some("DX1AAA"));
        assert_eq!(fox.recipients, ["K1ABC", "W9XYZ"]);
    }

    #[test]
    fn free_text_and_unresolved_hash_do_not_invent_sender() {
        assert!(ParsedMessage::parse("HELLO BG5ATV").sender.is_none());
        assert!(ParsedMessage::parse("K1JT <...> -10").unresolved);
        assert_ne!(
            normalize_message("K1JT <...> -10"),
            normalize_message("K1JT BG5ATV -10")
        );
    }
}
