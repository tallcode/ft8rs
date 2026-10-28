use ft8rs::decode::dx::DxStreamDecodeSession;
use ft8rs::input::audio::read_wav_mono_f32;
use ft8rs::{DecodeProfile, SlotTimestamp, StreamDecodeConfig};

#[test]
fn native_observations_keep_target_and_reconfiguration_drops_old_target_state() {
    if cfg!(debug_assertions) {
        panic!("run decoder tests optimized");
    }
    let audio = read_wav_mono_f32("tests/ft8/210703_133430.wav").unwrap();
    let config = StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        hiscall: Some("F5RXL".into()),
        ..Default::default()
    };
    let mut session = DxStreamDecodeSession::new(config.clone());
    let rows = session.decode_slot_at(
        &SlotTimestamp::parse("210703_133430").unwrap(),
        &audio.samples,
    );
    assert!(rows.iter().any(|r| r.msg == "CQ F5RXL IN94"));
    assert!(rows
        .iter()
        .all(|r| r.msg.split_whitespace().any(|c| c == "F5RXL")));
    assert!(session
        .last_diagnostics()
        .observation_rows
        .iter()
        .all(|n| *n > 0));
    assert!(session.last_diagnostics().augmentation_errors.is_empty());
    let next = session.reconfigured(
        StreamDecodeConfig {
            hiscall: Some("HA0DU".into()),
            ..config
        },
        true,
        false,
    );
    assert_eq!(next.context_snapshot().target, "HA0DU");
    assert!(next.context_snapshot().hisgrid.is_none());
    assert!(next.context_snapshot().foci.is_empty());
}
