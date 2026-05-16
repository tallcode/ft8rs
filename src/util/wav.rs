/// WAV file parsing and writing.

use std::io::Write;

pub struct WavData {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

pub fn parse_wav_buffer(buf: &[u8]) -> Result<WavData, String> {
    if buf.len() < 44 {
        return Err("File too small for WAV".into());
    }

    let riff = &buf[0..4];
    let wave = &buf[8..12];
    if riff != b"RIFF" || wave != b"WAVE" {
        return Err("Not a WAV file".into());
    }

    let mut offset = 12;
    let mut fmt_found = false;
    let mut sample_rate = 0;
    let mut bits_per_sample = 0;
    let mut num_channels = 1;
    let mut audio_format = 0;
    let mut data_offset = 0;
    let mut data_size = 0;

    while offset < buf.len() - 8 {
        let chunk_id = &buf[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]) as usize;
        offset += 8;

        if chunk_id == b"fmt " {
            audio_format = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as u32;
            num_channels = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
            sample_rate = u32::from_le_bytes([
                buf[offset + 4],
                buf[offset + 5],
                buf[offset + 6],
                buf[offset + 7],
            ]);
            bits_per_sample = u16::from_le_bytes([buf[offset + 14], buf[offset + 15]]) as usize;
            fmt_found = true;
        } else if chunk_id == b"data" {
            data_offset = offset;
            data_size = chunk_size;
            break;
        }
        offset += chunk_size;
    }

    if !fmt_found {
        return Err("No fmt chunk found".into());
    }
    if audio_format != 1 {
        return Err(format!("Unsupported audio format: {} (only PCM=1)", audio_format));
    }
    if data_offset == 0 {
        return Err("No data chunk found".into());
    }

    let bytes_per_sample = bits_per_sample / 8;
    let total_samples = data_size / (bytes_per_sample * num_channels);
    let mut samples = Vec::with_capacity(total_samples);

    for i in 0..total_samples {
        let pos = data_offset + i * num_channels * bytes_per_sample;
        let val: f32 = match bits_per_sample {
            16 => {
                let v = i16::from_le_bytes([buf[pos], buf[pos + 1]]);
                v as f32 / 32768.0
            }
            32 => {
                let v = i32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
                v as f32 / 2147483648.0
            }
            8 => (buf[pos] as i16 - 128) as f32 / 128.0,
            _ => return Err(format!("Unsupported bits per sample: {}", bits_per_sample)),
        };
        samples.push(val);
    }

    Ok(WavData {
        sample_rate,
        samples,
    })
}

pub fn write_mono16_wav_file<W: Write>(
    writer: &mut W,
    samples: &[f32],
    sample_rate: u32,
) -> std::io::Result<()> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = (samples.len() as u32) * block_align as u32;

    // RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_size).to_le_bytes())?;
    writer.write_all(b"WAVE")?;

    // fmt chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&num_channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&(sample_rate * block_align as u32).to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&bits_per_sample.to_le_bytes())?;

    // data chunk
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;

    for &sample in samples {
        let clamped = sample.max(-1.0).min(1.0);
        let val: i16 = if clamped < 0.0 {
            (clamped * 32768.0).round() as i16
        } else {
            (clamped * 32767.0).round() as i16
        };
        writer.write_all(&val.to_le_bytes())?;
    }

    Ok(())
}
