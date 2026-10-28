mod adapters;
mod data;
mod extended;
mod random;
mod support;
use adapters::ADAPTERS;
use ft8rs::{SlotTimestamp, StreamDecodeConfig};
use support::{audio, normal, replay};

#[test]
fn upstream_waveforms_cover_message_types_offsets_noise_and_channel_damage() {
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    let mut errors = Vec::new();
    for source in ["wsjtx", "jtdx"] {
        let manifest = data::read(&format!("ft8/audio/{source}.tsv"));
        for line in manifest.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 8);
            let id = fields[0];
            let samples = audio(data::root().join(format!("ft8/audio/{}", fields[1])));
            let message = fields[2];
            let frequency: f64 = fields[3].parse().unwrap();
            let dt: f64 = fields[4].parse().unwrap();
            let target = if id == "nonstandard" {
                "KH1/KH7Z"
            } else {
                "K1ABC"
            };
            for adapter in ADAPTERS {
                let mut decoder = adapter.session(StreamDecodeConfig {
                    nfa: 100.0,
                    nfb: 3100.0,
                    nfqso: frequency,
                    hiscall: Some(target.into()),
                    ..Default::default()
                });
                let result = replay(decoder.as_mut(), &timestamp, &samples);
                let label = format!("{} / {source}/{id}", adapter.name);
                eprintln!("{label}: {}", result.describe());
                let targeted = normal(message)
                    .split_whitespace()
                    .any(|word| word == target);
                if (!adapter.target_only || targeted)
                    && !result.rows.iter().any(|row| {
                        normal(&row.msg) == normal(message)
                            && (row.freq - frequency).abs() <= 5.0
                            && (row.dt - dt).abs() <= 0.3
                    })
                {
                    errors.push(format!(
                        "{label}: missing {message:?} @ {frequency}/{dt}; got {:?}",
                        result
                            .rows
                            .iter()
                            .map(|r| (&r.msg, r.freq, r.dt))
                            .collect::<Vec<_>>()
                    ));
                }
                for row in &result.rows {
                    // Every signal in this corpus has known content. Extra rows
                    // are false decodes, not gains or unlabelled real traffic.
                    if normal(&row.msg) != normal(message) {
                        errors.push(format!("{label}: false decode {:?}", row.msg));
                    }
                }
            }
        }
    }
    assert!(
        errors.is_empty(),
        "FT8 waveform regressions:\n{}",
        errors.join("\n")
    );
}

#[test]
fn silence_and_fixed_noise_with_wrong_context_do_not_create_messages() {
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    for (name, samples) in [
        ("silence", vec![0.0; 180000]),
        ("noise-1", support::noise(2026102801)),
        ("noise-2", support::noise(2026102809)),
    ] {
        for adapter in ADAPTERS {
            let mut decoder = adapter.session(StreamDecodeConfig {
                mycall: Some("K1JT".into()),
                hiscall: Some("BG5ATV".into()),
                hisgrid: Some("PM00".into()),
                nfqso: 1000.0,
                ..Default::default()
            });
            let result = replay(decoder.as_mut(), &timestamp, &samples);
            assert!(
                result.rows.is_empty(),
                "{} {name}: false decode {:?}",
                adapter.name,
                result.rows.iter().map(|r| &r.msg).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a8_recovery_requires_correct_context_and_preserves_native_capabilities() {
    let samples = audio("ft8/a8d_k1jt_bg5atv_pm00.wav");
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    for adapter in ADAPTERS {
        for context in ["none", "correct", "wrong", "partial"] {
            let config = StreamDecodeConfig {
                nfa: 900.0,
                nfb: 1100.0,
                nfqso: 1000.0,
                mycall: (context != "none").then(|| "K1JT".into()),
                hiscall: Some(
                    if context == "wrong" {
                        "W9XYZ"
                    } else {
                        "BG5ATV"
                    }
                    .into(),
                ),
                hisgrid: matches!(context, "correct" | "wrong").then(|| "PM00".into()),
                lft8apon: context != "none",
                ..Default::default()
            };
            let result = replay(adapter.session(config).as_mut(), &timestamp, &samples);
            let recovered = result
                .rows
                .iter()
                .any(|r| normal(&r.msg) == "K1JT BG5ATV PM00");
            let expected = context == "correct" && adapter.supports_a8;
            assert_eq!(
                recovered,
                expected,
                "{} context={context}: {:?}",
                adapter.name,
                result.rows.iter().map(|r| &r.msg).collect::<Vec<_>>()
            );
            assert!(
                result
                    .rows
                    .iter()
                    .all(|r| normal(&r.msg) == "K1JT BG5ATV PM00"),
                "{} context={context}: invented message",
                adapter.name
            );
        }
    }
}

#[test]
fn simultaneous_signals_keep_both_messages() {
    let root = data::root().join("ft8/audio");
    for source in ["wsjtx", "jtdx"] {
        let first = audio(root.join(format!("{source}_cq.wav")));
        let second = audio(root.join(format!("{source}_report.wav")));
        let samples: Vec<_> = first
            .iter()
            .zip(second)
            .map(|(a, b)| (a + b) * 0.5)
            .collect();
        for adapter in ADAPTERS {
            let result = replay(
                adapter
                    .session(StreamDecodeConfig {
                        hiscall: Some("K1ABC".into()),
                        ..Default::default()
                    })
                    .as_mut(),
                &SlotTimestamp::parse("261028_120000").unwrap(),
                &samples,
            );
            for expected in ["CQ K1ABC FN42", "W9XYZ K1ABC -11"] {
                assert!(
                    result.rows.iter().any(|r| normal(&r.msg) == expected),
                    "{} {source}: dropped {expected}; {}",
                    adapter.name,
                    result.describe()
                );
            }
            assert!(result
                .rows
                .iter()
                .all(|r| ["CQ K1ABC FN42", "W9XYZ K1ABC -11"].contains(&normal(&r.msg).as_str())));
        }
    }
}

#[test]
fn reused_session_does_not_replay_previous_audio_or_context() {
    for adapter in ADAPTERS {
        let mut session = adapter.session(StreamDecodeConfig {
            hiscall: Some("K1ABC".into()),
            ..Default::default()
        });
        let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
        let signal = audio(data::root().join("ft8/audio/wsjtx_cq.wav"));
        let first = replay(session.as_mut(), &timestamp, &signal);
        assert!(
            first.rows.iter().any(|r| normal(&r.msg) == "CQ K1ABC FN42"),
            "{}",
            adapter.name
        );
        for offset in [15, 30] {
            let empty = replay(
                session.as_mut(),
                &timestamp.add_seconds(offset),
                &vec![0.0; 180000],
            );
            assert!(
                empty.rows.is_empty(),
                "{} stale rows in slot +{offset}",
                adapter.name
            );
        }
    }
}

#[test]
fn crowded_band_preserves_each_native_message_in_hybrid() {
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    for source in ["wsjtx", "jtdx"] {
        let weak = audio(data::root().join(format!("ft8/audio/{source}_weak.wav")));
        let strong = audio(data::root().join(format!("ft8/audio/{source}_neighbor.wav")));
        let samples: Vec<_> = weak
            .iter()
            .zip(strong)
            .map(|(a, b)| (a + b) * 0.5)
            .collect();
        let mut replays = Vec::new();
        for adapter in ADAPTERS {
            let result = replay(
                adapter
                    .session(StreamDecodeConfig {
                        hiscall: Some("K1ABC".into()),
                        hybrid_sic: false,
                        ..Default::default()
                    })
                    .as_mut(),
                &timestamp,
                &samples,
            );
            assert!(
                result
                    .rows
                    .iter()
                    .all(|r| ["CQ K1ABC FN42", "CQ W9XYZ EN37"].contains(&normal(&r.msg).as_str())),
                "{} {source}: false crowded-band message",
                adapter.name
            );
            if adapter.native_baseline {
                assert!(
                    result
                        .rows
                        .iter()
                        .any(|r| normal(&r.msg) == "CQ W9XYZ EN37"),
                    "{} {source}: lost strong neighboring signal",
                    adapter.name
                );
            }
            replays.push((adapter, result));
        }
        let native_rows: Vec<_> = replays
            .iter()
            .filter(|(adapter, _)| adapter.native_baseline)
            .flat_map(|(_, result)| &result.rows)
            .collect();
        for (adapter, result) in &replays {
            if adapter.preserves_native {
                for row in &native_rows {
                    assert!(
                        result.rows.iter().any(|other| support::matches(row, other)),
                        "{} {source}: dropped native row {}",
                        adapter.name,
                        row.msg
                    );
                }
            }
        }
    }
}

#[test]
fn callback_failure_is_returned_to_the_caller() {
    let samples = audio(data::root().join("ft8/audio/wsjtx_cq.wav"));
    let timestamp = SlotTimestamp::parse("261028_120000").unwrap();
    for adapter in ADAPTERS {
        let mut called = false;
        let result = adapter
            .session(StreamDecodeConfig {
                hiscall: Some("K1ABC".into()),
                ..Default::default()
            })
            .decode(&timestamp, &samples, &mut |_| {
                called = true;
                Err("test callback stopped".into())
            });
        assert!(called, "{} did not invoke callback", adapter.name);
        assert!(
            matches!(result, Err(ref error) if error.contains("test callback stopped")),
            "{} swallowed callback error",
            adapter.name
        );
    }
}

#[test]
#[ignore = "serial optimized real-recording snapshot; optionally set FT8RS_DECODER_REPORT_DIR"]
fn historical_recordings_preserve_required_rows_and_export_per_message_snapshots() {
    use std::io::Write;
    for (file, start, long) in [
        ("210703_133430", "210703_133430", false),
        ("230208_140300", "230208_140300", true),
    ] {
        let samples = audio(format!("ft8/{file}.wav"));
        let baseline =
            std::fs::read_to_string(support::root().join(format!("ft8/{file}.csv"))).unwrap();
        for adapter in ADAPTERS {
            let config = StreamDecodeConfig {
                nfa: if long { 200.0 } else { 100.0 },
                jtdx_threads: if long && adapter.name == "jtdx" { 4 } else { 1 },
                mycall: (adapter.target_only && long).then(|| "F1MLZ".into()),
                hiscall: adapter
                    .target_only
                    .then(|| if long { "UA3QNA" } else { "F5RXL" }.into()),
                ..Default::default()
            };
            let mut session = adapter.session(config.clone());
            let mut rows = Vec::new();
            let start = SlotTimestamp::parse(start).unwrap();
            for (slot, chunk) in samples.chunks(180000).enumerate() {
                let mut padded = vec![0.0; 180000];
                padded[..chunk.len()].copy_from_slice(chunk);
                let result = replay(
                    session.as_mut(),
                    &start.add_seconds(slot as i64 * 15),
                    &padded,
                );
                rows.extend(result.rows.into_iter().map(|row| (slot, row)));
            }
            let mut missing = Vec::new();
            let mut targets = 0;
            for line in baseline.lines().skip(1) {
                let parts: Vec<_> = line.split(',').collect();
                let marker = parts.get(5).copied().unwrap_or("").trim();
                let included = match adapter.name {
                    "wsjtx" => !matches!(marker, "J" | "E"),
                    "jtdx" => !matches!(marker, "W" | "E"),
                    _ => marker != "E",
                };
                if !included || adapter.target_only {
                    continue;
                }
                let timestamp = SlotTimestamp::parse(parts[0]).unwrap();
                let slot = if long {
                    let time = timestamp.format_time();
                    let minutes: usize = time[2..4].parse().unwrap();
                    let seconds: usize = time[4..6].parse().unwrap();
                    ((minutes - 3) * 60 + seconds) / 15
                } else {
                    0
                };
                targets += 1;
                if !rows
                    .iter()
                    .any(|(s, row)| *s == slot && normal(&row.msg) == normal(parts[4]))
                {
                    missing.push(format!("{slot}: {}", parts[4]));
                }
            }
            eprintln!(
                "{} {file}: rows={} target={}/{} missing={missing:?}",
                adapter.name,
                rows.len(),
                targets - missing.len(),
                targets
            );
            if let Some(directory) = std::env::var_os("FT8RS_DECODER_REPORT_DIR") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let mut output =
                    std::fs::File::create(directory.join(format!("{}-{file}.tsv", adapter.name)))
                        .unwrap();
                writeln!(output, "# config={config:?}").unwrap();
                rows.sort_by(|(sa, a), (sb, b)| {
                    sa.cmp(sb)
                        .then_with(|| a.msg.cmp(&b.msg))
                        .then_with(|| a.freq.total_cmp(&b.freq))
                });
                for (slot, row) in &rows {
                    writeln!(
                        output,
                        "{slot}\t{}\t{:.9}\t{:.9}\t{:.9}",
                        normal(&row.msg),
                        row.freq,
                        row.dt,
                        row.snr
                    )
                    .unwrap();
                }
            }
            let required = match (adapter.name, long) {
                ("wsjtx", false) => Some(21),
                ("wsjtx", true) => Some(424),
                ("jtdx", false) => Some(20),
                ("jtdx", true) => Some(430),
                _ => None,
            };
            if let Some(required) = required {
                assert!(
                    targets - missing.len() >= required,
                    "{} {file}: {missing:?}",
                    adapter.name
                );
            }
            if adapter.name == "jtdx" && long {
                // Preserve identities, not just the 430 count: recovering the
                // known miss must never compensate for losing a different row.
                assert!(
                    missing.iter().all(|row| row == "16: F1MLZ UA3QNA -04"),
                    "JTDX lost a previously required message: {missing:?}"
                );
            }
            if adapter.target_only {
                assert!(rows.iter().all(|(_, r)| normal(&r.msg)
                    .split_whitespace()
                    .any(|w| Some(w) == config.hiscall.as_deref())));
            }
        }
    }
}
