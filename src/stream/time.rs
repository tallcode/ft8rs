use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotTimestamp {
    prefix: Option<String>,
    seconds_of_day: u32,
}

impl SlotTimestamp {
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if let Some((prefix, time)) = trimmed.rsplit_once('_') {
            validate_date_prefix(prefix)?;
            return parse_hhmmss(time).map(|seconds_of_day| Self {
                prefix: Some(prefix.to_string()),
                seconds_of_day,
            });
        }

        parse_hhmmss(trimmed).map(|seconds_of_day| Self {
            prefix: None,
            seconds_of_day,
        })
    }

    pub fn infer_from_path(path: impl AsRef<Path>) -> Option<Self> {
        let stem = path.as_ref().file_stem()?.to_string_lossy();
        let stem = stem.as_ref();
        let suffix = stem
            .as_bytes()
            .windows(13)
            .rposition(|window| is_timestamp_bytes(window))
            .map(|idx| &stem[idx..idx + 13])?;
        Self::parse(suffix).ok()
    }

    pub fn add_seconds(&self, seconds: i64) -> Self {
        let day = 24 * 60 * 60;
        let t = (self.seconds_of_day as i64 + seconds).rem_euclid(day);
        Self {
            prefix: self.prefix.clone(),
            seconds_of_day: t as u32,
        }
    }

    pub fn format(&self) -> String {
        let h = self.seconds_of_day / 3600;
        let m = (self.seconds_of_day / 60) % 60;
        let s = self.seconds_of_day % 60;
        let time = format!("{h:02}{m:02}{s:02}");
        match &self.prefix {
            Some(prefix) => format!("{prefix}_{time}"),
            None => time,
        }
    }
}

fn parse_hhmmss(value: &str) -> Result<u32, String> {
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid time '{value}', expected HHMMSS"));
    }
    let h = value[0..2].parse::<u32>().unwrap();
    let m = value[2..4].parse::<u32>().unwrap();
    let s = value[4..6].parse::<u32>().unwrap();
    if h > 23 || m > 59 || s > 59 {
        return Err(format!("invalid time '{value}', expected HHMMSS"));
    }
    Ok(h * 3600 + m * 60 + s)
}

fn validate_date_prefix(prefix: &str) -> Result<(), String> {
    let valid_len = prefix.len() == 6 || prefix.len() == 8;
    if valid_len && prefix.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(format!(
            "invalid date prefix '{prefix}', expected YYMMDD or YYYYMMDD"
        ))
    }
}

fn is_timestamp_bytes(window: &[u8]) -> bool {
    window.len() == 13
        && window[6] == b'_'
        && window[..6].iter().all(u8::is_ascii_digit)
        && window[7..].iter().all(u8::is_ascii_digit)
}

impl std::fmt::Display for SlotTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.format())
    }
}

#[cfg(test)]
mod tests {
    use super::SlotTimestamp;

    #[test]
    fn parses_wsjtx_timestamp() {
        let ts = SlotTimestamp::parse("230208_140300").unwrap();
        assert_eq!(ts.format(), "230208_140300");
        assert_eq!(ts.add_seconds(15).format(), "230208_140315");
    }

    #[test]
    fn parses_time_only() {
        let ts = SlotTimestamp::parse("235950").unwrap();
        assert_eq!(ts.add_seconds(15).format(), "000005");
    }

    #[test]
    fn infers_from_wav_filename() {
        let ts = SlotTimestamp::infer_from_path("tests/ft8/230208_140300.wav").unwrap();
        assert_eq!(ts.format(), "230208_140300");
    }
}
