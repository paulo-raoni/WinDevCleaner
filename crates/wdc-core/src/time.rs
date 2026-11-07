//! Small time helpers, so the engine needs no date library.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Converts days since 1970-01-01 to a civil (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// UTC timestamp for file names: `20261107-201500-123`.
pub fn file_stamp(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}{mo:02}{d:02}-{:02}{:02}{:02}-{:03}",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        unix_ms % 1000
    )
}

/// UTC timestamp in RFC 3339 form: `2026-11-07T20:15:00Z`.
pub fn rfc3339(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Local wall-clock time of day `HH:MM:SS`, for the app's activity panel.
pub fn local_clock() -> String {
    #[cfg(windows)]
    {
        let t = crate::win::local_time();
        format!("{:02}:{:02}:{:02}", t.0, t.1, t.2)
    }
    #[cfg(not(windows))]
    {
        let secs = now_unix_ms() / 1000;
        let sod = secs % 86_400;
        format!("{:02}:{:02}:{:02}", sod / 3600, (sod % 3600) / 60, sod % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        // 2026-11-07T20:15:00.123Z
        let ms = 1_794_082_500_123;
        assert_eq!(rfc3339(ms), "2026-11-07T20:15:00Z");
        assert_eq!(file_stamp(ms), "20261107-201500-123");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }
}
