use chrono::{DateTime, Utc};
use chrono_tz::Tz;

pub fn reporting_day(timestamp: DateTime<Utc>, timezone: &str) -> String {
    let tz = timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    timestamp.with_timezone(&tz).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn invalid_timezone_falls_back_to_utc() {
        let at = Utc.with_ymd_and_hms(2026, 8, 29, 17, 0, 0).unwrap();
        assert_eq!(reporting_day(at, "invalid"), "2026-08-29");
    }
}
