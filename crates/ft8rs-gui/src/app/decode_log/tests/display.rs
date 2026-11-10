use super::*;

// Frozen sequential UI operation, before batching. This is intentionally
// independent of DecodeBatch's indexing/eviction bookkeeping. It shares the
// merge rule; comparison.rs checks that rule against explicit expected rows.
fn insert_decode_row_reference(
    rows: &mut Vec<Row>,
    display: &mut Vec<DisplayItem>,
    mut incoming: Row,
    tolerance: f64,
) {
    if fold_decode_row(rows, &incoming, tolerance).is_some() {
        return;
    }
    let slot = incoming.slot;
    let index = rows.partition_point(|row| row.slot <= slot);
    if let Some(previous) = rows[..index].last().filter(|row| row.slot == slot) {
        incoming.slot_band = previous.slot_band;
    }
    let append = index == rows.len();
    if append {
        if rows.last().is_none_or(|row| row.slot != slot) {
            display.push(DisplayItem::Separator(slot, incoming.slot_band));
        }
        display.push(DisplayItem::Decode(index));
    }
    rows.insert(index, incoming);
    if !append {
        *display = display_items(rows, DisplayFilters::NONE);
    }
    if rows.len() > MAX_ROWS {
        rows.drain(..rows.len() - MAX_ROWS);
        *display = display_items(rows, DisplayFilters::NONE);
    }
}

#[test]
fn batch_matches_sequential_rows_across_capacity_late_and_hash_updates() {
    let row = |utc_slot, id| {
        let message = format!("CQ K{id}ABC FN42");
        Row {
            text: message.clone(),
            slot: SlotKey::utc(utc_slot),
            slot_band: None,
            paired_external: None,
            freq: 200.0 + (id % 2800) as f64,
            dt: 0.0,
            tokens: norm_tokens(&message),
            filterable: None,
            source: DecodeSource::Local,
        }
    };
    for initial in [0, MAX_ROWS - 10, MAX_ROWS] {
        let initial_rows: Vec<_> = (0..initial).map(|i| row((i / 40) as u64, i)).collect();
        let mut seed = 739u64;
        let mut input = Vec::new();
        for i in 0..1536 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut incoming = if seed.is_multiple_of(3) && !initial_rows.is_empty() {
                initial_rows[(seed as usize / 3) % initial_rows.len()].clone()
            } else {
                row(498 + seed % 8, i + seed as usize % 10000)
            };
            if seed.is_multiple_of(5) {
                incoming.tokens[1] = "<...>".into();
                incoming.text = "CQ <...> FN42".into();
            }
            if seed.is_multiple_of(7) {
                incoming.source = DecodeSource::External;
            }
            if seed.is_multiple_of(11) {
                incoming.text.push_str(" exp");
            }
            input.push(incoming);
        }
        for chunk_size in [1, 17, 128, 1536] {
            let mut expected = initial_rows.clone();
            let mut expected_display = display_items(&expected, DisplayFilters::NONE);
            let mut actual = expected.clone();
            let mut actual_display = expected_display.clone();
            for chunk in input.chunks(chunk_size) {
                for incoming in chunk {
                    insert_decode_row_reference(
                        &mut expected,
                        &mut expected_display,
                        incoming.clone(),
                        7.0,
                    );
                }
                {
                    let mut batch = DecodeBatch::new(
                        &mut actual,
                        &mut actual_display,
                        7.0,
                        DisplayFilters::NONE,
                    );
                    for incoming in chunk {
                        batch.insert(incoming.clone());
                    }
                }
                assert_eq!(
                    actual, expected,
                    "initial={initial}, chunk_size={chunk_size}"
                );
                assert_eq!(actual_display, expected_display);
            }
        }
    }
}

#[test]
fn repeated_udp_observations_and_late_rows_stay_in_one_slot_section() {
    let row = |utc_slot, source, dt, snr| Row {
        text: format!("{snr} JA8IIK/P SV9GPV -18"),
        slot: SlotKey::utc(utc_slot),
        slot_band: None,
        paired_external: None,
        freq: 1137.0,
        dt,
        tokens: norm_tokens("JA8IIK/P SV9GPV -18"),
        filterable: None,
        source,
    };
    let mut rows = Vec::new();
    let mut display = Vec::new();
    insert_decode_row(
        &mut rows,
        &mut display,
        row(1, DecodeSource::External, 2.4, -6),
        7.0,
    );
    insert_decode_row(
        &mut rows,
        &mut display,
        row(1, DecodeSource::External, 2.4, -8),
        7.0,
    );
    assert_eq!(rows.len(), 1);
    insert_decode_row(
        &mut rows,
        &mut display,
        row(1, DecodeSource::Local, 2.4, -7),
        7.0,
    );
    insert_decode_row(
        &mut rows,
        &mut display,
        row(1, DecodeSource::External, 2.4, -9),
        7.0,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, DecodeSource::Both);
    assert!(rows[0].text.starts_with("-7 "));
    // Same payload at a distinct DT is a separate transmission.
    insert_decode_row(
        &mut rows,
        &mut display,
        row(1, DecodeSource::Local, 0.1, -20),
        7.0,
    );
    assert_eq!(rows.len(), 2);
    insert_decode_row(
        &mut rows,
        &mut display,
        row(3, DecodeSource::External, 0.1, -20),
        7.0,
    );
    insert_decode_row(
        &mut rows,
        &mut display,
        row(2, DecodeSource::Local, 0.1, -20),
        7.0,
    );
    assert_eq!(
        rows.iter().map(|r| r.slot.utc_slot).collect::<Vec<_>>(),
        [1, 1, 2, 3]
    );
    let sections: Vec<_> = display
        .iter()
        .filter_map(|item| match item {
            DisplayItem::Separator(slot, _) => Some(slot.utc_slot),
            _ => None,
        })
        .collect();
    assert_eq!(sections, [1, 2, 3]);
}

#[test]
fn slot_separators_capture_the_current_band_only_on_first_insert() {
    let row = |utc_slot, slot_band, freq, filterable| Row {
        text: "CQ K1ABC FN42".into(),
        slot: SlotKey::utc(utc_slot),
        slot_band,
        paired_external: None,
        freq,
        dt: 0.0,
        tokens: norm_tokens("CQ K1ABC FN42"),
        filterable,
        source: DecodeSource::Local,
    };
    let filters = DisplayFilters {
        hide_contest: false,
        hide_telemetry: true,
    };
    let mut rows = Vec::new();
    let mut display = Vec::new();
    {
        let mut batch = DecodeBatch::new(&mut rows, &mut display, 7.0, filters);
        batch.insert(row(
            10,
            Some("20m"),
            1000.0,
            Some(FilterableMessage::Telemetry),
        ));
        batch.insert(row(12, None, 1000.0, None));
        batch.insert(row(10, Some("40m"), 1100.0, None));
        batch.insert(row(11, Some("40m"), 1000.0, None));
        batch.insert(row(12, Some("40m"), 1100.0, None));
    }
    assert_eq!(
        display,
        [
            DisplayItem::Separator(SlotKey::utc(10), Some("20m")),
            DisplayItem::Decode(1),
            DisplayItem::Separator(SlotKey::utc(11), Some("40m")),
            DisplayItem::Decode(2),
            DisplayItem::Separator(SlotKey::utc(12), None),
            DisplayItem::Decode(3),
            DisplayItem::Decode(4),
        ]
    );
    assert_eq!(
        display_items(&rows[1..], filters)[0],
        DisplayItem::Separator(SlotKey::utc(10), Some("20m"))
    );
}

#[test]
fn display_filters_retain_local_rows_and_leave_udp_visible() {
    let row = |utc_slot, text: &str, source, filterable| Row {
        text: text.into(),
        slot: SlotKey::utc(utc_slot),
        slot_band: None,
        paired_external: None,
        freq: 1386.0,
        dt: 0.0,
        tokens: norm_tokens(text),
        filterable,
        source,
    };
    let filters = DisplayFilters {
        hide_contest: true,
        hide_telemetry: true,
    };
    let rover_cq = "CQ K1ABC/R FN42";
    let mut rows = Vec::new();
    let mut display = Vec::new();
    {
        let mut batch = DecodeBatch::new(&mut rows, &mut display, 7.0, filters);
        batch.insert(row(
            10,
            "7",
            DecodeSource::Local,
            Some(FilterableMessage::Telemetry),
        ));
        batch.insert(row(
            11,
            rover_cq,
            DecodeSource::Local,
            Some(FilterableMessage::Contest),
        ));
        batch.insert(row(12, "17E65738A547413FF0", DecodeSource::External, None));
    }
    assert_eq!(
        display,
        [
            DisplayItem::Separator(SlotKey::utc(12), None),
            DisplayItem::Decode(2)
        ]
    );
    assert_eq!(display_items(&rows, DisplayFilters::NONE).len(), 6);
    for (filters, visible_slot) in [
        (
            DisplayFilters {
                hide_contest: false,
                ..filters
            },
            11,
        ),
        (
            DisplayFilters {
                hide_telemetry: false,
                ..filters
            },
            10,
        ),
    ] {
        assert_eq!(
            display_items(&rows, filters)[0],
            DisplayItem::Separator(SlotKey::utc(visible_slot), None)
        );
    }
    DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row(
        11,
        rover_cq,
        DecodeSource::External,
        None,
    ));
    DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row(
        12,
        "17E65738A547413FF0",
        DecodeSource::Local,
        Some(FilterableMessage::Telemetry),
    ));
    assert_eq!(
        display,
        [
            DisplayItem::Separator(SlotKey::utc(11), None),
            DisplayItem::Decode(1),
            DisplayItem::Separator(SlotKey::utc(12), None),
            DisplayItem::Decode(2),
        ]
    );
    assert_eq!(rows[1].source, DecodeSource::Both);
    assert_eq!(rows[2].source, DecodeSource::Both);
    assert_eq!(display_items(&rows, filters), display);
}

#[test]
fn reads_filter_types_from_the_ft8_symbols() {
    use FilterableMessage::{Contest, Telemetry};

    // Symbols 30..33 from frames encoded by the FT8 encoder.
    for (symbols, expected) in [
        ([4, 0, 0], None),            // 0.0, free text
        ([3, 3, 1], None),            // 0.1, DXpedition
        ([0, 4, 0], Some(Contest)),   // 0.3, Field Day
        ([7, 0, 1], Some(Contest)),   // 0.4, Field Day
        ([7, 3, 1], Some(Telemetry)), // 0.5, telemetry
        ([4, 0, 3], None),            // 1, standard
        ([6, 5, 5], None),            // 2, /P
        ([6, 3, 4], Some(Contest)),   // 3, RTTY Roundup
        ([6, 1, 0], None),            // 4, nonstandard call
        ([3, 2, 2], Some(Contest)),   // 5, EU VHF exchange
    ] {
        let mut tones = [0; 79];
        for sync in [0, 36, 72] {
            tones[sync..sync + 7].copy_from_slice(&[3, 1, 4, 0, 6, 5, 2]);
        }
        tones[30..33].copy_from_slice(&symbols);
        assert_eq!(
            filterable_message(ft8_payload(&tones).as_ref(), ""),
            expected
        );
    }
    assert_eq!(ft8_payload(&[0; 79]), None);
}

fn local_fixture_row(wire: &str, decoded: &str) -> Row {
    use ft8rs::decode::lib_jtdx::ft8v2::packjt77::pack77;

    const COSTAS: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
    const GRAY: [i32; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
    let bits = pack77(wire);
    let mut tones = [0; 79];
    for sync in [0, 36, 72] {
        tones[sync..sync + 7].copy_from_slice(&COSTAS);
    }
    for (tone, bits) in tones[7..33].iter_mut().zip(bits.chunks(3)) {
        let symbol = (bits[0] << 2) | (bits[1] << 1) | bits.get(2).copied().unwrap_or(0);
        *tone = GRAY[symbol as usize];
    }
    assert_eq!(ft8_payload(&tones).unwrap().as_slice(), bits);
    local_decode_row(ft8rs_engine::protocol::DecodeRecord {
        timestamp: ft8rs::SlotTimestamp::parse("120000").unwrap(),
        row: ft8rs::stream::StreamDecodedMessage {
            freq: 1002.0,
            dt: 0.1,
            snr: -20.0,
            msg: decoded.into(),
            sync: 0.0,
            itone: tones,
        },
        provenance: StreamDecodeProvenance::Experiment,
        stage: ft8rs_engine::protocol::DecodeStage::Final,
    })
}

#[test]
fn hide_contest_matches_payload_types_rover_calls_and_acknowledged_grids() {
    let filters = DisplayFilters {
        hide_contest: true,
        ..DisplayFilters::NONE
    };
    for (message, hidden) in [
        ("CQ WU8JCF/R EN81", true),
        ("K1ABC/R W9XYZ RR73", true),
        ("<K1ABC/R> W9XYZ RR73", true),
        ("BG5ATV WA7QSU R MO76", true),
        ("W9XYZ K1ABC R RR99", true),
        ("G4ABC/P PA9XYZ JO22", false),
        ("PA9XYZ G4ABC/P R-10", false),
        ("CQ G4ABC/P IO91", false),
        ("PA9XYZ G4ABC/P R IO91", true),
        ("K1ABC W9XYZ R 6A WI", true),
        ("K1ABC W9XYZ R 32A EMA", true),
        ("K1ABC W9XYZ R 579 MA", true),
        ("<G4ABC/P> <PA9XYZ> R 580071 JO22DB", true),
        ("CQ ZW5STAY/R", true),
        ("CQ TEST K1ABC FN42", false),
        ("CQ FD K1ABC FN42", false),
        ("CQ RU K1ABC FN42", false),
        ("CQ WW K1ABC FN42", false),
        ("CQ TEST K1ABC/R FN42", true),
        ("CQ FD K1ABC/R FN42", true),
        ("CQ RU K1ABC/R FN42", true),
        ("CQ WW K1ABC/R FN42", true),
        ("CQ RU G4ABC/P IO91", false),
        ("BG5ATV WA7QSU MO76", false),
        ("BG5ATV WA7QSU R-10", false),
        ("BG5ATV WA7QSU R+10", false),
        ("BG5ATV WA7QSU RR73", false),
        ("CQ POTA K1ABC FN42", false),
        ("CQ 6W/IK2HKT", false),
        ("TEST /R", false),
        ("R MO76", false),
    ] {
        let row = local_fixture_row(message, message);
        let mut rows = Vec::new();
        let mut display = Vec::new();
        DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row);
        assert_eq!(display.is_empty(), hidden, "{message}");
        assert_eq!(display, display_items(&rows, filters));
        assert_eq!(display_items(&rows, DisplayFilters::NONE).len(), 2);
    }
    assert_eq!(
        filterable_message(None, "BG5ATV WA7QSU R MO76 a7"),
        Some(FilterableMessage::Contest)
    );
}

#[test]
fn local_rows_restore_hash_brackets_from_the_received_payload() {
    for (wire, decoded, expected) in [
        (
            "<6W/IK2HKT> JA5ARW PM64",
            "6W/IK2HKT JA5ARW PM64",
            "<6W/IK2HKT> JA5ARW PM64",
        ),
        (
            "<6W/IK2HKT> JA5ARW PM64",
            "<...> JA5ARW PM64",
            "<...> JA5ARW PM64",
        ),
        ("CQ 6W/IK2HKT", "CQ 6W/IK2HKT", "CQ 6W/IK2HKT"),
        (
            "<JA5ARW> 6W/IK2HKT RR73",
            "JA5ARW 6W/IK2HKT RR73",
            "<JA5ARW> 6W/IK2HKT RR73",
        ),
        (
            "JA5ARW <6W/IK2HKT> -12",
            "JA5ARW 6W/IK2HKT -12",
            "JA5ARW <6W/IK2HKT> -12",
        ),
        (
            "K1ABC RR73; W9XYZ <KH1/KH7Z> -08",
            "K1ABC KH1/KH7Z RR73",
            "K1ABC <KH1/KH7Z> RR73",
        ),
        (
            "K1ABC RR73; W9XYZ <KH1/KH7Z> -08",
            "W9XYZ KH1/KH7Z -08",
            "W9XYZ <KH1/KH7Z> -08",
        ),
        (
            "<G4ABC/P> <PA9XYZ> R 580071 JO22DB",
            "G4ABC/P <PA9XYZ> R 580071 JO22DB",
            "<G4ABC/P> <PA9XYZ> R 580071 JO22DB",
        ),
        (
            "<6W/IK2HKT> JA5ARW PM64",
            "6W/IK2HKT W9XYZ PM64",
            "6W/IK2HKT W9XYZ PM64",
        ),
        ("CQ W9XYZ/R EN37", "CQ W9XYZ/R EN37", "CQ W9XYZ/R EN37"),
        (
            "W9XYZ K1ABC R FN42",
            "W9XYZ K1ABC R FN42",
            "W9XYZ K1ABC R FN42",
        ),
    ] {
        let row = local_fixture_row(wire, decoded);
        assert_eq!(clean_message(&row.text[24..]), expected, "{wire}");
        assert_eq!(row.tokens, norm_tokens(decoded));
        assert!(row.text.ends_with(" exp"));
    }
}

#[test]
fn separators_mark_first_and_subsequent_nonempty_slots() {
    let row = |utc_slot| Row {
        text: String::new(),
        slot: SlotKey::utc(utc_slot),
        slot_band: None,
        paired_external: None,
        freq: 1000.0,
        dt: 0.0,
        tokens: Vec::new(),
        filterable: None,
        source: DecodeSource::Local,
    };
    let rows = [row(10), row(10), row(13), row(13), row(15)];
    let items = display_items(&rows, DisplayFilters::NONE);
    assert_eq!(items.len(), 8);
    assert_eq!(items[0], DisplayItem::Separator(SlotKey::utc(10), None));
    assert!(matches!(items[1], DisplayItem::Decode(0)));
    assert!(matches!(items[2], DisplayItem::Decode(1)));
    assert_eq!(items[3], DisplayItem::Separator(SlotKey::utc(13), None));
    assert_eq!(items[6], DisplayItem::Separator(SlotKey::utc(15), None));
    assert_eq!(
        display_items(&rows[1..], DisplayFilters::NONE)[0],
        DisplayItem::Separator(SlotKey::utc(10), None)
    );
    assert!(display_items(&[], DisplayFilters::NONE).is_empty());
    // Unix epoch plus one skipped slot: no divider for the missing slot.
    assert_eq!(
        separator_label(2, None),
        "------ 1970-01-01 - 00:00:30 UTC - FT8 ------"
    );
    assert_eq!(
        separator_label(2, Some("20m")),
        "------ 1970-01-01 - 00:00:30 UTC - 20m - FT8 ------"
    );
}

#[test]
fn clock_segments_keep_repeated_utc_slots_separate_and_in_timeline_order() {
    let old = SlotKey::utc(10);
    let repeated = SlotKey {
        clock_segment: 1,
        utc_slot: 10,
    };
    let earlier_after_step = SlotKey {
        clock_segment: 1,
        utc_slot: 9,
    };
    let row = |slot, source, slot_band| Row {
        text: "CQ K1ABC FN42".to_owned(),
        slot,
        slot_band,
        paired_external: None,
        freq: 1000.0,
        dt: 0.0,
        tokens: norm_tokens("CQ K1ABC FN42"),
        source,
        filterable: None,
    };

    let mut log = DecodeLog::default();
    {
        let mut batch = log.batch(7.0, DisplayFilters::NONE);
        batch.insert(row(old, DecodeSource::Local, Some("20m")));
        batch.insert(row(repeated, DecodeSource::External, Some("40m")));
        batch.insert(row(earlier_after_step, DecodeSource::Local, Some("40m")));
    }

    let (rows, items) = log.display();
    assert_eq!(
        rows.iter()
            .map(|row| (row.slot, row.source))
            .collect::<Vec<_>>(),
        [
            (old, DecodeSource::Local),
            (earlier_after_step, DecodeSource::Local),
            (repeated, DecodeSource::External),
        ],
    );
    assert_eq!(
        items,
        [
            DisplayItem::Separator(old, Some("20m")),
            DisplayItem::Decode(0),
            DisplayItem::Separator(earlier_after_step, Some("40m")),
            DisplayItem::Decode(1),
            DisplayItem::Separator(repeated, Some("40m")),
            DisplayItem::Decode(2),
        ],
    );
}
