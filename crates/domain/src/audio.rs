//! The audio toolbox's policy: the equalizer, its presets and the clip guard, which output device
//! to use when the chosen one is gone, and the playback speeds on offer. Pure data and
//! arithmetic; the media layer turns the result into mpv properties and filters.

/// Bands of the graphic equalizer, one octave apart.
pub const BANDS: usize = 10;

/// Centre frequency of each band in Hz, lowest first.
pub const BAND_HZ: [u32; BANDS] = [31, 62, 125, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000];

/// The most a band is moved either way, in dB.
pub const MAX_GAIN_DB: f32 = 12.0;

/// A band's label: "31", "62", ..., "1k", ..., "16k".
pub fn band_label(hz: u32) -> String {
    if hz >= 1_000 {
        format!("{}k", hz / 1_000)
    } else {
        hz.to_string()
    }
}

/// Gains in dB, one per band.
pub type Gains = [f32; BANDS];

/// A gain made safe to use: finite, within [`MAX_GAIN_DB`], and rounded to a whole dB (the
/// sliders move in whole steps; this also keeps a preset and "the same gains" comparable).
pub fn clamp_gain(db: f32) -> f32 {
    if db.is_finite() {
        db.round().clamp(-MAX_GAIN_DB, MAX_GAIN_DB)
    } else {
        0.0
    }
}

/// The equalizer as the owner set it: the switch and the ten gains. The gains are kept while the
/// switch is off, so turning it back on restores the curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Equalizer {
    pub enabled: bool,
    pub gains_db: Gains,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self::flat()
    }
}

impl Equalizer {
    /// Off, every band at 0 dB.
    pub const fn flat() -> Self {
        Self {
            enabled: false,
            gains_db: [0.0; BANDS],
        }
    }

    /// Gains cleaned up with [`clamp_gain`].
    pub fn sanitized(mut self) -> Self {
        for g in &mut self.gains_db {
            *g = clamp_gain(*g);
        }
        self
    }

    /// The gains the filter should have right now: the curve when on, flat when off.
    pub fn effective_gains(&self) -> Gains {
        if self.enabled {
            self.sanitized().gains_db
        } else {
            [0.0; BANDS]
        }
    }

    /// Whether the filter does nothing (off, or every band at 0 dB).
    pub fn is_flat(&self) -> bool {
        self.effective_gains().iter().all(|g| *g == 0.0)
    }

    /// The clip guard: how much the whole signal is turned down so the boosts cannot push it
    /// past full scale, in dB (zero or negative). The largest boost, taken whole: a band that
    /// raises its neighbourhood by 6 dB takes 6 dB off everything. A little conservative (music
    /// rarely puts full-scale energy in one band) and so never clips, and costs no limiter
    /// latency. Cuts need no guard.
    pub fn preamp_db(&self) -> f32 {
        let boost = self
            .effective_gains()
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        -boost
    }

    /// The preset whose curve these gains are, when they are exactly one.
    pub fn preset(&self) -> Option<Preset> {
        let gains = self.sanitized().gains_db;
        Preset::ALL.into_iter().find(|p| p.gains() == gains)
    }

    /// `gains` as text for the settings table: ten comma-separated whole numbers.
    pub fn gains_to_text(gains: &Gains) -> String {
        gains
            .iter()
            .map(|g| format!("{}", clamp_gain(*g) as i32))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The inverse of [`Equalizer::gains_to_text`]. Anything but ten numbers is unreadable
    /// (`None`), so a damaged value falls back to flat instead of half a curve.
    pub fn gains_from_text(text: &str) -> Option<Gains> {
        let mut out = [0.0; BANDS];
        let mut parts = text.split(',');
        for slot in &mut out {
            *slot = clamp_gain(parts.next()?.trim().parse::<f32>().ok()?);
        }
        parts.next().is_none().then_some(out)
    }
}

/// The curves on offer as chips. Gains are lowest band first (31 Hz to 16 kHz).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Flat,
    BassBoost,
    Vocal,
    TrebleBoost,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Flat,
        Preset::BassBoost,
        Preset::Vocal,
        Preset::TrebleBoost,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Flat => "Flat",
            Preset::BassBoost => "Bass boost",
            Preset::Vocal => "Vocal",
            Preset::TrebleBoost => "Treble boost",
        }
    }

    pub fn gains(self) -> Gains {
        match self {
            Preset::Flat => [0.0; BANDS],
            Preset::BassBoost => [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            Preset::Vocal => [-2.0, -3.0, -3.0, 1.0, 4.0, 4.0, 3.0, 1.0, -1.0, -2.0],
            Preset::TrebleBoost => [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0],
        }
    }
}

// ---- output device ---------------------------------------------------------------------------

/// An audio output the system offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// What the player is told to use (mpv's device name, e.g. `wasapi/{guid}`). Stable across
    /// plugging and unplugging, so it is what is saved.
    pub id: String,
    /// What the owner reads ("Speakers (Realtek Audio)").
    pub name: String,
}

/// The device name that means "whatever the system default is".
pub const SYSTEM_DEFAULT: &str = "auto";

/// The device to play on: `chosen` while the system still lists it, else the system default
/// (`None`). Called again whenever the list changes, so a device that comes back is switched to
/// again by itself. `None` as `chosen` is the owner picking "System default".
pub fn resolve_device<'a>(chosen: Option<&'a str>, available: &[AudioDevice]) -> Option<&'a str> {
    chosen.filter(|id| available.iter().any(|d| d.id == *id))
}

/// What the Output device row shows: the chosen device's name, or "System default" when none is
/// chosen. A chosen device that is not plugged in reads as "System default" too: that is what
/// is playing.
pub fn device_label(chosen: Option<&str>, available: &[AudioDevice]) -> String {
    resolve_device(chosen, available)
        .and_then(|id| available.iter().find(|d| d.id == id))
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "System default".to_owned())
}

// ---- speed -----------------------------------------------------------------------------------

/// The playback speeds on offer, slowest first. Normal speed is one of them.
pub const SPEEDS: [f32; 4] = [0.75, 1.0, 1.25, 1.5];

/// Normal speed.
pub const NORMAL_SPEED: f32 = 1.0;

/// A speed the engine may be given: one of [`SPEEDS`] (anything else is normal speed).
pub fn sanitize_speed(speed: f32) -> f32 {
    SPEEDS
        .into_iter()
        .find(|s| (s - speed).abs() < 1e-3)
        .unwrap_or(NORMAL_SPEED)
}

/// A speed as the button shows it: "0.75×", "1.0×", "1.25×", "1.5×".
pub fn speed_label(speed: f32) -> String {
    let text = format!("{:.2}", speed);
    let text = if text.ends_with('0') {
        text[..text.len() - 1].to_owned()
    } else {
        text
    };
    format!("{text}\u{00d7}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn a_new_equalizer_is_off_and_flat() {
        let eq = Equalizer::default();
        assert!(!eq.enabled);
        assert!(eq.is_flat());
        assert_eq!(eq.preamp_db(), 0.0);
    }

    #[test]
    fn the_bands_run_from_31_hz_to_16_khz_in_octaves() {
        assert_eq!(BAND_HZ.len(), BANDS);
        assert_eq!(BAND_HZ[0], 31);
        assert_eq!(BAND_HZ[BANDS - 1], 16_000);
        for pair in BAND_HZ.windows(2) {
            let ratio = pair[1] as f32 / pair[0] as f32;
            assert!((1.9..=2.1).contains(&ratio), "{pair:?}");
        }
        let labels: Vec<_> = BAND_HZ.iter().map(|h| band_label(*h)).collect();
        assert_eq!(
            labels,
            ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"]
        );
    }

    #[test]
    fn gains_are_whole_decibels_within_twelve() {
        assert_eq!(clamp_gain(5.4), 5.0);
        assert_eq!(clamp_gain(-5.6), -6.0);
        assert_eq!(clamp_gain(40.0), MAX_GAIN_DB);
        assert_eq!(clamp_gain(-40.0), -MAX_GAIN_DB);
        assert_eq!(clamp_gain(f32::NAN), 0.0);
        assert_eq!(clamp_gain(f32::INFINITY), 0.0);
    }

    #[test]
    fn off_means_flat_whatever_the_curve() {
        let mut eq = Equalizer {
            enabled: false,
            gains_db: Preset::BassBoost.gains(),
        };
        assert_eq!(eq.effective_gains(), [0.0; BANDS]);
        assert!(eq.is_flat());
        assert_eq!(eq.preamp_db(), 0.0);
        eq.enabled = true;
        assert_eq!(eq.effective_gains(), Preset::BassBoost.gains());
        assert!(!eq.is_flat());
    }

    #[test]
    fn the_clip_guard_takes_off_the_largest_boost_and_ignores_cuts() {
        let mut eq = Equalizer {
            enabled: true,
            gains_db: Preset::BassBoost.gains(),
        };
        assert_eq!(eq.preamp_db(), -6.0);
        eq.gains_db = [-3.0; BANDS];
        assert_eq!(eq.preamp_db(), 0.0, "cuts cannot clip");
        eq.gains_db = [12.0; BANDS];
        assert_eq!(eq.preamp_db(), -12.0);
    }

    #[test]
    fn no_preset_can_clip_past_its_preamp_and_all_stay_in_range() {
        for p in Preset::ALL {
            let eq = Equalizer {
                enabled: true,
                gains_db: p.gains(),
            };
            let max = p.gains().iter().copied().fold(f32::MIN, f32::max).max(0.0);
            assert_eq!(eq.preamp_db(), -max, "{p:?}");
            assert!(p.gains().iter().all(|g| g.abs() <= MAX_GAIN_DB), "{p:?}");
            assert_eq!(eq.preset(), Some(p), "{p:?}");
        }
        assert_eq!(Preset::Flat.gains(), [0.0; BANDS]);
    }

    #[test]
    fn a_hand_made_curve_is_not_a_preset() {
        let mut gains = Preset::Vocal.gains();
        gains[0] += 1.0;
        let eq = Equalizer {
            enabled: true,
            gains_db: gains,
        };
        assert_eq!(eq.preset(), None);
    }

    #[test]
    fn gains_round_trip_through_text() {
        for p in Preset::ALL {
            let text = Equalizer::gains_to_text(&p.gains());
            assert_eq!(Equalizer::gains_from_text(&text), Some(p.gains()), "{text}");
        }
        assert_eq!(
            Equalizer::gains_to_text(&Preset::Vocal.gains()),
            "-2,-3,-3,1,4,4,3,1,-1,-2"
        );
    }

    #[test]
    fn damaged_text_is_unreadable_not_half_a_curve() {
        assert_eq!(Equalizer::gains_from_text(""), None);
        assert_eq!(Equalizer::gains_from_text("1,2,3"), None);
        assert_eq!(Equalizer::gains_from_text("0,0,0,0,0,0,0,0,0,x"), None);
        assert_eq!(Equalizer::gains_from_text("0,0,0,0,0,0,0,0,0,0,0"), None);
        // Out-of-range numbers are brought back in range.
        let g = Equalizer::gains_from_text("99,0,0,0,0,0,0,0,0,-99").unwrap();
        assert_eq!((g[0], g[9]), (MAX_GAIN_DB, -MAX_GAIN_DB));
    }

    #[test]
    fn the_chosen_device_is_used_while_it_is_listed_and_the_default_when_it_is_not() {
        let list = [dev("wasapi/a", "Speakers"), dev("wasapi/b", "Headset")];
        assert_eq!(resolve_device(Some("wasapi/b"), &list), Some("wasapi/b"));
        assert_eq!(resolve_device(None, &list), None);
        // Unplugged: the default plays; plugged back in, the same call picks it again.
        assert_eq!(resolve_device(Some("wasapi/b"), &list[..1]), None);
        assert_eq!(resolve_device(Some("wasapi/b"), &list), Some("wasapi/b"));
        assert_eq!(resolve_device(Some("wasapi/b"), &[]), None);
    }

    #[test]
    fn the_row_names_what_is_actually_playing() {
        let list = [dev("wasapi/a", "Speakers"), dev("wasapi/b", "Headset")];
        assert_eq!(device_label(None, &list), "System default");
        assert_eq!(device_label(Some("wasapi/b"), &list), "Headset");
        assert_eq!(device_label(Some("wasapi/gone"), &list), "System default");
    }

    #[test]
    fn the_speeds_are_the_four_on_offer_and_labelled_plainly() {
        let labels: Vec<_> = SPEEDS.iter().map(|s| speed_label(*s)).collect();
        assert_eq!(
            labels,
            ["0.75\u{d7}", "1.0\u{d7}", "1.25\u{d7}", "1.5\u{d7}"]
        );
        assert!(SPEEDS.contains(&NORMAL_SPEED));
    }

    #[test]
    fn only_a_listed_speed_reaches_the_engine() {
        assert_eq!(sanitize_speed(1.25), 1.25);
        assert_eq!(sanitize_speed(0.75), 0.75);
        assert_eq!(sanitize_speed(2.0), NORMAL_SPEED);
        assert_eq!(sanitize_speed(f32::NAN), NORMAL_SPEED);
    }
}
