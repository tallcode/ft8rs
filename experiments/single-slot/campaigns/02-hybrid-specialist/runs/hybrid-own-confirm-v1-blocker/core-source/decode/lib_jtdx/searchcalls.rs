//! Mirrors JTDX `lib/searchcalls.f90`.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

static CALLS: OnceLock<Option<HashSet<String>>> = OnceLock::new();

pub(crate) fn searchcalls(callsign1: &str, callsign2: &str) -> bool {
    let Some(calls) = calls() else {
        return true;
    };

    let callsign1 = callsign1.trim();
    let mut callsign2 = callsign2.trim();

    if callsign1.len() > 7 && callsign2.is_empty() {
        return true;
    }
    if callsign2.len() > 7 {
        callsign2 = "";
    }

    if contains_call(calls, callsign1) {
        return true;
    }
    !callsign2.is_empty() && contains_call(calls, callsign2)
}

fn contains_call(calls: &HashSet<String>, callsign: &str) -> bool {
    if callsign.is_empty() || callsign == "TU;" {
        return false;
    }
    calls.contains(callsign)
}

fn calls() -> Option<&'static HashSet<String>> {
    CALLS
        .get_or_init(|| allcall7_text().map(|text| parse_allcall7(&text)))
        .as_ref()
}

fn allcall7_text() -> Option<String> {
    allcall7_paths()
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
}

fn allcall7_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("ALLCALL7.TXT"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("ALLCALL7.TXT"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ALLCALL7.TXT"));
    paths
}

fn parse_allcall7(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|line| {
            let call = line.trim().trim_end_matches(',');
            if call.is_empty() || call.starts_with("//") {
                None
            } else {
                Some(call.to_string())
            }
        })
        .collect()
}
