use std::time::Duration;

use apollo_services_common::LoudnessTable;

/// Tunables for the engine. Defaults are the production policy.
#[derive(Clone, Debug)]
pub struct MediaConfig {
    /// `None` = mpv's platform default (WASAPI). Tests use `Some("null")` to stay silent.
    pub audio_output: Option<String>,
    /// mpv software volume, 0..=130. 100 leaves the OS mixer in charge.
    pub volume: u8,
    /// Hard cap for the demuxer read-ahead buffer. Memory only; never on disk (MEDIA-01).
    pub max_buffer_bytes: u64,
    pub max_back_buffer_bytes: u64,
    pub network_timeout: Duration,
    /// Position updates are coalesced to at most one per this interval (about 4 Hz).
    pub position_interval: Duration,
    /// A lease this close to its reported expiry counts as stale (RES-04).
    pub lease_margin: Duration,
    /// How long a lease with no reported expiry may sit before it is treated as stale.
    pub max_unknown_lease_age: Duration,
    /// Where the engine reads a track's loudness for volume normalization (filled by the
    /// loudness fetcher). Empty by default: every track plays at unity.
    pub loudness: LoudnessTable,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            audio_output: None,
            volume: 100,
            max_buffer_bytes: 24 * 1024 * 1024,
            max_back_buffer_bytes: 8 * 1024 * 1024,
            network_timeout: Duration::from_secs(20),
            position_interval: Duration::from_millis(250),
            lease_margin: Duration::from_secs(60),
            max_unknown_lease_age: Duration::from_secs(300),
            loudness: LoudnessTable::default(),
        }
    }
}

/// Options applied before the core starts (they cannot be changed afterwards for
/// `config`/`load-scripts`). Everything that could read user config, run scripts, spawn an
/// extractor, show video, grab media keys, cache to disk, or touch other files is off.
pub fn core_options(cfg: &MediaConfig) -> Vec<(&'static str, String)> {
    let mut o: Vec<(&'static str, String)> = vec![
        // Isolation from the user's mpv installation.
        ("config", "no".into()),
        ("load-scripts", "no".into()),
        ("ytdl", "no".into()),
        ("osc", "no".into()),
        ("input-default-bindings", "no".into()),
        ("input-vo-keyboard", "no".into()),
        ("input-media-keys", "no".into()), // SMTC belongs to the app, not libmpv
        ("terminal", "no".into()),
        ("load-unsafe-playlists", "no".into()),
        ("access-references", "no".into()),
        ("resume-playback", "no".into()),
        ("save-position-on-quit", "no".into()),
        // Audio only.
        ("vo", "null".into()),
        ("vid", "no".into()),
        ("audio-display", "no".into()),
        ("idle", "yes".into()),
        ("keep-open", "no".into()),
        ("audio-client-name", "Apollo".into()),
        ("volume", cfg.volume.to_string()),
        // Memory-only, bounded buffering.
        ("cache", "yes".into()),
        ("cache-on-disk", "no".into()),
        ("demuxer-max-bytes", cfg.max_buffer_bytes.to_string()),
        (
            "demuxer-max-back-bytes",
            cfg.max_back_buffer_bytes.to_string(),
        ),
        // Network hygiene.
        ("network-timeout", cfg.network_timeout.as_secs().to_string()),
        ("tls-verify", "yes".into()),
    ];
    if let Some(ao) = &cfg.audio_output {
        o.push(("ao", ao.clone()));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(o: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
        o.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn hardened_defaults() {
        let o = core_options(&MediaConfig::default());
        for (k, v) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("vid", "no"),
            ("vo", "null"),
            ("cache-on-disk", "no"),
            ("input-media-keys", "no"),
            ("load-unsafe-playlists", "no"),
            ("tls-verify", "yes"),
        ] {
            assert_eq!(get(&o, k), Some(v), "{k}");
        }
        assert_eq!(get(&o, "ao"), None, "default output device is the system one");
        let bytes: u64 = get(&o, "demuxer-max-bytes").unwrap().parse().unwrap();
        assert!(bytes <= 64 * 1024 * 1024, "buffer must stay bounded");
    }

    #[test]
    fn explicit_output_is_passed_through() {
        let cfg = MediaConfig {
            audio_output: Some("null".into()),
            ..Default::default()
        };
        assert_eq!(get(&core_options(&cfg), "ao"), Some("null"));
    }
}
