use super::*;

#[cfg(test)]
mod tests {
    use super::is_stdcall;
    use super::HashCallBook;

    #[test]
    fn stdcall_matches_wsjtx_one_based_iarea() {
        assert!(is_stdcall("D1DX"));
        assert!(is_stdcall("R6KEE"));
        assert!(is_stdcall("F1PPH"));
        assert!(is_stdcall("IW1PUR"));
        assert!(is_stdcall("DL8YHR"));
        assert!(!is_stdcall("KN87"));
    }

    #[test]
    fn type1_r_grid_uses_third_word_like_wsjtx() {
        let bits = super::pack77("K1ABC W9XYZ R FN42");
        let msg = super::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "K1ABC W9XYZ R FN42");
    }

    #[test]
    fn report_tokens_are_not_standard_callsigns_for_split77() {
        assert!(!super::parse_callsign("RR73").is_standard);
        assert!(!super::parse_callsign("73").is_standard);
    }

    #[test]
    fn two_word_type1_rejects_second_call_slash_like_wsjtx() {
        assert!(super::try_pack_type1(&["K1ABC".into(), "W9XYZ/R".into()]).is_none());
    }

    #[test]
    fn split77_directed_cq_uses_wsjtx_chkcall_for_compound_call() {
        assert_eq!(
            super::split77("CQ DX PJ4/KA1ABC FN42"),
            vec!["CQ_DX", "PJ4/KA1ABC", "FN42"]
        );
    }

    #[test]
    fn directed_cq_dx_round_trips() {
        let bits = super::pack77("CQ DX DL8YHR JO41");
        let msg = super::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "CQ DX DL8YHR JO41");
    }

    #[test]
    fn type01_dxpedition_round_trips_with_hash10_book() {
        let book = HashCallBook::new();
        book.save("R5AF/O");
        let bits = super::pack77("RA3Y RR73; JR1FTJ <R5AF/O> +00");
        let msg = super::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "RA3Y RR73; JR1FTJ <R5AF/O> +00");
    }

    #[test]
    fn type03_field_day_round_trips() {
        let bits = super::pack77("WA9XYZ KA1ABC R 16A EMA");
        let msg = super::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "WA9XYZ KA1ABC R 16A EMA");
    }

    #[test]
    fn type05_telemetry_round_trips() {
        let bits = super::pack77("0123456789ABCDEF01");
        let msg = super::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "123456789ABCDEF01");
    }

    #[test]
    fn type3_rtty_round_trips() {
        let bits = super::pack77("TU; W9XYZ K1ABC R 579 MA");
        let msg = super::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "TU; W9XYZ K1ABC R 579 MA");
    }

    #[test]
    fn type5_eu_vhf_round_trips_with_hash_book() {
        let book = HashCallBook::new();
        book.save("K1ABC");
        book.save("G4ABC/P");
        let bits = super::pack77("<K1ABC> <G4ABC/P> R 590003 IO91NP");
        let msg = super::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "<K1ABC> <G4ABC/P> R 590003 IO91NP");
    }
}

#[cfg(test)]
mod unpack_tests {
    use super::unpack77;
    use super::HashCallBook;
    use super::{pack28, pack77, C38};

    fn append_bits(bits: &mut Vec<u8>, value: usize, width: usize) {
        for bit in (0..width).rev() {
            bits.push(((value >> bit) & 1) as u8);
        }
    }

    fn ihashcall(call: &str, width: usize) -> usize {
        let mut n8: u64 = 0;
        for c in format!("{:<11}", call.to_ascii_uppercase())
            .chars()
            .take(11)
        {
            let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
            n8 = 38 * n8 + j;
        }
        let prod = 47_055_833_459u64.wrapping_mul(n8);
        ((prod >> (64 - width as u32)) & ((1u64 << width as u32) - 1)) as usize
    }

    fn grid6_24(grid: &str) -> usize {
        let bytes = grid.as_bytes();
        (bytes[0] - b'A') as usize * 18 * 10 * 10 * 24 * 24
            + (bytes[1] - b'A') as usize * 10 * 10 * 24 * 24
            + (bytes[2] - b'0') as usize * 10 * 24 * 24
            + (bytes[3] - b'0') as usize * 24 * 24
            + (bytes[4] - b'A') as usize * 24
            + (bytes[5] - b'A') as usize
    }

    #[test]
    fn cq_r_grid_is_rejected_like_wsjtx() {
        let bits = pack77("CQ K1ABC R FN42");
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn cq_ack_report_is_rejected_like_wsjtx() {
        let bits = pack77("CQ K1ABC RRR");
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn cq_unresolved_hash_is_rejected_like_wsjtx() {
        let bits = pack77("CQ <NOHASH>");
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn unpack28_rejects_invalid_standard_call_like_wsjtx_callok() {
        assert!(super::unpack28(2_063_592 + 4_194_304, super::UnpackContext::default()).is_none());
    }

    #[test]
    fn unpacks_type_01_dxpedition_rr73_semicolon_message() {
        let book = HashCallBook::new();
        book.save("R5AF/O");
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("RA3Y"), 28);
        append_bits(&mut bits, pack28("JR1FTJ"), 28);
        append_bits(&mut bits, ihashcall("R5AF/O", 10), 10);
        append_bits(&mut bits, 15, 5); // +00 => (0 + 30) / 2
        append_bits(&mut bits, 1, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(
            unpack77(&bits, Some(&book)).unwrap(),
            "RA3Y RR73; JR1FTJ <R5AF/O> +00"
        );
    }

    #[test]
    fn type01_uses_hiscall_hash10_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("RA3Y"), 28);
        append_bits(&mut bits, pack28("JR1FTJ"), 28);
        append_bits(&mut bits, ihashcall("R5AF/O", 10), 10);
        append_bits(&mut bits, 15, 5);
        append_bits(&mut bits, 1, 3);
        append_bits(&mut bits, 0, 3);
        let context = super::UnpackContext::with_calls(None, None, Some("R5AF/O"));
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "RA3Y RR73; JR1FTJ <R5AF/O> +00"
        );
    }

    #[test]
    fn type1_uses_mycall_hash22_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, super::N_TOKENS + ihashcall("K1ABC", 22), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, pack28("W9XYZ"), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, super::MAXGRID4 + 1, 15);
        append_bits(&mut bits, 1, 3);
        let context = super::UnpackContext::with_calls(None, Some("K1ABC"), None);
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "<K1ABC> W9XYZ"
        );
    }

    #[test]
    fn unpacks_type_03_field_day_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("WA9XYZ"), 28);
        append_bits(&mut bits, pack28("KA1ABC"), 28);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 15, 4);
        append_bits(&mut bits, 0, 3);
        append_bits(&mut bits, 11, 7); // EMA, WSJT-X 1-based section index
        append_bits(&mut bits, 3, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "WA9XYZ KA1ABC R 16A EMA");
    }

    #[test]
    fn unpacks_type_05_telemetry_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 0x012345, 23);
        append_bits(&mut bits, 0x6789AB, 24);
        append_bits(&mut bits, 0xCDEF01, 24);
        append_bits(&mut bits, 5, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "123456789ABCDEF01");
    }

    #[test]
    fn unpacks_type_3_rtty_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, pack28("W9XYZ"), 28);
        append_bits(&mut bits, pack28("K1ABC"), 28);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 5, 3);
        append_bits(&mut bits, 8000 + 21, 13); // MA, WSJT-X 1-based mult index
        append_bits(&mut bits, 3, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "TU; W9XYZ K1ABC R 579 MA");
    }

    #[test]
    fn rejects_type_3_rtty_cq_token_in_callsign_slot() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, pack28("CQ_001"), 28);
        append_bits(&mut bits, pack28("IZ7MMG"), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 3, 3);
        append_bits(&mut bits, 2025, 13);
        append_bits(&mut bits, 3, 3);

        assert_eq!(bits.len(), 77);
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn unpacks_type_5_eu_vhf_hashed_calls_message() {
        let book = HashCallBook::new();
        book.save("K1ABC");
        book.save("G4ABC/P");
        let mut bits = Vec::new();
        append_bits(&mut bits, ihashcall("K1ABC", 12), 12);
        append_bits(&mut bits, ihashcall("G4ABC/P", 22), 22);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 7, 3);
        append_bits(&mut bits, 3, 11);
        append_bits(&mut bits, grid6_24("IO91NP"), 25);
        append_bits(&mut bits, 5, 3);
        assert_eq!(
            unpack77(&bits, Some(&book)).unwrap(),
            "<K1ABC> <G4ABC/P> R 590003 IO91NP"
        );
    }

    #[test]
    fn type5_uses_mycall_hash12_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, ihashcall("K1ABC", 12), 12);
        append_bits(&mut bits, ihashcall("G4ABC/P", 22), 22);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 7, 3);
        append_bits(&mut bits, 3, 11);
        append_bits(&mut bits, grid6_24("IO91NP"), 25);
        append_bits(&mut bits, 5, 3);
        let context = super::UnpackContext::with_calls(None, Some("K1ABC"), None);
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "<K1ABC> <...> R 590003 IO91NP"
        );
    }
}
