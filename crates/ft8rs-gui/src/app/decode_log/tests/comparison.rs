use super::*;

#[test]
fn compare_keeps_local_provenance_in_both_arrival_orders() {
    let message = "CQ K1ABC FN42";
    assert_eq!(provenance_tag(StreamDecodeProvenance::CandidateRetry), "Pr");
    assert_eq!(
        provenance_tag(StreamDecodeProvenance::PlusCompatibility),
        "Pc"
    );
    assert_eq!(
        provenance_tag(StreamDecodeProvenance::ContextRecovery),
        "ctx"
    );
    for provenance in [
        StreamDecodeProvenance::Experiment,
        StreamDecodeProvenance::CooperativeRecovery,
        StreamDecodeProvenance::ContextRecovery,
        StreamDecodeProvenance::PlusCompatibility,
        StreamDecodeProvenance::CandidateRetry,
        StreamDecodeProvenance::ApMask,
        StreamDecodeProvenance::WsjtxAp(ft8rs::stream::session::WsjtxApType::Cq),
        StreamDecodeProvenance::A8List,
    ] {
        let local_text = format!(
            "120000 -20   0.1  1002  {message} {}",
            provenance_tag(provenance)
        );
        let external_text = format!("120000 -19   0.2  1000  {message}");
        for first in [DecodeSource::Local, DecodeSource::External] {
            let local_first = first == DecodeSource::Local;
            let mut rows = vec![Row {
                text: if local_first {
                    local_text.clone()
                } else {
                    external_text.clone()
                },
                slot: SlotKey::utc(1),
                slot_band: None,
                paired_external: None,
                freq: if local_first { 1002.0 } else { 1000.0 },
                dt: 0.0,
                tokens: norm_tokens(message),
                filterable: None,
                source: first,
            }];
            assert!(fold_decode_row(
                &mut rows,
                &Row {
                    slot: SlotKey::utc(1),
                    slot_band: None,
                    paired_external: None,
                    freq: if local_first { 1000.0 } else { 1002.0 },
                    dt: 0.0,
                    tokens: norm_tokens(message),
                    filterable: None,
                    source: if local_first {
                        DecodeSource::External
                    } else {
                        DecodeSource::Local
                    },
                    text: (if local_first {
                        &external_text
                    } else {
                        &local_text
                    })
                    .to_string()
                },
                7.0
            )
            .is_some());
            assert_eq!(rows[0].source, DecodeSource::Both);
            assert_eq!(rows[0].text, local_text);
            assert_eq!(rows[0].freq, 1002.0);
        }
    }
}

#[test]
fn compare_retains_resolved_hash_calls_in_both_arrival_orders() {
    for (local_message, external_message, expected_message) in [
        ("<...> W1AW RR73", "<PJ4X> W1AW RR73", "<PJ4X> W1AW RR73"),
        ("PJ4X <...> -16", "PJ4X <W1AW> -16", "PJ4X <W1AW> -16"),
        ("<PJ4X> W1AW RR73", "<...> W1AW RR73", "<PJ4X> W1AW RR73"),
        (
            "<...> <W1AW> RR73",
            "<PJ4X> <...> RR73",
            "<PJ4X> <W1AW> RR73",
        ),
        (
            "<...> <...> RR73",
            "<PJ4X> <W1AW> RR73",
            "<PJ4X> <W1AW> RR73",
        ),
        ("<...> <...> RR73", "<...> <W1AW> RR73", "<...> <W1AW> RR73"),
    ] {
        let local = || Row {
            text: format!("120000 -20   0.1  1002  {local_message:<20} exp"),
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: 1002.0,
            dt: 0.1,
            tokens: norm_tokens(local_message),
            filterable: None,
            source: DecodeSource::Local,
        };
        let external = || Row {
            text: external_decode_text(&crate::wsjtx_udp::ExternalDecode {
                ms_since_midnight: 43_200_000,
                snr: -19,
                dt: 0.2,
                freq: 1000,
                message: external_message.into(),
            }),
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: 1000.0,
            dt: 0.2,
            tokens: norm_tokens(external_message),
            filterable: None,
            source: DecodeSource::External,
        };
        for local_first in [true, false] {
            let mut rows = Vec::new();
            let mut display = Vec::new();
            let observations = if local_first {
                [local(), external()]
            } else {
                [external(), local()]
            };
            for observation in observations {
                insert_decode_row(&mut rows, &mut display, observation, 7.0);
            }
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].source, DecodeSource::Both);
            assert_eq!(rows[0].tokens, norm_tokens(expected_message));
            assert!(rows[0].text.starts_with("120000 -20   0.1  1002  "));
            assert_eq!(clean_message(&rows[0].text[24..]), expected_message);
            assert!(rows[0].text.ends_with(" exp"));
            assert_eq!(rows[0].freq, 1002.0);
            assert_eq!(rows[0].dt, 0.1);

            // Neither source's later unresolved observation may erase a call.
            let text = rows[0].text.clone();
            for source in [DecodeSource::Local, DecodeSource::External] {
                let report = rows[0].tokens.last().unwrap();
                let unresolved = Row {
                    tokens: norm_tokens(&format!("<...> <...> {report}")),
                    filterable: None,
                    source,
                    ..local()
                };
                assert!(fold_decode_row(&mut rows, &unresolved, 7.0).is_some());
                assert_eq!(rows[0].text, text);
                assert_eq!(rows[0].tokens, norm_tokens(expected_message));
            }
            // A resolved call must also narrow subsequent merge matching.
            let mut conflicting = external();
            conflicting.tokens = rows[0].tokens.clone();
            let index = conflicting.tokens[..2]
                .iter()
                .position(|token| !is_hash_token(token))
                .unwrap();
            conflicting.tokens[index] = "W9XYZ".into();
            assert!(fold_decode_row(&mut rows, &conflicting, 7.0).is_none());
            assert_eq!(rows[0].text, text);
        }
    }
}

#[test]
fn compare_ignores_udp_annotations_and_preserves_all_local_annotations() {
    let local = |message: &str, provenance| {
        let record = ft8rs_engine::protocol::DecodeRecord {
            timestamp: ft8rs::SlotTimestamp::parse("120000").unwrap(),
            row: ft8rs::stream::StreamDecodedMessage {
                freq: 1002.0,
                dt: 0.1,
                snr: -20.0,
                msg: message.into(),
                sync: 0.0,
                itone: [0; 79],
            },
            provenance,
            stage: ft8rs_engine::protocol::DecodeStage::Final,
        };
        Row {
            text: local_decode_text(&record, None),
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: record.row.freq,
            dt: record.row.dt,
            tokens: norm_tokens(message),
            filterable: None,
            source: DecodeSource::Local,
        }
    };
    for local_first in [true, false] {
        let native = local(
            "<...> W1AW RR73 ? a7 exp",
            StreamDecodeProvenance::Experiment,
        );
        let decode = crate::wsjtx_udp::ExternalDecode {
            ms_since_midnight: 43_200_000,
            snr: -19,
            dt: 0.2,
            freq: 1000,
            message: "<PJ4X> W1AW RR73 ?a31 dp im".into(),
        };
        let external = Row {
            text: external_decode_text(&decode),
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: decode.freq as f64,
            dt: decode.dt,
            tokens: norm_tokens(&decode.message),
            filterable: None,
            source: DecodeSource::External,
        };
        let (first, second) = if local_first {
            (native, external)
        } else {
            (external, native)
        };
        let mut rows = vec![first];
        assert!(fold_decode_row(&mut rows, &second, 7.0).is_some());
        assert_eq!(rows[0].source, DecodeSource::Both);
        assert!(rows[0].text.contains("<PJ4X> W1AW RR73"));
        let annotations = |row: &Row| {
            row.text
                .split_whitespace()
                .filter(|token| is_annotation(token))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(annotations(&rows[0]), ["?", "a7", "exp"]);
        let repeated = local(
            "<...> W1AW RR73 sic",
            StreamDecodeProvenance::CooperativeRecovery,
        );
        for _ in 0..2 {
            assert!(fold_decode_row(&mut rows, &repeated, 7.0).is_some());
            assert_eq!(annotations(&rows[0]), ["?", "a7", "exp", "sic"]);
            assert_eq!(rows[0].tokens, norm_tokens("<PJ4X> W1AW RR73"));
        }
    }
}

#[test]
fn unknown_provenance_does_not_prevent_compare_matching() {
    let message = "CQ BA7SAY OL53";
    for (first, second) in [
        (DecodeSource::Local, DecodeSource::External),
        (DecodeSource::External, DecodeSource::Local),
    ] {
        let mut rows = vec![Row {
            text: format!(
                "{message} {}",
                provenance_tag(StreamDecodeProvenance::Unknown)
            ),
            slot: SlotKey::utc(3362),
            slot_band: None,
            paired_external: None,
            freq: 2367.0,
            dt: 0.0,
            tokens: norm_tokens(message),
            filterable: None,
            source: first,
        }];
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                slot: SlotKey::utc(3362),
                slot_band: None,
                paired_external: None,
                freq: 2369.0,
                dt: 0.0,
                tokens: norm_tokens("CQ BA7SAY OL53 ? a2"),
                filterable: None,
                source: second,
                text: message.to_string()
            },
            7.0
        )
        .is_some());
        assert_eq!(rows[0].source, DecodeSource::Both);
    }
}

#[test]
fn low_confidence_zl2agy_matches_in_either_arrival_order() {
    let external = crate::wsjtx_udp::ExternalDecode {
        ms_since_midnight: 40_530_000,
        snr: -23,
        dt: -1.1,
        freq: 1118,
        message: format!("{:<36}? a1", "CQ ZL2AGY RF70"),
    };
    let local_text = "111530 -21  -1.1  1118  CQ ZL2AGY RF70";
    let local_tokens = norm_tokens("CQ ZL2AGY RF70");
    let external_text = external_decode_text(&external);
    let external_tokens = norm_tokens(&external.message);
    assert_eq!(external_tokens, local_tokens);
    assert_eq!(external_text.split_whitespace().last(), Some("RF70"));
    assert!(!external_text.contains('?'));
    assert!(!external_text.contains("a1"));

    for first in [DecodeSource::Local, DecodeSource::External] {
        let (text, tokens, incoming, source) = if first == DecodeSource::Local {
            (
                local_text,
                &local_tokens,
                &external_tokens,
                DecodeSource::External,
            )
        } else {
            (
                external_text.as_str(),
                &external_tokens,
                &local_tokens,
                DecodeSource::Local,
            )
        };
        let mut rows = vec![Row {
            text: text.to_string(),
            slot: SlotKey::utc(u64::from(external.ms_since_midnight / 15_000)),
            slot_band: None,
            paired_external: None,
            freq: 1118.0,
            dt: 0.0,
            tokens: tokens.clone(),
            filterable: None,
            source: first,
        }];
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                slot: SlotKey::utc(u64::from(external.ms_since_midnight / 15_000)),
                slot_band: None,
                paired_external: None,
                freq: 1118.0,
                dt: 0.0,
                tokens: incoming.to_vec(),
                filterable: None,
                source,
                text: local_text.to_string()
            },
            7.0
        )
        .is_some());
        assert_eq!(rows[0].source, DecodeSource::Both);
        assert_eq!(rows[0].text, local_text);
    }

    // Removing annotations must give the same display as a plain payload.
    let without_text_marker = crate::wsjtx_udp::ExternalDecode {
        message: "CQ ZL2AGY RF70".to_string(),
        ..external
    };
    assert_eq!(external_decode_text(&without_text_marker), external_text);
}

#[test]
fn annotation_detection() {
    for t in [
        "a1", "a7", "a9", "a72", "a31", "?", "?a1", "a1?", "sic", "ctx", "exp", "ap", "dp", "im",
        "p", "Pc", "Pr", "retry",
    ] {
        assert!(is_annotation(t), "{t} should be an annotation");
    }
    for t in [
        "CQ", "A7", "FN42", "K1ABC", "RR73", "73", "R-12", "<...>", "<K1ABC>",
    ] {
        assert!(!is_annotation(t), "{t} should not be an annotation");
    }
}

#[test]
fn clean_message_drops_annotations() {
    assert_eq!(clean_message("CQ K1ABC FN42 a7"), "CQ K1ABC FN42");
    assert_eq!(clean_message("K1ABC W1AW R-12 ? a31"), "K1ABC W1AW R-12");
    assert_eq!(clean_message("CQ BG5ATV PM00"), "CQ BG5ATV PM00");
    let body = "CQ ZL2AGY RF70";
    for annotations in ["? a1", "?a1", "a1?", "a7 a8 ap dp im sic ctx exp"] {
        let annotated = format!("{body} {annotations}");
        assert_eq!(clean_message(&annotated), body);
        assert_eq!(norm_tokens(&annotated), norm_tokens(body));
        assert!(tokens_compatible(
            &norm_tokens(&annotated),
            &norm_tokens("CQ ZL2AGY RF70 ? a2"),
        ));
    }
    // Keep uppercase payload words, including callsigns and free text.
    assert_eq!(clean_message("A7 TEST EXP AP"), "A7 TEST EXP AP");
}

#[test]
fn external_with_annotations_matches_local_body() {
    let local = norm_tokens("CQ K1ABC FN42");
    let external = norm_tokens("CQ K1ABC FN42 a7"); // WSJT-X AP marker
    assert!(tokens_compatible(&local, &external));
}

#[test]
fn hashed_calls_match_leniently() {
    let resolved = norm_tokens("PJ4X K1ABC RR73");
    let hashed = norm_tokens("<...> K1ABC RR73 a2");
    assert!(tokens_compatible(&resolved, &hashed));
}

#[test]
fn different_messages_do_not_match() {
    assert!(!tokens_compatible(
        &norm_tokens("CQ K1ABC FN42"),
        &norm_tokens("CQ W1AW EM73"),
    ));
    assert!(!tokens_compatible(
        &norm_tokens("<PJ4X> K1ABC RR73"),
        &norm_tokens("<W1AW> K1ABC RR73"),
    ));
    assert!(tokens_compatible(
        &norm_tokens("<PJ4X> K1ABC RR73"),
        &norm_tokens("PJ4X K1ABC RR73"),
    ));
}

#[test]
fn repeated_rows_fold_and_distinct_dt_signals_are_preserved() {
    let mk = |source, freq, msg: &str| Row {
        text: String::new(),
        slot: SlotKey::utc(1),
        slot_band: None,
        paired_external: None,
        freq,
        dt: 0.0,
        tokens: norm_tokens(msg),
        filterable: None,
        source,
    };
    let mut rows = vec![mk(DecodeSource::Local, 1000.0, "CQ K1ABC FN42")];

    // An identical local observation is a duplicate, regardless of SNR.
    assert!(fold_decode_row(
        &mut rows,
        &Row {
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: 1002.0,
            dt: 0.0,
            tokens: norm_tokens("CQ K1ABC FN42"),
            filterable: None,
            source: DecodeSource::Local,
            text: String::new()
        },
        7.0
    )
    .is_some());
    assert_eq!(rows[0].source, DecodeSource::Local);

    // An EXTERNAL decode of the same signal folds into the local row → Both.
    assert!(fold_decode_row(
        &mut rows,
        &Row {
            slot: SlotKey::utc(1),
            slot_band: None,
            paired_external: None,
            freq: 1001.0,
            dt: 0.0,
            tokens: norm_tokens("CQ K1ABC FN42"),
            filterable: None,
            source: DecodeSource::External,
            text: String::new()
        },
        7.0
    )
    .is_some());
    assert_eq!(rows[0].source, DecodeSource::Both);
    // A Both row continues to absorb repeat observations from either source.
    for source in [DecodeSource::Local, DecodeSource::External] {
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                slot: SlotKey::utc(1),
                slot_band: None,
                paired_external: None,
                freq: 1001.0,
                dt: 0.0,
                tokens: norm_tokens("CQ K1ABC FN42"),
                filterable: None,
                source,
                text: String::new()
            },
            7.0
        )
        .is_some());
    }

    // A different slot never folds.
    assert!(fold_decode_row(
        &mut rows,
        &Row {
            slot: SlotKey::utc(2),
            slot_band: None,
            paired_external: None,
            freq: 1000.0,
            dt: 0.0,
            tokens: norm_tokens("CQ K1ABC FN42"),
            filterable: None,
            source: DecodeSource::External,
            text: String::new()
        },
        7.0
    )
    .is_none());
}

#[test]
fn a_paired_row_keeps_distinct_same_source_signals_and_both_measurements() {
    let row = |source, freq| Row {
        text: "CQ K1ABC FN42".to_owned(),
        slot: SlotKey::utc(1),
        slot_band: None,
        paired_external: None,
        freq,
        dt: 0.0,
        tokens: norm_tokens("CQ K1ABC FN42"),
        source,
        filterable: None,
    };
    for first_source in [DecodeSource::Local, DecodeSource::External] {
        let mut rows = Vec::new();
        let mut display = Vec::new();
        let pair = if first_source == DecodeSource::Local {
            [
                (DecodeSource::Local, 1000.0),
                (DecodeSource::External, 1001.0),
            ]
        } else {
            [
                (DecodeSource::External, 1001.0),
                (DecodeSource::Local, 1000.0),
            ]
        };
        for (source, frequency) in pair {
            insert_decode_row(&mut rows, &mut display, row(source, frequency), 7.0);
        }
        assert_eq!(rows[0].source, DecodeSource::Both);
        assert_eq!(rows[0].paired_external, Some((1001.0, 0.0)));
        // The UDP repeat is 5 Hz from its own original measurement.
        insert_decode_row(
            &mut rows,
            &mut display,
            row(DecodeSource::External, 1006.0),
            7.0,
        );
        assert_eq!(rows.len(), 1);
        // The same frequency is 6 Hz away from the local measurement.
        insert_decode_row(
            &mut rows,
            &mut display,
            row(DecodeSource::Local, 1006.0),
            7.0,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].source, DecodeSource::Local);
    }
}

#[test]
fn compare_frequency_tolerance_uses_the_supplied_value_inclusively() {
    for first in [DecodeSource::Local, DecodeSource::External] {
        let incoming = if first == DecodeSource::Local {
            DecodeSource::External
        } else {
            DecodeSource::Local
        };
        for (tolerance, offset, expected) in [
            (0.0, 0.0, true),
            (0.0, 0.25, false),
            (0.25, 0.25, true),
            (0.25, -0.25, true),
            (0.25, 0.5, false),
            (2.0, 2.0, true),
            (2.0, -2.0, true),
            (2.0, 2.25, false),
            (5.0, 5.0, true),
            (5.0, 5.25, false),
            (10.0, 10.0, true),
            (10.0, 10.25, false),
        ] {
            let mut rows = vec![Row {
                text: String::new(),
                slot: SlotKey::utc(2702),
                slot_band: None,
                paired_external: None,
                freq: 1118.0,
                dt: 0.0,
                tokens: norm_tokens("CQ ZL2AGY RF70 a7"),
                filterable: None,
                source: first,
            }];
            assert_eq!(
                fold_decode_row(
                    &mut rows,
                    &Row {
                        slot: SlotKey::utc(2702),
                        slot_band: None,
                        paired_external: None,
                        freq: 1118.0 + offset,
                        dt: 0.0,
                        tokens: norm_tokens("CQ ZL2AGY RF70 ? a1"),
                        filterable: None,
                        source: incoming,
                        text: String::new()
                    },
                    tolerance
                )
                .is_some(),
                expected,
                "source={first:?}, tolerance={tolerance}, offset={offset}",
            );
            assert_eq!(rows[0].source == DecodeSource::Both, expected);
        }
    }
}
