//! The name an instant playlist starts with: today's date and "Untitled", like "Oct 5 · Untitled",
//! with " 2", " 3" and so on when that name is taken. The date is the PC's local one (UTC where
//! there is no Windows clock, which only tests and the cloud see).

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Oct 5 · Untitled`, or the first of `… Untitled 2`, `… Untitled 3`, … that `taken` does not
/// claim. `month` is 1-12.
pub fn default_name(month: u32, day: u32, taken: impl Fn(&str) -> bool) -> String {
    let month = MONTHS[(month.clamp(1, 12) - 1) as usize];
    let base = format!("{month} {day} \u{b7} Untitled");
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .expect("an endless range finds a free name")
}

/// Today's `(month, day)` on this PC.
pub fn today() -> (u32, u32) {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime only fills the structure it returns.
        let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        (u32::from(now.wMonth), u32::from(now.wDay))
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        month_day_of(secs)
    }
}

/// `(month, day)` of a Unix time in UTC (days since 1970-01-01 to a civil date).
#[cfg_attr(windows, allow(dead_code))]
fn month_day_of(unix_secs: u64) -> (u32, u32) {
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_date_and_untitled() {
        assert_eq!(default_name(10, 5, |_| false), "Oct 5 \u{b7} Untitled");
        assert_eq!(default_name(1, 31, |_| false), "Jan 31 \u{b7} Untitled");
        assert_eq!(default_name(12, 1, |_| false), "Dec 1 \u{b7} Untitled");
    }

    #[test]
    fn a_taken_name_steps_to_the_next_number() {
        let taken = ["Oct 5 \u{b7} Untitled", "Oct 5 \u{b7} Untitled 2"];
        assert_eq!(
            default_name(10, 5, |n| taken.contains(&n)),
            "Oct 5 \u{b7} Untitled 3"
        );
        // Only the exact name counts.
        assert_eq!(
            default_name(10, 5, |n| n == "Oct 5 \u{b7} Untitled 2"),
            "Oct 5 \u{b7} Untitled"
        );
    }

    #[test]
    fn unix_time_becomes_a_calendar_day() {
        assert_eq!(month_day_of(0), (1, 1));
        // 2026-10-05 17:13:56 UTC.
        assert_eq!(month_day_of(1_791_220_436), (10, 5));
        // A leap day, and the day after it.
        assert_eq!(month_day_of(1_709_164_800), (2, 29));
        assert_eq!(month_day_of(1_709_251_200), (3, 1));
    }
}
