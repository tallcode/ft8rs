use std::path::Path;

use hound::WavReader;

#[derive(Clone, Debug)]
pub struct AudioSamples {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

pub fn read_wav_mono_f32(path: impl AsRef<Path>) -> Result<AudioSamples, String> {
    let reader = WavReader::open(path.as_ref())
        .map_err(|err| format!("failed to open WAV {}: {err}", path.as_ref().display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err("WAV has zero channels".to_string());
    }

    let channels = spec.channels as usize;
    let samples = match spec.sample_format {
        hound::SampleFormat::Int => read_int_samples(reader, spec.bits_per_sample, channels)?,
        hound::SampleFormat::Float => read_float_samples(reader, channels)?,
    };

    Ok(AudioSamples {
        sample_rate: spec.sample_rate,
        samples,
    })
}

/// Reduce a capture stream to the decoder's 12 kHz working rate the way WSJT-X
/// does. For the overwhelmingly common 4:1 case (a 48 kHz sound card / virtual
/// cable such as FlexRadio DAX) this applies WSJT-X's exact `fil4` anti-alias
/// FIR before decimating, so out-of-band energy above ~6 kHz can't fold back
/// into the FT8 passband. Other source rates fall back to linear interpolation
/// (WSJT-X sidesteps them by asking the OS for 48 kHz in the first place).
///
/// This matters only on the live soundcard path: WAV fixtures are already 12 kHz
/// so the decoder's acceptance tests never exercise resampling.
pub fn downsample_12k(src: &[f32], from_rate: u32) -> Vec<f32> {
    const TARGET: u32 = 12_000;
    if from_rate == 4 * TARGET {
        return fil4_decimate(src);
    }
    resample_linear(src, from_rate, TARGET)
}

/// WSJT-X's `lib/fil4.f90`: a 49-tap linear-phase FIR low-pass (fc 4500 Hz,
/// fstop 6000 Hz, 40 dB stop) that decimates 48 kHz → 12 kHz by 4. Ported
/// faithfully (same coefficients, same 4-sample sliding window seeded with
/// zeros); we keep the math in f32 rather than requantizing to i16 each tap,
/// which only raises fidelity.
fn fil4_decimate(src: &[f32]) -> Vec<f32> {
    const NDOWN: usize = 4;
    const NTAPS: usize = 49;
    // Coefficients copied verbatim from lib/fil4.f90.
    const W: [f32; NTAPS] = [
        0.000861074040,
        0.010051920210,
        0.010161983649,
        0.011363155076,
        0.008706594219,
        0.002613872664,
        -0.005202883094,
        -0.011720748164,
        -0.013752163325,
        -0.009431602741,
        0.000539063909,
        0.012636767098,
        0.021494659597,
        0.021951235065,
        0.011564169382,
        -0.007656470131,
        -0.028965787341,
        -0.042637874109,
        -0.039203309748,
        -0.013153301537,
        0.034320769178,
        0.094717832646,
        0.154224604789,
        0.197758325022,
        0.213715139513,
        0.197758325022,
        0.154224604789,
        0.094717832646,
        0.034320769178,
        -0.013153301537,
        -0.039203309748,
        -0.042637874109,
        -0.028965787341,
        -0.007656470131,
        0.011564169382,
        0.021951235065,
        0.021494659597,
        0.012636767098,
        0.000539063909,
        -0.009431602741,
        -0.013752163325,
        -0.011720748164,
        -0.005202883094,
        0.002613872664,
        0.008706594219,
        0.011363155076,
        0.010161983649,
        0.010051920210,
        0.000861074040,
    ];

    let n2 = src.len() / NDOWN;
    let mut out = Vec::with_capacity(n2);
    // Sliding tap history, seeded with zeros exactly like the Fortran `t`.
    let mut t = [0.0f32; NTAPS];
    let mut k = 0usize;
    for _ in 0..n2 {
        // Shift old data down by NDOWN, then insert the next NDOWN input samples
        // at the end (mirrors the two array-slice assignments in fil4.f90).
        t.copy_within(NDOWN.., 0);
        t[NTAPS - NDOWN..].copy_from_slice(&src[k..k + NDOWN]);
        k += NDOWN;
        out.push(W.iter().zip(t.iter()).map(|(w, s)| w * s).sum());
    }
    out
}

pub fn resample_linear(src: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return src.to_vec();
    }
    if src.is_empty() {
        return Vec::new();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((src.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let lo = src_pos.floor() as usize;
        let frac = src_pos - lo as f64;
        let v0 = *src.get(lo).unwrap_or(&0.0) as f64;
        let v1 = *src.get(lo + 1).unwrap_or(&0.0) as f64;
        out.push((v0 * (1.0 - frac) + v1 * frac) as f32);
    }
    out
}

fn read_int_samples(
    reader: WavReader<std::io::BufReader<std::fs::File>>,
    bits_per_sample: u16,
    channels: usize,
) -> Result<Vec<f32>, String> {
    let scale = match bits_per_sample {
        8 => 128.0,
        16 => 32768.0,
        24 => 8_388_608.0,
        32 => 2_147_483_648.0,
        _ => {
            return Err(format!(
                "unsupported integer WAV bit depth: {bits_per_sample}"
            ))
        }
    };

    fold_interleaved_channels(
        reader.into_samples::<i32>().map(|sample| {
            sample
                .map(|value| value as f32 / scale)
                .map_err(|err| format!("failed to read WAV sample: {err}"))
        }),
        channels,
    )
}

fn read_float_samples(
    reader: WavReader<std::io::BufReader<std::fs::File>>,
    channels: usize,
) -> Result<Vec<f32>, String> {
    fold_interleaved_channels(
        reader
            .into_samples::<f32>()
            .map(|sample| sample.map_err(|err| format!("failed to read WAV sample: {err}"))),
        channels,
    )
}

fn fold_interleaved_channels<I>(samples: I, channels: usize) -> Result<Vec<f32>, String>
where
    I: IntoIterator<Item = Result<f32, String>>,
{
    if channels == 1 {
        return samples.into_iter().collect();
    }

    let mut out = Vec::new();
    let mut acc = 0.0f32;
    let mut pos = 0usize;
    for sample in samples {
        acc += sample?;
        pos += 1;
        if pos == channels {
            out.push(acc / channels as f32);
            acc = 0.0;
            pos = 0;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(freq: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        // Skip the FIR warm-up (leading zeros in the tap history) so the estimate
        // reflects steady state, not the ramp-in.
        let tail = &samples[samples.len() / 4..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    // A 1 kHz in-band tone must survive the decimation with roughly full
    // amplitude (fil4 passband reaches to 4.5 kHz).
    #[test]
    fn fil4_passes_in_band_tone() {
        let out = downsample_12k(&tone(1_000.0, 48_000, 48_000), 48_000);
        assert_eq!(out.len(), 12_000);
        let level = rms(&out) * 2f32.sqrt(); // sine RMS → amplitude
        assert!(level > 0.9, "1 kHz tone should pass ~unattenuated, got {level}");
    }

    // A 9 kHz tone would fold to |9000 - 12000| = 3 kHz — right inside the FT8
    // band — under naive 4:1 decimation. fil4's 40 dB stop must crush it; linear
    // interpolation does not, which is the whole reason for this alignment.
    #[test]
    fn fil4_rejects_aliasing_tone_that_linear_lets_through() {
        let src = tone(9_000.0, 48_000, 48_000);
        let fil4 = rms(&downsample_12k(&src, 48_000));
        let linear = rms(&resample_linear(&src, 48_000, 12_000));
        assert!(
            fil4 < 0.05,
            "fil4 should reject the 9 kHz alias source, got rms {fil4}"
        );
        assert!(
            linear > 5.0 * fil4,
            "linear interpolation should leak far more aliasing than fil4 \
             (linear {linear} vs fil4 {fil4})"
        );
    }

    #[test]
    fn downsample_non_4to1_falls_back_to_linear() {
        // 44.1 kHz → 12 kHz isn't an integer 4:1, so it should match the linear
        // path exactly (no fil4).
        let src = tone(1_000.0, 44_100, 44_100);
        assert_eq!(downsample_12k(&src, 44_100), resample_linear(&src, 44_100, 12_000));
    }
}
