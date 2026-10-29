use crate::decode::{
    gen_ft8wave::gen_ft8wave, genft8::get_ft8_tones_from_77bits, packjt77::pack77,
};
use crate::stream::{DecodeProfile, ProfileStreamDecodeSession, SlotTimestamp, StreamDecodeConfig};

#[test]
fn configured_target_resolves_hashed_message_in_all_profiles_and_after_reconfigure() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let tones = get_ft8_tones_from_77bits(&pack77("<6W/IK2HKT> JI1NFN PM96"));
    let (wave, _) = gen_ft8wave(&tones, 1000.0);
    let mut samples = vec![0.0_f32; 180_000];
    let mut rng = 12345_u64;
    for sample in &mut samples {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        *sample = ((rng >> 32) as f32 / u32::MAX as f32 - 0.5) * 0.01;
    }
    for (dest, value) in samples[6000..].iter_mut().zip(wave) {
        *dest += value as f32 * 0.1;
    }
    let timestamp = SlotTimestamp::parse("261029_020945").unwrap();
    for profile in [
        DecodeProfile::Wsjtx,
        DecodeProfile::Jtdx,
        DecodeProfile::Hybrid,
        DecodeProfile::Dx,
    ] {
        let config = StreamDecodeConfig {
            profile,
            hiscall: Some("6W/IK2HKT".into()),
            hybrid_sic: false,
            ..Default::default()
        };
        let original = ProfileStreamDecodeSession::new(StreamDecodeConfig {
            hiscall: Some("K1ABC".into()),
            ..config.clone()
        });
        let mut session = original.reconfigure(config, true, true);
        let mut messages = Vec::new();
        session
            .decode_slot_streaming_with_provenance_at(&timestamp, &samples, |row| {
                messages.push(row.decode.msg.clone());
                Ok(())
            })
            .unwrap();
        assert!(
            messages
                .iter()
                .any(|msg| msg.contains("6W/IK2HKT") && msg.contains("JI1NFN")),
            "{profile:?}: {messages:?}"
        );
    }
}
