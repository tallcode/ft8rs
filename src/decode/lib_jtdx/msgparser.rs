//! Mirrors JTDX `lib/msgparser.f90`.

pub(crate) fn msgparser(msg37: &str) -> Option<(String, String)> {
    let words: Vec<&str> = msg37.split_whitespace().collect();
    if words.len() < 5 {
        return None;
    }

    let call1 = words[0];
    let call2 = words[2];
    let mut call3 = words[3].to_string();
    if call3.starts_with("<.") {
        call3 = "<...>".to_string();
    } else {
        call3 = call3
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string();
    }
    let report = words[4..].join(" ");

    Some((
        format!("{call1} {call3} RR73"),
        format!("{call2} {call3} {report}"),
    ))
}
