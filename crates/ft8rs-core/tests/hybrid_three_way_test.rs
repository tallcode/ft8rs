use ft8rs::decode::hybrid::HybridStreamDecodeSession;
use ft8rs::decode::lib_jtdx::JtdxStreamDecodeSession;
use ft8rs::input::audio::read_wav_mono_f32;
use ft8rs::{SlotTimestamp, StreamDecodeConfig, StreamDecodeSession};

#[test]
fn three_way_output_retains_each_native_path_with_swl_off_and_on() {
    if cfg!(debug_assertions) {
        panic!("run decoder tests with --profile fast or --release");
    }
    let audio = read_wav_mono_f32("tests/ft8/210703_133430.wav").unwrap();
    let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
    for swl in [false, true] {
        let config = StreamDecodeConfig {
            swl,
            hybrid_sic: false,
            ..Default::default()
        };
        let mut expected = StreamDecodeSession::new(config.clone_for_profile_wsjt_x())
            .decode_slot_streaming_with_provenance_at(&timestamp, &audio.samples, |_| Ok(()))
            .unwrap()
            .into_iter()
            .map(|row| row.decode)
            .collect::<Vec<_>>();
        for nagcc in [false, true] {
            expected.extend(
                JtdxStreamDecodeSession::new(StreamDecodeConfig {
                    nagcc,
                    ..config.clone()
                })
                .decode_slot_streaming_at(&timestamp, &audio.samples, |_| Ok(()))
                .unwrap(),
            );
        }
        let mut hybrid = HybridStreamDecodeSession::new(config);
        let actual = hybrid
            .decode_slot_streaming_at(&timestamp, &audio.samples, |_| Ok(()))
            .unwrap();
        assert!(hybrid.last_diagnostics().agc_rows > 0);
        for row in &expected {
            assert!(
                actual.iter().any(|other| matches(row, other)),
                "SWL={swl}: dropped {}",
                row.msg
            );
        }
        for (index, row) in actual.iter().enumerate() {
            assert!(
                expected.iter().any(|other| matches(row, other)),
                "unexpected output {}",
                row.msg
            );
            assert!(
                !actual[..index].iter().any(|other| matches(row, other)),
                "duplicate {}",
                row.msg
            );
        }
    }
}

fn matches(a: &ft8rs::StreamDecodedMessage, b: &ft8rs::StreamDecodedMessage) -> bool {
    let normal = |message: &str| {
        message
            .split_whitespace()
            .map(|token| {
                if token == "<...>" {
                    token.to_owned()
                } else {
                    token.trim_matches(['<', '>']).to_ascii_uppercase()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    normal(&a.msg) == normal(&b.msg) && (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}
