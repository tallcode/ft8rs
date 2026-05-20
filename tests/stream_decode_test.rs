/// Stream decoder tests.
use ft8rs::stream::{StreamDecoder, StreamDecodeConfig, AudioBuffer, DecodeStage};
use ft8rs::stream::cross_slot::{CrossSlotMemory, SavedDecode};
use ft8rs::encode_ft8;
use ft8rs::util::waveform::WaveformOptions;

#[test]
fn test_stream_decoder_builds() {
    let config = StreamDecodeConfig::default();
    let _decoder = StreamDecoder::new(config);
}

#[test]
fn test_audio_buffer_stages() {
    let mut buf = AudioBuffer::new(12000);
    assert_eq!(buf.stage(), DecodeStage::Insufficient);

    // Push 11s of audio
    let samples = vec![0.0f32; 11 * 12000];
    buf.push(&samples);
    let stage1 = buf.stage();
    println!("After 11s: stage={:?}, nzhsym={}", stage1, buf.nzhsym());

    // Push more to get to 13s
    let more = vec![0.0f32; 2 * 12000];
    buf.push(&more);
    let stage2 = buf.stage();
    println!("After 13s: stage={:?}, nzhsym={}", stage2, buf.nzhsym());

    // Push to 15s
    let more2 = vec![0.0f32; 2 * 12000];
    buf.push(&more2);
    let stage3 = buf.stage();
    println!("After 15s: stage={:?}, nzhsym={}", stage3, buf.nzhsym());
}

#[test]
fn test_stream_decode_15s_window() {
    let messages = vec![
        "CQ TEST M1ABC IO91".to_string(),
        "M1ABC G2DEF IO91".to_string(),
        "G2DEF M1ABC RRR".to_string(),
    ];

    let sr = 12000f64;
    let mut full_audio = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        let wav = encode_ft8(msg, WaveformOptions {
            sample_rate: Some(sr),
            base_frequency: Some(1000.0 + (i as f64) * 500.0),
            ..Default::default()
        });
        full_audio.extend_from_slice(&wav);
    }

    let target = (sr * 15.0).ceil() as usize;
    while full_audio.len() < target {
        full_audio.push(0.0);
    }

    let config = StreamDecodeConfig::default();
    let mut decoder = StreamDecoder::new(config);

    let chunk_size = (sr * 2.0) as usize;
    for chunk in full_audio.chunks(chunk_size) {
        decoder.push_audio(chunk);
        decoder.process();
    }

    let results = decoder.finish_slot();
    println!("Stream decoded {} messages", results.len());
    for r in &results {
        println!("  {} at {:.1}Hz, {:.1}s, SNR={:.1}", r.msg, r.freq, r.dt, r.snr);
    }

    assert!(results.len() >= 1, "Expected at least 1 decoded message, got {}", results.len());
}

#[test]
fn test_cross_slot_memory() {
    let mut memory = CrossSlotMemory::new();

    let d1 = SavedDecode {
        freq: 1000.0, dt: 0.5, msg: "CQ TEST M1ABC IO91".to_string(),
        itone: [0; 79], snr: -10.0, sync: 2.0, subtracted: false,
    };
    let d2 = SavedDecode {
        freq: 1500.0, dt: 1.0, msg: "M1ABC G2DEF IO91".to_string(),
        itone: [0; 79], snr: -15.0, sync: 1.5, subtracted: false,
    };

    assert!(memory.save(d1));
    assert!(memory.save(d2));

    let d1_dup = SavedDecode {
        freq: 1000.0, dt: 0.5, msg: "CQ TEST M1ABC IO91".to_string(),
        itone: [0; 79], snr: -10.0, sync: 2.0, subtracted: false,
    };
    assert!(!memory.save(d1_dup));

    assert_eq!(memory.count(), 2);
    assert_eq!(memory.previous_count(), 0);

    memory.rotate_slot();
    assert_eq!(memory.count(), 0);
    assert_eq!(memory.previous_count(), 2);
}
