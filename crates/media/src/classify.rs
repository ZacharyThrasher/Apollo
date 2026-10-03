//! Maps mpv/ffmpeg diagnostic text to the coordinator's coarse failure classes (NET-02).
//! Pure string logic so it is testable without mpv. Input text may contain URLs; this
//! module never stores or returns it.

use apollo_domain::playback::Failure;

/// Severity order used when several diagnostics were seen for one failed load.
fn rank(f: Failure) -> u8 {
    match f {
        Failure::Device => 6,
        Failure::LeaseExpired => 5,
        Failure::Throttled => 4,
        Failure::Unsupported => 3,
        Failure::Network => 2,
        Failure::Media => 1,
        _ => 0,
    }
}

pub fn classify_log(text: &str) -> Option<Failure> {
    let t = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));

    if has(&["http error 429", "too many requests"]) {
        return Some(Failure::Throttled);
    }
    if has(&[
        "http error 401",
        "http error 403",
        "http error 404",
        "http error 410",
        "forbidden",
        "unauthorized",
    ]) {
        // The signed URL was refused or has expired: one re-resolution is allowed.
        return Some(Failure::LeaseExpired);
    }
    if has(&[
        "http error 5",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
    ]) {
        return Some(Failure::Network);
    }
    let audio_ctx = has(&["[ao", "wasapi", "audio device", "ao/"]);
    if audio_ctx
        && has(&[
            "lost",
            "removed",
            "invalidated",
            "not found",
            "could not open",
            "could not initialize",
            "failed",
            "unplugged",
            "no longer",
        ])
    {
        return Some(Failure::Device);
    }
    if has(&[
        "partial file", // body cut off by the server
        "could not connect",
        "couldn't connect",
        "failure when receiving data",
        "failure when sending data",
        "connection refused",
        "connection reset",
        "timed out",
        "timeout",
        "could not resolve",
        "name or service not known",
        "network is unreachable",
        "end of file", // truncated body from the server
        "tls",
        "ssl",
        "certificate",
    ]) {
        return Some(Failure::Network);
    }
    if has(&[
        "failed to recognize file format",
        "unrecognized file format",
        "invalid data found when processing input",
        "unsupported",
        "no audio",
        "codec not found",
    ]) {
        return Some(Failure::Unsupported);
    }
    None
}

/// The class for a load that ended with an error, given the diagnostics seen while it ran.
/// `Media` when nothing more specific is known (never "YouTube blocked you").
pub fn classify_failed_load<'a>(logs: impl IntoIterator<Item = &'a str>) -> Failure {
    logs.into_iter()
        .filter_map(classify_log)
        .max_by_key(|f| rank(*f))
        .unwrap_or(Failure::Media)
}

/// mpv error codes that matter (client.h): AO_INIT_FAILED, NOTHING_TO_PLAY, UNKNOWN_FORMAT.
const ERR_AO_INIT_FAILED: i32 = -14;
const ERR_NOTHING_TO_PLAY: i32 = -16;
const ERR_UNKNOWN_FORMAT: i32 = -17;

/// Failure class for an END_FILE event with an error: the code first (output device), then the
/// diagnostics seen while the file ran, then what the code alone tells us.
pub fn classify_end_file_error<'a>(code: i32, logs: impl IntoIterator<Item = &'a str>) -> Failure {
    if code == ERR_AO_INIT_FAILED {
        return Failure::Device;
    }
    match classify_failed_load(logs) {
        Failure::Media => match code {
            ERR_UNKNOWN_FORMAT | ERR_NOTHING_TO_PLAY => Failure::Unsupported,
            _ => Failure::Media,
        },
        specific => specific,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses() {
        assert_eq!(
            classify_log("[ffmpeg] http: HTTP error 403 Forbidden"),
            Some(Failure::LeaseExpired)
        );
        assert_eq!(classify_log("HTTP error 410 Gone"), Some(Failure::LeaseExpired));
        assert_eq!(
            classify_log("HTTP error 429 Too Many Requests"),
            Some(Failure::Throttled)
        );
        assert_eq!(
            classify_log("HTTP error 503 Service Unavailable"),
            Some(Failure::Network)
        );
    }

    #[test]
    fn device_and_network_and_format() {
        assert_eq!(classify_log("[ao/wasapi] Device lost"), Some(Failure::Device));
        assert_eq!(
            classify_log("Could not open/initialize audio device -> no sound."),
            Some(Failure::Device)
        );
        assert_eq!(classify_log("tcp: Connection refused"), Some(Failure::Network));
        assert_eq!(classify_log("error: Could not connect to server"), Some(Failure::Network));
        assert_eq!(
            classify_log("transfer failed: Transferred a partial file"),
            Some(Failure::Network)
        );
        assert_eq!(
            classify_log("Failed to recognize file format."),
            Some(Failure::Unsupported)
        );
        assert_eq!(classify_log("everything is fine"), None);
    }

    #[test]
    fn end_file_codes() {
        assert_eq!(classify_end_file_error(-14, ["whatever"]), Failure::Device);
        assert_eq!(classify_end_file_error(-17, ["noise"]), Failure::Unsupported);
        assert_eq!(classify_end_file_error(-13, ["HTTP error 403 Forbidden"]), Failure::LeaseExpired);
        assert_eq!(classify_end_file_error(-13, ["noise"]), Failure::Media);
    }

    #[test]
    fn most_specific_wins_and_unknown_is_media() {
        assert_eq!(
            classify_failed_load(["tcp: timed out", "HTTP error 403 Forbidden", "noise"]),
            Failure::LeaseExpired
        );
        assert_eq!(classify_failed_load(["noise"]), Failure::Media);
        assert_eq!(classify_failed_load(std::iter::empty()), Failure::Media);
    }
}
