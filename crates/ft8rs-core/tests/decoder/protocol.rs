//! FT8 golden vectors and channel-code experiments shared by native backends.
mod data;
mod protocol_adapters;
use protocol_adapters::{Backend, BACKENDS};

struct Vector {
    input: String,
    decoded: String,
    bits: Vec<u8>,
    codeword: Vec<u8>,
    tones: Vec<i32>,
    receive_valid: bool,
}

fn vectors(backend: Backend) -> Vec<Vector> {
    let source = data::read(&format!("ft8/protocol/{}.tsv", backend.name()));
    source
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 6);
            let bits = |s: &str, count| {
                assert_eq!(s.len(), count);
                s.bytes()
                    .map(|b| {
                        assert!(b == b'0' || b == b'1');
                        b - b'0'
                    })
                    .collect()
            };
            assert_eq!(fields[4].len(), 79);
            Vector {
                input: fields[0].into(),
                decoded: fields[1].into(),
                bits: bits(fields[2], 77),
                codeword: bits(fields[3], 174),
                tones: fields[4]
                    .bytes()
                    .map(|b| {
                        assert!((b'0'..=b'7').contains(&b));
                        (b - b'0') as i32
                    })
                    .collect(),
                receive_valid: fields[5].ends_with('T'),
            }
        })
        .collect()
}

#[test]
fn upstream_message_bits_crc_parity_tones_and_receive_unpack() {
    let call_data = data::read("ft8/protocol/calls.txt");
    let calls: Vec<_> = call_data.lines().collect();
    let mut errors = Vec::new();
    for backend in BACKENDS {
        let vectors = vectors(backend);
        let inputs: Vec<_> = vectors.iter().map(|v| v.input.as_str()).collect();
        let messages = data::read("ft8/protocol/messages.txt");
        assert!(!inputs.is_empty(), "upstream FT8 corpus must not be empty");
        assert_eq!(
            inputs,
            messages.lines().collect::<Vec<_>>(),
            "incomplete upstream message corpus"
        );
        for vector in vectors {
            let label = format!("{backend:?}: {}", vector.input);
            if backend.pack(&vector.input) != vector.bits {
                errors.push(format!("{label}: pack77"));
            }
            let expected = vector.receive_valid.then_some(vector.decoded.as_str());
            if backend.unpack(&vector.bits, &calls).as_deref() != expected {
                errors.push(format!(
                    "{label}: unpack expected {:?}, got {:?}",
                    expected,
                    backend.unpack(&vector.bits, &calls)
                ));
            }
            if backend.encode(&vector.bits) != vector.codeword {
                errors.push(format!("{label}: CRC/LDPC encoding"));
            }
            // Both engines use the shared FT8 symbol mapper. Its input is the
            // independent native codeword, not bits re-encoded by Rust.
            if crate::decode::genft8::get_ft8_tones_from_codeword(&vector.codeword).as_slice()
                != vector.tones
            {
                errors.push(format!("{label}: tone order"));
            }
        }
    }
    assert!(
        errors.is_empty(),
        "upstream vector mismatches:\n{}",
        errors.join("\n")
    );
}

#[test]
fn original_callsign_hash_vectors_for_each_backend() {
    for line in data::read("ft8/protocol/hash.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
    {
        let fields: Vec<_> = line.split('\t').collect();
        let hashes = std::array::from_fn(|i| fields[i + 1].parse().unwrap());
        for backend in BACKENDS {
            assert!(
                backend.hash_matches(fields[0], hashes),
                "{backend:?} hash: {:?}",
                fields[0]
            );
        }
    }
}

#[test]
fn crc_rejects_every_single_bit_corruption_in_message_and_crc() {
    for backend in BACKENDS {
        for vector in vectors(backend) {
            let word = &vector.codeword[..91];
            assert!(backend.crc_valid(word), "{backend:?}: {}", vector.input);
            for bit in 0..91 {
                let mut corrupt = word.to_vec();
                corrupt[bit] ^= 1;
                assert!(
                    !backend.crc_valid(&corrupt),
                    "{backend:?}: {} corrupt bit {bit}",
                    vector.input
                );
            }
        }
    }
}

#[test]
fn bp_and_osd_recover_known_words_with_soft_errors_and_ap_masks() {
    if cfg!(debug_assertions) {
        panic!("run decoder tests with --profile fast or --release");
    }
    for backend in BACKENDS {
        for vector in vectors(backend) {
            for errors in [0, 1, 4, 8] {
                let mut llr: [f64; 174] =
                    std::array::from_fn(|i| (2.0 * vector.codeword[i] as f64 - 1.0) * 8.0);
                for i in 0..errors {
                    llr[(i * 19 + 3) % 174] *= -0.025;
                }
                for ap_bits in [0, 28] {
                    let mut mask = [0; 174];
                    for i in 0..ap_bits {
                        mask[i] = 1;
                        llr[i] = (2.0 * vector.codeword[i] as f64 - 1.0) * 12.0;
                    }
                    for (method, decoded) in [
                        ("bp", backend.bp(&llr, &mask)),
                        ("osd", backend.osd(&llr, &mask)),
                    ] {
                        assert_eq!(
                            decoded.as_deref(),
                            Some(vector.codeword.as_slice()),
                            "{backend:?} {method}: {} errors={errors} AP={ap_bits}",
                            vector.input
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn all_zero_codeword_is_valid_at_the_channel_code_layer() {
    // Zero is a legitimate LDPC/CRC word. Signal detection and semantic
    // validation, exercised by the audio negative cases, must reject silence.
    for backend in BACKENDS {
        assert_eq!(
            backend.bp(&[-8.0; 174], &[0; 174]).as_deref(),
            Some([0u8; 174].as_slice())
        );
        assert_eq!(
            backend.osd(&[-8.0; 174], &[0; 174]).as_deref(),
            Some([0u8; 174].as_slice())
        );
    }
}

#[test]
fn native_waveform_reconstruction_and_subtraction() {
    use crate::decode::{gen_ft8wave, lib_jtdx, subtractft8};
    for source in BACKENDS {
        let path = data::root().join(format!("ft8/waveforms/{}.wav", source.name()));
        let audio = crate::input::audio::read_wav_mono_f32(path).unwrap();
        let original = audio.samples;
        let vector = vectors(source)
            .into_iter()
            .find(|v| v.input == "CQ K1ABC FN42")
            .unwrap();
        let tones: [i32; 79] = vector.tones.try_into().unwrap();
        let energy = |samples: &[f64]| samples.iter().map(|v| v * v).sum::<f64>();
        let input: Vec<_> = original.iter().map(|&v| v as f64).collect();
        let before = energy(&input);
        for backend in BACKENDS {
            let residual = match backend {
                Backend::Wsjtx => {
                    let mut copy = input.clone();
                    subtractft8::subtract_ft8(&mut copy, &tones, 1000.0, 0.5);
                    copy
                }
                Backend::Jtdx => {
                    let mut copy = original.clone();
                    lib_jtdx::ft8v2::subtractft8::subtractft8(
                        &mut copy, &tones, 1000.0, 0.5, false,
                    );
                    copy.into_iter().map(f64::from).collect()
                }
            };
            assert!(residual.iter().all(|v| v.is_finite()));
            let ratio = energy(&residual) / before;
            eprintln!("{backend:?} subtract {source:?}: residual energy={ratio:.6}");
            assert!(
                ratio < 0.01,
                "{backend:?} failed native subtraction: {ratio}"
            );
        }
        for backend in BACKENDS {
            let (_, reconstructed) = match backend {
                Backend::Wsjtx => gen_ft8wave::gen_ft8wave(&tones, 1000.0),
                Backend::Jtdx => lib_jtdx::gen_ft8wave::gen_ft8wave(&tones, 1000.0),
            };
            let reference = &input[6000..6000 + 151680];
            let expected_scale = 12000.0 / 32768.0;
            let mismatch = reference
                .iter()
                .zip(&reconstructed)
                .map(|(a, b)| (a - b * expected_scale).powi(2))
                .sum::<f64>()
                / before;
            assert!(
                mismatch < 0.0004,
                "{backend:?} reconstruction {source:?}: relative MSE={mismatch}"
            );
        }
    }
}

mod random;

#[test]
fn seeded_awgn_and_block_rayleigh_report_correct_and_undetected_decodes() {
    if cfg!(debug_assertions) {
        panic!("run channel experiments optimized");
    }
    for backend in BACKENDS {
        let vectors = vectors(backend);
        for rayleigh in [false, true] {
            for sigma in [0.25, 0.6, 1.2] {
                let mut noise = random::Noise::new(2026102801);
                let mut good = 0;
                let mut undetected = 0;
                for trial in 0..16 {
                    let vector = &vectors[trial * 3 % vectors.len()];
                    let amplitude = if rayleigh {
                        (noise.gaussian().powi(2) + noise.gaussian().powi(2)).sqrt() / 2f64.sqrt()
                    } else {
                        1.0
                    };
                    let llr = std::array::from_fn(|i| {
                        let rx = amplitude * (2.0 * vector.codeword[i] as f64 - 1.0)
                            + sigma * noise.gaussian();
                        2.0 * amplitude * rx / (sigma * sigma)
                    });
                    let decoded = backend
                        .bp(&llr, &[0; 174])
                        .or_else(|| backend.osd(&llr, &[0; 174]));
                    if let Some(word) = decoded {
                        if word == vector.codeword {
                            good += 1;
                        } else {
                            undetected += 1;
                        }
                    }
                }
                eprintln!("{backend:?} rayleigh={rayleigh} sigma={sigma}: good={good}/16 undetected={undetected}");
                assert_eq!(
                    undetected, 0,
                    "{backend:?} rayleigh={rayleigh} sigma={sigma}"
                );
                if !rayleigh && sigma == 0.25 {
                    assert_eq!(good, 16);
                }
            }
        }
    }
}

/// Called inside each native module so private unpack28 remains private in
/// production. Only the shared corpus and assertions live in this harness.
pub(crate) fn assert_original_pack28_vectors(
    backend: &str,
    pack: impl Fn(&str) -> usize,
    unpack: impl Fn(usize, Option<&str>) -> Option<String>,
) {
    let corpus = data::read("ft8/calls/inputs.txt");
    let source = data::read(&format!("ft8/calls/{backend}.tsv"));
    let mut inputs = Vec::new();
    let mut errors = Vec::new();
    let canonical = |s: &str| s.replace("CQ_", "CQ ").trim().to_owned();
    for line in source
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
    {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 7);
        let number: usize = fields[2].parse().unwrap();
        let seed = (fields[0] == "pack").then_some(fields[1]);
        if let Some(token) = seed {
            inputs.push(token);
            let actual = pack(token);
            if actual != number {
                errors.push(format!("{token:?}: pack28 expected {number}, got {actual}"));
            }
        }
        for (context, text, flag) in [(None, fields[3], fields[4]), (seed, fields[5], fields[6])] {
            assert!(matches!(flag, "T" | "F"));
            let expected = (flag == "T").then(|| canonical(text));
            let actual = unpack(number, context).map(|value| canonical(&value));
            if actual != expected {
                errors.push(format!(
                    "{} n28={number} seed={context:?}: expected {expected:?}, got {actual:?}",
                    fields[1]
                ));
            }
        }
    }
    assert_eq!(
        inputs,
        corpus.lines().collect::<Vec<_>>(),
        "incomplete original test28 corpus"
    );
    assert!(
        errors.is_empty(),
        "{backend} test28 mismatches:\n{}",
        errors.join("\n")
    );
}
