use super::*;

fn dx(msg: &str) -> (Option<String>, Option<String>) {
    dx_call_and_grid(&norm_tokens(msg))
}

#[test]
fn double_click_extracts_cq_call_and_grid() {
    let both = (Some("F5BZB".to_string()), Some("IN94".to_string()));
    assert_eq!(dx("CQ F5BZB IN94"), both);
    // Directional CQ: skip DX / the pure-digit frequency, take the callsign.
    assert_eq!(dx("CQ DX F5BZB IN94"), both);
    assert_eq!(dx("CQ 072 F5BZB IN94"), both);
    // No grid present.
    assert_eq!(dx("CQ F5BZB"), (Some("F5BZB".to_string()), None));
}

#[test]
fn double_click_extracts_sender_from_directed_message() {
    // "TO FROM ..." — the DX is the sender (second token).
    let both = (Some("F5BZB".to_string()), Some("IN94".to_string()));
    assert_eq!(dx("W1FC F5BZB IN94"), both);
    assert_eq!(dx("W1FC F5BZB -08"), (Some("F5BZB".to_string()), None));
    assert_eq!(dx("W1FC F5BZB RR73"), (Some("F5BZB".to_string()), None));
}

#[test]
fn double_click_resolves_hash_braces_and_skips_unresolved() {
    assert_eq!(dx("K1ABC <F5BZB> RR73"), (Some("F5BZB".to_string()), None));
    // Unresolved hash: no callsign to load (freq still fills at the call site).
    assert_eq!(dx("K1ABC <...> RR73"), (None, None));
}

#[test]
fn grid4_is_structural_but_extraction_excludes_the_rr73_ack() {
    assert!(is_grid4("IN94"));
    assert!(is_grid4("RR99"));
    // Structurally not grid4.
    for t in ["-08", "R-12", "73", "F5BZB", "IN9", "IS94"] {
        assert!(!is_grid4(t), "{t} is not a grid4");
    }
    // RR73 is structurally grid4, but the ack must never load as a grid.
    assert!(is_grid4("RR73"));
    let rr73 = ["X".to_string(), "RR73".to_string()];
    let in94 = ["X".to_string(), "IN94".to_string()];
    assert_eq!(grid_after(&rr73, 1), None);
    assert_eq!(grid_after(&in94, 1), Some("IN94".to_string()));
}

#[test]
fn new_target_does_not_inherit_the_previous_grid() {
    let mut call = "W1AW".to_string();
    let mut grid = "FN31".to_string();
    fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC W1AW RR73"));
    assert_eq!(grid, "FN31");
    fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC F5BZB RR73"));
    assert_eq!(call, "F5BZB");
    assert_eq!(grid, "");
    fill_target_context(&mut call, &mut grid, &norm_tokens("CQ F5BZB IN94"));
    assert_eq!(grid, "IN94");
    fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC <...> FN42"));
    assert_eq!((call.as_str(), grid.as_str()), ("F5BZB", "IN94"));
}

#[test]
fn slot_identity_survives_source_delay_midnight_and_next_day() {
    let day = 20_000 * 86_400;
    let previous = absolute_slot(5759, day - 1);
    assert_eq!(previous, absolute_slot(5759, day + 2));
    assert_eq!(absolute_slot(0, day - 1), absolute_slot(0, day + 2));
    assert_ne!(absolute_slot(0, day), absolute_slot(0, day + 86_400));
    assert_eq!(
        absolute_slot(120, day + 1801),
        absolute_slot(120, day + 1817)
    );
}
