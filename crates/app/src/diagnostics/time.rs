//! A Unix time as a UTC date and time, for file names and report headers. No time-zone data:
//! the report and the export both say UTC.

/// `2026-10-04T08:12:33Z`.
pub fn iso_utc(unix: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil(unix);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// `2026-10-04`, for a suggested file name.
pub fn date_utc(unix: u64) -> String {
    let (y, m, d, ..) = civil(unix);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Year, month, day, hour, minute, second (UTC) of a Unix time. Days since the epoch become a
/// calendar date with the standard civil-from-days conversion (proleptic Gregorian).
fn civil(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (secs / 3_600) as u32,
        (secs % 3_600 / 60) as u32,
        (secs % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_759_565_553), "2025-10-04T08:12:33Z");
        assert_eq!(iso_utc(4_102_444_799), "2099-12-31T23:59:59Z");
        assert_eq!(date_utc(1_759_565_553), "2025-10-04");
    }
}
