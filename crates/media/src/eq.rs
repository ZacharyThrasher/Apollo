//! The equalizer's filter chain and the smoothing between curves.
//!
//! The chain is ten mpv `lavfi` filters, each holding one libavfilter `equalizer` (a peaking
//! biquad, one octave wide) under the label `eq0`..`eq9`. Installing the chain re-creates mpv's
//! audio filters, which is not click-free, so it happens once, before the first boost. After
//! that a band's gain is changed in place with `af-command <label> gain <dB> equalizer`, which
//! moves the biquad's coefficients and keeps its state, and [`EqBands`] glides there (linearly
//! in dB) instead of stepping. The target is named (`equalizer`) because the default one,
//! "all", reports an error for the graph's plumbing filters even though the command lands.
//!
//! A filter graph that mpv rebuilds (a stream of another sample rate, a device change) comes
//! back with the gains the chain was installed with, so the engine sends every band's current
//! gain again when mpv says its audio was reconfigured.

use std::time::{Duration, Instant};

use apollo_domain::audio::{Gains, BANDS, BAND_HZ};

use crate::ramp::GainRamp;

/// How long a change of curve takes, bands and the clip guard's preamp alike, so a drag on a
/// slider never leaves the preamp lagging behind a boost.
pub const EQ_GLIDE: Duration = Duration::from_millis(80);

/// The filter name `af-command` is aimed at inside each band's graph.
pub const COMMAND_TARGET: &str = "equalizer";

/// mpv's label for a band's filter.
pub fn label(band: usize) -> String {
    format!("eq{band}")
}

/// A gain as `af-command` and the filter string take it.
pub fn gain_arg(db: f32) -> String {
    format!("{db:.2}")
}

/// The value of mpv's `af` option that installs the chain with `gains`.
pub fn chain(gains: &Gains) -> String {
    (0..BANDS)
        .map(|band| {
            format!(
                "@{}:lavfi=[equalizer=f={}:t=o:w=1:g={}]",
                label(band),
                BAND_HZ[band],
                gain_arg(gains[band])
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The ten band gains in dB as they are right now, each gliding to its target.
#[derive(Clone, Debug)]
pub struct EqBands {
    bands: [GainRamp; BANDS],
}

impl EqBands {
    pub fn new(now: Instant) -> Self {
        Self {
            bands: std::array::from_fn(|_| GainRamp::new(now)),
        }
    }

    /// Glide every band to `gains`.
    pub fn glide(&mut self, now: Instant, gains: &Gains, over: Duration) {
        for (band, db) in self.bands.iter_mut().zip(gains) {
            band.glide(now, *db, over);
        }
    }

    pub fn is_ramping(&self) -> bool {
        self.bands.iter().any(GainRamp::is_ramping)
    }

    /// Where every band is heading.
    pub fn targets(&self) -> Gains {
        std::array::from_fn(|i| self.bands[i].target_db())
    }

    /// The bands that have moved since the last call, with the gain to give each now.
    pub fn step(&mut self, now: Instant) -> Vec<(usize, f32)> {
        self.bands
            .iter_mut()
            .enumerate()
            .filter_map(|(i, band)| band.step(now).map(|db| (i, db)))
            .collect()
    }

    /// Forget what was sent, so the next [`EqBands::step`] returns every band again (the
    /// filter graph was rebuilt and holds the install-time gains).
    pub fn resend(&mut self, now: Instant) {
        let targets = self.targets();
        for (band, db) in self.bands.iter_mut().zip(targets) {
            *band = GainRamp::new(now);
            band.jump(now, db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::audio::Preset;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn the_chain_is_ten_labelled_octave_wide_peaking_filters() {
        let s = chain(&Preset::BassBoost.gains());
        let parts: Vec<_> = s.split(",@").collect();
        assert_eq!(parts.len(), BANDS);
        assert_eq!(
            parts[0].trim_start_matches('@'),
            "eq0:lavfi=[equalizer=f=31:t=o:w=1:g=6.00]"
        );
        assert!(
            s.contains("@eq5:lavfi=[equalizer=f=1000:t=o:w=1:g=0.00]"),
            "{s}"
        );
        assert!(
            s.ends_with("@eq9:lavfi=[equalizer=f=16000:t=o:w=1:g=0.00]"),
            "{s}"
        );
        // Only the characters mpv's option parser expects around a graph.
        assert!(!s.contains(' '));
    }

    #[test]
    fn a_negative_gain_keeps_its_sign_and_the_decimal_point_is_a_dot() {
        assert_eq!(gain_arg(-3.0), "-3.00");
        assert_eq!(gain_arg(0.5), "0.50");
        assert!(chain(&Preset::Vocal.gains()).contains("f=62:t=o:w=1:g=-3.00"));
    }

    #[test]
    fn labels_are_stable() {
        assert_eq!(label(0), "eq0");
        assert_eq!(label(9), "eq9");
    }

    #[test]
    fn a_new_curve_glides_every_band_to_its_gain_and_lands_exactly() {
        let t0 = Instant::now();
        let mut b = EqBands::new(t0);
        b.step(t0);
        b.glide(t0, &Preset::BassBoost.gains(), MS(80));
        assert!(b.is_ramping());
        let mid = b.step(t0 + MS(40));
        // The bands that move are the ones the curve changes; the flat ones stay quiet.
        assert!(mid.iter().all(|(i, _)| *i < 4), "{mid:?}");
        let (_, first) = mid.iter().find(|(i, _)| *i == 0).copied().unwrap();
        assert!((first - 3.0).abs() < 1e-3, "halfway to +6: {first}");
        let end = b.step(t0 + MS(80));
        assert!(
            end.contains(&(0, 6.0)) && end.contains(&(1, 5.0)),
            "{end:?}"
        );
        assert!(!b.is_ramping());
        assert!(
            b.step(t0 + MS(200)).is_empty(),
            "settled bands are not re-sent"
        );
    }

    #[test]
    fn the_first_step_sends_every_band_once() {
        let t0 = Instant::now();
        let mut b = EqBands::new(t0);
        assert_eq!(b.step(t0).len(), BANDS);
        assert!(b.step(t0 + MS(10)).is_empty());
    }

    #[test]
    fn resend_brings_every_band_back_at_its_target_and_stops_any_glide() {
        let t0 = Instant::now();
        let mut b = EqBands::new(t0);
        b.glide(t0, &Preset::TrebleBoost.gains(), MS(80));
        b.step(t0 + MS(80));
        b.resend(t0 + MS(100));
        let all = b.step(t0 + MS(100));
        assert_eq!(all.len(), BANDS);
        let sent: Vec<f32> = all.iter().map(|(_, db)| *db).collect();
        assert_eq!(sent, Preset::TrebleBoost.gains());
        assert_eq!(b.targets(), Preset::TrebleBoost.gains());
    }

    #[test]
    fn redirecting_a_glide_starts_from_where_the_band_is() {
        let t0 = Instant::now();
        let mut b = EqBands::new(t0);
        b.glide(t0, &[8.0; BANDS], MS(80));
        b.step(t0 + MS(40));
        b.glide(t0 + MS(40), &[0.0; BANDS], MS(80));
        let back = b.step(t0 + MS(80));
        let (_, db) = back[0];
        assert!((db - 2.0).abs() < 1e-3, "from +4 toward 0, halfway: {db}");
    }
}
