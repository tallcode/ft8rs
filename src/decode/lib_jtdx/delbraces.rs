//! Mirrors JTDX `lib/delbraces.f90`.

pub(crate) fn delbraces(msg37: &str) -> String {
    let mut words: Vec<String> = msg37.split_whitespace().map(str::to_string).collect();
    for idx in 0..words.len().min(3) {
        if let Some(stripped) = strip_non_hash_braces(&words[idx]) {
            words[idx] = stripped;
            break;
        }
    }
    words.join(" ")
}

fn strip_non_hash_braces(word: &str) -> Option<String> {
    if !word.starts_with('<') || word.as_bytes().get(1) == Some(&b'.') {
        return None;
    }
    let end = word.find('>')?;
    let mut out = String::new();
    out.push_str(&word[1..end]);
    out.push_str(&word[end + 1..]);
    Some(out)
}
