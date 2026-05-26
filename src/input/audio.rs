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
