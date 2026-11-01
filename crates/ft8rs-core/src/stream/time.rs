#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotTimestamp {
    date: Option<Date>,
    seconds_of_day: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Date {
    days: i64,
    full_year: bool,
}

impl SlotTimestamp {
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if let Some((prefix, time)) = trimmed.rsplit_once('_') {
            let date = parse_date_prefix(prefix)?;
            return parse_hhmmss(time).map(|seconds_of_day| Self {
                date: Some(date),
                seconds_of_day,
            });
        }

        parse_hhmmss(trimmed).map(|seconds_of_day| Self {
            date: None,
            seconds_of_day,
        })
    }

    pub fn add_seconds(&self, seconds: i64) -> Self {
        let total = self.seconds_of_day as i128 + seconds as i128;
        Self {
            date: self.date.as_ref().map(|date| Date {
                days: date.days + total.div_euclid(86_400) as i64,
                full_year: date.full_year,
            }),
            seconds_of_day: total.rem_euclid(86_400) as u32,
        }
    }

    pub fn format(&self) -> String {
        let time = self.format_time();
        match &self.date {
            Some(date) => {
                let (year, month, day) = civil_from_days(date.days);
                if date.full_year {
                    format!("{year:04}{month:02}{day:02}_{time}")
                } else {
                    format!("{:02}{month:02}{day:02}_{time}", year.rem_euclid(100))
                }
            }
            None => time,
        }
    }

    pub fn format_time(&self) -> String {
        let h = self.seconds_of_day / 3600;
        let m = (self.seconds_of_day / 60) % 60;
        let s = self.seconds_of_day % 60;
        format!("{h:02}{m:02}{s:02}")
    }

    pub fn milliseconds_since_midnight(&self) -> u32 {
        self.seconds_of_day * 1000
    }

    pub fn nutc(&self) -> u32 {
        let h = self.seconds_of_day / 3600;
        let m = (self.seconds_of_day / 60) % 60;
        let s = self.seconds_of_day % 60;
        h * 10000 + m * 100 + s
    }

    pub fn from_unix_seconds_utc(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let seconds_of_day = seconds.rem_euclid(86_400) as u32;
        Self {
            date: Some(Date {
                days,
                full_year: false,
            }),
            seconds_of_day,
        }
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
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
    (year, month as u32, day as u32)
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

// Gregorian civil-date arithmetic, inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + i64::from(day) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn parse_date_prefix(prefix: &str) -> Result<Date, String> {
    let invalid = || format!("invalid date '{prefix}', expected a valid YYMMDD or YYYYMMDD date");
    if !matches!(prefix.len(), 6 | 8) || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    // YY dates use 2000..2099 on input; arithmetic retains the actual century.
    let year_len = prefix.len() - 4;
    let year = prefix[..year_len].parse::<i64>().unwrap() + if year_len == 2 { 2000 } else { 0 };
    let month = prefix[year_len..year_len + 2].parse::<u32>().unwrap();
    let day = prefix[year_len + 2..].parse::<u32>().unwrap();
    if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(invalid());
    }
    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year, month, day) {
        return Err(invalid());
    }
    Ok(Date {
        days,
        full_year: year_len == 4,
    })
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
    fn dated_arithmetic_handles_calendar_boundaries_both_ways() {
        for (before, after) in [
            ("261031_235945", "261101_000000"),
            ("261231_235945", "270101_000000"),
            ("240228_235945", "240229_000000"),
            ("20240229_235945", "20240301_000000"),
            ("21000228_235945", "21000301_000000"),
            ("991231_235945", "000101_000000"),
        ] {
            let original = SlotTimestamp::parse(before).unwrap();
            let next = original.add_seconds(15);
            assert_eq!(next.format(), after);
            assert_eq!(next.add_seconds(-15), original);
        }
        assert_eq!(
            SlotTimestamp::parse("20261101_000000")
                .unwrap()
                .add_seconds(-15)
                .format(),
            "20261031_235945"
        );
    }

    #[test]
    fn rejects_invalid_calendar_dates() {
        for value in [
            "261332_120000",
            "260229_120000",
            "20260431_120000",
            "00000101_000000",
            "21000229_120000",
        ] {
            assert!(SlotTimestamp::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn parses_timestamp() {
        let ts = SlotTimestamp::parse("230208_140300").unwrap();
        assert_eq!(ts.format(), "230208_140300");
        assert_eq!(ts.format_time(), "140300");
        assert_eq!(ts.nutc(), 140300);
        assert_eq!(ts.add_seconds(15).format(), "230208_140315");
    }

    #[test]
    fn parses_time_only() {
        let ts = SlotTimestamp::parse("235950").unwrap();
        assert_eq!(ts.add_seconds(15).format(), "000005");
    }

    #[test]
    fn formats_unix_seconds_as_utc_timestamp() {
        let ts = SlotTimestamp::from_unix_seconds_utc(1_675_864_980);
        assert_eq!(ts.format(), "230208_140300");
    }
}
