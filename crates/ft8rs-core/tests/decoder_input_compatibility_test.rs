use ft8rs::decode::lib_jtdx::ft8v2::packjt77::{pack77, unpack77};
use ft8rs::input::audio::{downsample_12k, read_wav_mono_f32};
use ft8rs::input::{decode_wav_file, FileDecodeOptions};
use ft8rs::stream::slot::decode_12k_slots;
use ft8rs::{SlotTimestamp, StreamDecodeConfig};

fn rms(samples: &[f32]) -> f64 {
    let tail = &samples[500..];
    (tail.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / tail.len() as f64).sqrt()
}

fn tone(frequency: f64, rate: u32) -> Vec<f32> {
    (0..rate)
        .map(|i| (std::f64::consts::TAU * frequency * f64::from(i) / f64::from(rate)).sin() as f32)
        .collect()
}

#[test]
fn downsampling_keeps_in_band_audio_and_rejects_alias_sources() {
    for rate in [44_100, 48_000, 96_000] {
        let pass = downsample_12k(&tone(1000.0, rate), rate);
        let stop = downsample_12k(&tone(9000.0, rate), rate);
        assert_eq!(pass.len(), 12_000);
        // WSJT-X's unchanged 49-tap fil4 has its own passband ripple:
        // its 1 kHz response is about 0.94443, giving this reference RMS.
        let expected = if rate == 48_000 {
            0.66781
        } else {
            std::f64::consts::FRAC_1_SQRT_2
        };
        assert!((rms(&pass) - expected).abs() < 0.001);
        assert!(rms(&stop) < 0.01, "{rate} Hz input leaked an alias source");
    }
}

fn set_bits(bits: &mut [u8], start: usize, width: usize, value: usize) {
    for (i, bit) in bits[start..start + width].iter_mut().enumerate() {
        *bit = ((value >> (width - i - 1)) & 1) as u8;
    }
}

#[test]
fn jtdx_field_day_payload_uses_the_original_section_numbers() {
    let template = pack77("K1ABC W9XYZ 1A AB");
    for (section, expected) in [(16, "GTA"), (25, "MAR"), (44, "NT"), (85, "PE")] {
        let mut bits = template.clone();
        set_bits(&mut bits, 64, 7, section);
        assert_eq!(
            unpack77(&bits, None).unwrap(),
            format!("K1ABC W9XYZ 1A {expected}")
        );
    }
    let mut reserved = template;
    set_bits(&mut reserved, 64, 7, 86);
    assert!(unpack77(&reserved, None).is_none());
}

#[test]
fn jtdx_rtty_multiplier_range_matches_the_original_protocol() {
    let mut bits = pack77("K1ABC W9XYZ 579 AL");
    set_bits(&mut bits, 61, 13, 8065);
    assert_eq!(unpack77(&bits, None).unwrap(), "K1ABC W9XYZ 579 DC");
    set_bits(&mut bits, 61, 13, 8066);
    assert!(unpack77(&bits, None).is_none());
}

#[test]
fn wav_and_live_paths_apply_the_same_48k_antialias_filter() {
    let fixture = read_wav_mono_f32("tests/ft8/210703_133430.wav").unwrap();
    let samples: Vec<f32> = fixture
        .samples
        .iter()
        .flat_map(|sample| [*sample; 4])
        .enumerate()
        .map(|(i, sample)| {
            sample + (std::f64::consts::TAU * 9000.0 * i as f64 / 48_000.0).sin() as f32 * 0.3
        })
        .collect();
    let path = std::env::temp_dir().join(format!(
        "ft8rs-antialias-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for sample in &samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();

    let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
    let config = StreamDecodeConfig::default();
    let result = decode_wav_file(
        &path,
        FileDecodeOptions {
            start_time: timestamp.clone(),
            config: config.clone(),
        },
    );
    std::fs::remove_file(path).unwrap();
    let from_file = result.unwrap();
    let from_live = decode_12k_slots(&downsample_12k(&samples, 48_000), timestamp, config);
    let keys = |rows: Vec<ft8rs::stream::slot::TimestampedDecode>| {
        rows.into_iter()
            .map(|row| (row.decode.msg, row.decode.snr.to_bits()))
            .collect::<Vec<_>>()
    };
    assert!(!from_file.is_empty());
    assert_eq!(keys(from_file), keys(from_live));
}
