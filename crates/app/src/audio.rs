//! The audio toolbox's glue between the Settings screen, the saved settings and the player: the
//! saved output device and equalizer reach the player when the app starts and whenever they are
//! saved again, and the arithmetic the equalizer's sliders and chips share.
//!
//! The screen edits the player directly, so a slider moves the sound at once and a drag is not
//! a write per pixel; the settings are written when a change is final, and the player follows
//! them from there (finding it already so).

use std::cell::RefCell;
use std::rc::Rc;

use apollo_domain::audio::{clamp_gain, Equalizer, Gains, BANDS, MAX_GAIN_DB};
use gpui::{App, Entity};

use crate::player::PlayerModel;
use crate::settings::{SettingsModel, AUDIO_DEVICE, EQUALIZER_ENABLED, EQUALIZER_GAINS};

/// A slider's position (0 at the bottom, 1 at the top) as the gain it stands for: whole dB.
pub fn gain_at(fraction: f32) -> f32 {
    clamp_gain((fraction.clamp(0.0, 1.0) * 2.0 - 1.0) * MAX_GAIN_DB)
}

/// A gain's slider position.
pub fn fraction_of(gain_db: f32) -> f32 {
    ((clamp_gain(gain_db) + MAX_GAIN_DB) / (2.0 * MAX_GAIN_DB)).clamp(0.0, 1.0)
}

/// A gain as the slider's caption shows it: "+6", "0", "\u{2212}3".
pub fn gain_text(gain_db: f32) -> String {
    let db = clamp_gain(gain_db) as i32;
    match db {
        0 => "0".to_owned(),
        d if d > 0 => format!("+{d}"),
        d => format!("\u{2212}{}", -d),
    }
}

/// The equalizer once the curve is edited on the Settings screen. The switch is left alone
/// unless the new curve is not flat while it is off: then it comes on, so the change is heard
/// (a slider that does nothing audible reads as broken). Flattening never turns it on.
pub fn after_edit(current: Equalizer, gains: Gains) -> Equalizer {
    let mut eq = Equalizer {
        enabled: current.enabled,
        gains_db: gains,
    }
    .sanitized();
    if !eq.enabled && eq.gains_db.iter().any(|g| *g != 0.0) {
        eq.enabled = true;
    }
    eq
}

/// `gains` with band `band` set to `db`.
pub fn with_band(mut gains: Gains, band: usize, db: f32) -> Gains {
    if band < BANDS {
        gains[band] = clamp_gain(db);
    }
    gains
}

/// Writes the equalizer to the settings (the switch and the curve).
pub fn save_equalizer(settings: &Entity<SettingsModel>, eq: Equalizer, cx: &mut App) {
    settings.update(cx, |s, cx| {
        s.set_text(
            &EQUALIZER_GAINS,
            &Equalizer::gains_to_text(&eq.gains_db),
            cx,
        );
        s.set(&EQUALIZER_ENABLED, eq.enabled, cx);
    });
}

/// Writes the output device to the settings (`None` is the system default).
pub fn save_audio_device(settings: &Entity<SettingsModel>, device: Option<&str>, cx: &mut App) {
    settings.update(cx, |s, cx| {
        s.set_text(&AUDIO_DEVICE, device.unwrap_or(""), cx)
    });
}

/// Gives the player the saved output device and equalizer now, and again whenever they change in
/// the settings. Only a change of the saved values is followed, so a drag in progress on the
/// player is not undone by some other setting being saved.
pub fn bind(player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &mut App) {
    let saved = |cx: &App| {
        let s = settings.read(cx);
        (s.equalizer(), s.audio_device())
    };
    let (eq, device) = saved(cx);
    player.update(cx, |p, cx| {
        p.set_equalizer(eq, cx);
        p.set_audio_device(device.clone(), cx);
    });
    let last = Rc::new(RefCell::new((eq, device)));
    let player = player.clone();
    cx.observe(settings, move |settings, cx| {
        let now = {
            let s = settings.read(cx);
            (s.equalizer(), s.audio_device())
        };
        if *last.borrow() == now {
            return;
        }
        *last.borrow_mut() = now.clone();
        player.update(cx, |p, cx| {
            p.set_equalizer(now.0, cx);
            p.set_audio_device(now.1, cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::audio::Preset;

    #[test]
    fn the_slider_runs_from_minus_twelve_at_the_bottom_to_plus_twelve_at_the_top() {
        assert_eq!(gain_at(0.0), -12.0);
        assert_eq!(gain_at(0.5), 0.0);
        assert_eq!(gain_at(1.0), 12.0);
        assert_eq!(gain_at(2.0), 12.0, "out of range is held at the end");
        assert_eq!(fraction_of(-12.0), 0.0);
        assert_eq!(fraction_of(0.0), 0.5);
        assert_eq!(fraction_of(12.0), 1.0);
    }

    #[test]
    fn a_gain_survives_a_trip_through_the_slider_in_whole_decibels() {
        for db in -12..=12 {
            assert_eq!(gain_at(fraction_of(db as f32)), db as f32, "{db}");
        }
        // Between two steps the pointer lands on the nearest whole dB.
        assert_eq!(gain_at(0.5 + 0.4 / 24.0), 0.0);
        assert_eq!(gain_at(0.5 + 0.6 / 24.0), 1.0);
    }

    #[test]
    fn captions_use_a_plus_a_proper_minus_and_a_bare_zero() {
        assert_eq!(gain_text(6.0), "+6");
        assert_eq!(gain_text(-3.0), "\u{2212}3");
        assert_eq!(gain_text(0.0), "0");
        assert_eq!(gain_text(40.0), "+12");
    }

    #[test]
    fn editing_the_curve_while_off_turns_the_switch_on_but_flattening_does_not() {
        let off = Equalizer::flat();
        let edited = after_edit(off, Preset::BassBoost.gains());
        assert!(edited.enabled);
        assert_eq!(edited.gains_db, Preset::BassBoost.gains());

        let flattened = after_edit(off, Preset::Flat.gains());
        assert!(!flattened.enabled);

        let on = Equalizer {
            enabled: true,
            gains_db: Preset::Vocal.gains(),
        };
        assert!(after_edit(on, Preset::Flat.gains()).enabled, "stays on");
    }

    #[test]
    fn one_band_changes_and_the_rest_stay() {
        let gains = with_band(Preset::Vocal.gains(), 0, 9.4);
        assert_eq!(gains[0], 9.0);
        assert_eq!(&gains[1..], &Preset::Vocal.gains()[1..]);
        assert_eq!(
            with_band(gains, 99, 5.0),
            gains,
            "a band that is not there changes nothing"
        );
    }
}
