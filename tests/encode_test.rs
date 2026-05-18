use std::rc::Rc;
use ft8rs::util::pack_jt77::pack77;
use ft8rs::util::unpack_jt77::unpack77;
use ft8rs::ft8::encode::{encode174_91, get_tones, encode_message};
use ft8rs::ft4::encode::{encode_message as encode_message_ft4, get_tones as get_tones_ft4};
use ft8rs::ft4::scramble::xor_with_scrambler;
use ft8rs::util::hashcall::HashCallBook;
use ft8rs::util::waveform::{generate_ft8_waveform, WaveformOptions};
use ft8rs::{decode_ft8, DecodeFT8Options};

const SAMPLE_RATE: usize = 12_000;

#[derive(Debug)]
struct FT8Vector {
    msg: &'static str,
    bits77: &'static str,
    crc14: &'static str,
    parity83: &'static str,
    tones: &'static str,
}

const FT8_VECTORS: &[FT8Vector] = &[
    FT8Vector {
        msg: "CQ K1ABC FN42",
        bits77: "00000000000000000000000000100000010011011110111100011010100010100001100110001",
        crc14: "00101100101110",
        parity83: "10101000001001000110111100001111000000111010010110111110100110100100001010010100110",
        tones: "3140652 00000000100547670460602153343 3140652 73601104751700733474545513354 3140652",
    },
    FT8Vector {
        msg: "K1ABC W9XYZ EN37",
        bits77: "00001001101111011110001101010000011000010100100111011100000010000101011001001",
        crc14: "11000101111101",
        parity83: "01110100100111100101001100101110111111000010000000010111101001111100110101011011010",
        tones: "3140652 03224752350406114700513432537 3140652 46455756156477030037617546223 3140652",
    },
    FT8Vector {
        msg: "W9XYZ K1ABC -11",
        bits77: "00001100001010010011101110000000010011011110111100011010100111111010101000001",
        crc14: "11100001011000",
        parity83: "10101010001101100001100111100001111000000000111111000110110010111111100000111011001",
        tones: "3140652 02035572500547670461746302406 3140652 53631651575170007704437750721 3140652",
    },
    FT8Vector {
        msg: "TNX BOB 73 GL",
        bits77: "01100011111011011100111011100010101001001010111000000111111101010000000000000",
        crc14: "11111110001011",
        parity83: "10101110011111010000101100110101000111011110110000100000010101111000001010000100010",
        tones: "3140652 20744714706333640177350001770 3140652 64642730654607244050367013053 3140652",
    },
];

fn bits_to_string(bits: &[u8]) -> String {
    bits.iter().map(|b| b.to_string()).collect()
}

fn format_tones(tones: &[u8]) -> String {
    let sync = &tones[0..7];
    let data1 = &tones[7..36];
    let sync2 = &tones[36..43];
    let data2 = &tones[43..72];
    let sync3 = &tones[72..79];
    format!(
        "{} {} {} {} {}",
        sync.iter().map(|b| b.to_string()).collect::<String>(),
        data1.iter().map(|b| b.to_string()).collect::<String>(),
        sync2.iter().map(|b| b.to_string()).collect::<String>(),
        data2.iter().map(|b| b.to_string()).collect::<String>(),
        sync3.iter().map(|b| b.to_string()).collect::<String>()
    )
}

#[test]
fn test_pack77_basic() {
    for v in FT8_VECTORS {
        let bits77 = pack77(v.msg);
        assert_eq!(bits_to_string(&bits77), v.bits77, "Failed for message: {}", v.msg);
    }
}

#[test]
fn test_encode174_91() {
    for v in FT8_VECTORS {
        let bits77 = pack77(v.msg);
        let codeword = encode174_91(&bits77);
        let crc14 = &codeword[77..91];
        let parity83 = &codeword[91..174];
        assert_eq!(bits_to_string(crc14), v.crc14, "CRC14 failed for: {}", v.msg);
        assert_eq!(bits_to_string(parity83), v.parity83, "Parity83 failed for: {}", v.msg);
    }
}

#[test]
fn test_get_tones() {
    for v in FT8_VECTORS {
        let bits77 = pack77(v.msg);
        let codeword = encode174_91(&bits77);
        let tones = get_tones(&codeword);
        let tones_str = format_tones(&tones);
        assert_eq!(tones_str, v.tones, "Tones failed for: {}", v.msg);
    }
}

#[test]
fn test_roundtrip_pack_unpack() {
    let messages = [
        "CQ K1ABC FN42",
        "K1ABC W9XYZ EN37",
        "W9XYZ K1ABC -11",
        "TNX BOB 73 GL",
        "CQ TEST K1ABC FN42",
        "K1ABC W9XYZ 73",
    ];

    for msg in messages {
        let bits77 = pack77(msg);
        let book = HashCallBook::new();
        let unpacked = unpack77(&bits77, Some(&book)).unwrap();
        assert_eq!(unpacked, msg, "Roundtrip failed for: {}", msg);
    }
}

#[test]
fn test_ft4_tones_length() {
    let tones = encode_message_ft4("CQ JA1ABC FN42");
    assert_eq!(tones.len(), 103);
    for t in &tones {
        assert!(*t <= 3);
    }
    // Check Costas arrays
    assert_eq!(&tones[0..4], &[0, 1, 3, 2]);
    assert_eq!(&tones[33..37], &[1, 0, 2, 3]);
    assert_eq!(&tones[66..70], &[2, 3, 1, 0]);
    assert_eq!(&tones[99..103], &[3, 2, 0, 1]);
}

#[test]
fn test_ft4_encode_pipeline() {
    let msg = "CQ JA1ABC FN42";
    let bits77 = pack77(msg);
    let scrambled = xor_with_scrambler(&bits77);
    let codeword = encode174_91(&scrambled);
    let tones = get_tones_ft4(&codeword);
    let tones_direct = encode_message_ft4(msg);
    assert_eq!(tones, tones_direct);
}

#[test]
fn test_ft8_encode_waveform() {
    let tones = encode_message("CQ K1ABC FN42");
    assert_eq!(tones.len(), 79);

    let waveform = generate_ft8_waveform(&tones, WaveformOptions {
        sample_rate: Some(SAMPLE_RATE as f64),
        samples_per_symbol: Some(1920),
        bt: Some(2.0),
        base_frequency: Some(1000.0),
        initial_phase: Some(0.0),
    });

    assert!(!waveform.is_empty());
    // FT8: 79 symbols * 1920 samples = 151680 samples
    assert_eq!(waveform.len(), 79 * 1920);
}

#[test]
fn test_ft8_roundtrip_decode() {
    let messages = [
        "CQ K1ABC FN42",
        "K1ABC W9XYZ EN37",
        "W9XYZ K1ABC -11",
    ];

    for msg in messages {
        let tones = encode_message(msg);
        let base_freq = 1000.0;
        let waveform = generate_ft8_waveform(&tones, WaveformOptions {
            sample_rate: Some(SAMPLE_RATE as f64),
            samples_per_symbol: Some(1920),
            bt: Some(2.0),
            base_frequency: Some(base_freq),
            initial_phase: Some(0.0),
        });

        // Place signal in a 15-second buffer at t=0.5s
        let nmax = 15 * SAMPLE_RATE;
        let mut full_buffer = vec![0.0f32; nmax];
        let offset = (0.5 * SAMPLE_RATE as f64).round() as usize;
        for i in 0..waveform.len() {
            if offset + i < nmax {
                full_buffer[offset + i] = waveform[i];
            }
        }

        let book = HashCallBook::new();
        let decoded = decode_ft8(&full_buffer, DecodeFT8Options {
            sample_rate: Some(SAMPLE_RATE),
            freq_low: Some(500.0),
            freq_high: Some(1500.0),
            sync_min: Some(1.0),
            depth: Some(2),
            max_candidates: Some(300),
            hash_call_book: Some(Rc::new(book)),
        });

        let expected = msg.to_uppercase();
        let found = decoded.iter().find(|d| {
            d.msg.trim().to_uppercase() == expected
        });
        assert!(found.is_some(), "Failed to decode: {}. Got: {:?}", msg, decoded.iter().map(|d| &d.msg).collect::<Vec<_>>());
        if let Some(f) = found {
            assert!((f.freq - base_freq).abs() < 10.0, "Frequency offset too large: {}", f.freq - base_freq);
        }
    }
}

/// Baseline test: 210703_133430.wav must decode all 20 messages.
/// This is the quality gate — no commit may reduce this below 20.
#[test]
fn test_20_message_baseline() {
    use std::io::Read;
    
    let mut file = std::fs::File::open("tests/ft8/210703_133430.wav").expect("Missing test WAV");
    let mut raw_data = Vec::new();
    file.read_to_end(&mut raw_data).expect("Read failed");
    
    let num_channels = u16::from_le_bytes([raw_data[22], raw_data[23]]) as usize;
    let bits_per_sample = u16::from_le_bytes([raw_data[34], raw_data[35]]) as usize;
    let mut offset = 12usize;
    let samples: Vec<f32> = loop {
        if offset + 8 > raw_data.len() { panic!("No data chunk"); }
        let chunk_id = &raw_data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([raw_data[offset+4], raw_data[offset+5], raw_data[offset+6], raw_data[offset+7]]) as usize;
        if chunk_id == b"data" {
            let end = (offset + 8 + chunk_size).min(raw_data.len());
            let raw = &raw_data[offset + 8..end];
            assert_eq!(bits_per_sample, 16);
            assert_eq!(num_channels, 1);
            let mut s = Vec::new();
            for chunk in raw.chunks_exact(2) {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0;
                s.push(sample);
            }
            break s;
        }
        offset += 8 + chunk_size;
    };
    
    let decoded = decode_ft8(&samples, DecodeFT8Options {
        sample_rate: Some(SAMPLE_RATE),
        freq_low: Some(100.0),
        freq_high: Some(3000.0),
        sync_min: Some(0.8),
        depth: Some(3),
        max_candidates: Some(300),
        hash_call_book: None,
    });
    
    // ⚠️ QUALITY GATE: must decode all 20 messages
    assert!(decoded.len() >= 20, 
        "BASELINE FAILED: decoded {} messages, need ≥20.\nDecoded:\n{}",
        decoded.len(),
        decoded.iter().map(|d| format!("  {}  {:.0}Hz  {:.2}s", d.msg, d.freq, d.dt)).collect::<Vec<_>>().join("\n")
    );
    
    // Verify the 4 historically-missed weak signals are present
    let missed_signals = [
        "KD2UGC F6GCP R-23",
        "K1BZM EA3CJ JN01", 
        "WA2FZW DL5AXX RR73",
        "CQ EA2BFM IN83",
    ];
    for expected in &missed_signals {
        let found = decoded.iter().any(|d| d.msg.trim().to_uppercase() == *expected);
        assert!(found, "Critical weak signal not decoded: {}", expected);
    }
}
