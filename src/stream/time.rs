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

    pub fn from_unix_seconds_utc(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let seconds_of_day = seconds.rem_euclid(86_400) as u32;
        let (year, month, day) = civil_from_days(days);
        Self {
            prefix: Some(format!("{:02}{month:02}{day:02}", year.rem_euclid(100))),
            seconds_of_day,
        }
    }
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
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
    fn formats_unix_seconds_as_utc_wsjtx_timestamp() {
        let ts = SlotTimestamp::from_unix_seconds_utc(1_675_864_980);
        assert_eq!(ts.format(), "230208_140300");
    }
}
