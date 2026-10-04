//! The one thing Apollo reads from a `player` response: how loud the track is.
//!
//! Streams come from yt-dlp, so everything else in the response (stream URLs and ciphers,
//! `videoDetails`) is ignored here and never leaves this module. `playbackTracking`, the URLs a
//! play is reported to, is read by [`crate::tracking`], not here.

use serde_json::Value;

/// Loudness as YouTube measured it for one video.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerLoudness {
    /// `playerConfig.audioConfig.loudnessDb`: dB relative to YouTube's reference level, so `0.0`
    /// is a track at the reference, `+4.0` is 4 dB louder and `-3.0` is 3 dB quieter. This is the
    /// number YouTube's own "Stable volume" and "content loudness" use.
    pub loudness_db: Option<f32>,
    /// `playerConfig.audioConfig.perceptualLoudnessDb`: the same measurement on a different
    /// scale. Kept for diagnostics; nothing decides from it.
    pub perceptual_loudness_db: Option<f32>,
}

/// Reads the loudness out of a raw `player` response. A response that carries no audio config
/// (unplayable, age-gated, a client that is not served one) is not an error: it parses to a
/// value with both fields `None`.
pub fn parse_player_loudness(value: &Value) -> PlayerLoudness {
    let config = value.pointer("/playerConfig/audioConfig");
    let number = |key: &str| {
        config
            .and_then(|c| c.get(key))
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
    };
    // The audio formats repeat the measurement (`loudnessDb` beside `audioQuality`); read it from
    // the first one that has it when the audio config does not. Only the number is read: the
    // format's URL and cipher are never touched.
    let from_formats = || {
        value
            .pointer("/streamingData/adaptiveFormats")
            .and_then(Value::as_array)?
            .iter()
            .filter(|f| f.get("audioQuality").is_some())
            .find_map(|f| f.get("loudnessDb").and_then(Value::as_f64))
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
    };
    PlayerLoudness {
        loudness_db: number("loudnessDb").or_else(from_formats),
        perceptual_loudness_db: number("perceptualLoudnessDb"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_both_numbers_from_the_audio_config() {
        let v = json!({
            "playabilityStatus": {"status": "OK"},
            "playerConfig": {"audioConfig": {
                "loudnessDb": 4.120000839233398,
                "perceptualLoudnessDb": -9.5,
                "enablePerFormatLoudness": true
            }},
            "streamingData": {"adaptiveFormats": []}
        });
        let l = parse_player_loudness(&v);
        assert!((l.loudness_db.unwrap() - 4.12).abs() < 1e-4);
        assert_eq!(l.perceptual_loudness_db, Some(-9.5));
    }

    #[test]
    fn the_audio_formats_are_the_fallback_when_the_config_has_no_number() {
        let v = json!({
            "playerConfig": {"audioConfig": {}},
            "streamingData": {"adaptiveFormats": [
                {"itag": 137, "mimeType": "video/mp4", "loudnessDb": 99.0},
                {"itag": 251, "audioQuality": "AUDIO_QUALITY_MEDIUM", "loudnessDb": 2.5},
                {"itag": 140, "audioQuality": "AUDIO_QUALITY_MEDIUM", "loudnessDb": 2.6}
            ]}
        });
        assert_eq!(
            parse_player_loudness(&v).loudness_db,
            Some(2.5),
            "first audio format, not video"
        );
        let both = json!({
            "playerConfig": {"audioConfig": {"loudnessDb": 1.0}},
            "streamingData": {"adaptiveFormats": [{"audioQuality": "AUDIO_QUALITY_LOW", "loudnessDb": 9.0}]}
        });
        assert_eq!(parse_player_loudness(&both).loudness_db, Some(1.0), "the config wins");
    }

    #[test]
    fn a_negative_number_is_a_quiet_track() {
        let v = json!({"playerConfig": {"audioConfig": {"loudnessDb": -6.5}}});
        assert_eq!(parse_player_loudness(&v).loudness_db, Some(-6.5));
    }

    #[test]
    fn no_audio_config_is_unknown_not_an_error() {
        for v in [
            json!({}),
            json!({"playabilityStatus": {"status": "UNPLAYABLE"}}),
            json!({"playerConfig": {}}),
            json!({"playerConfig": {"audioConfig": {}}}),
        ] {
            assert_eq!(parse_player_loudness(&v), PlayerLoudness::default(), "{v}");
        }
    }

    #[test]
    fn values_that_are_not_numbers_are_unknown() {
        let v = json!({"playerConfig": {"audioConfig": {"loudnessDb": "loud", "perceptualLoudnessDb": null}}});
        assert_eq!(parse_player_loudness(&v), PlayerLoudness::default());
    }
}
