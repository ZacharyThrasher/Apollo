//! Volume normalization policy: how much to turn a track down so tracks play at about the same
//! loudness. Pure arithmetic; the media layer applies the result per track.
//!
//! The input is YouTube's own measurement (`loudnessDb`): decibels against the reference level
//! its player normalizes to, so `+4.0` means "4 dB louder than the reference". The policy is the
//! one YouTube's player uses:
//!
//! * a loud track is attenuated by exactly its excess, which lands it on the reference;
//! * a quiet track is left alone, because boosting would clip the peaks and nothing here
//!   limits;
//! * a track with no measurement is left alone.

/// The most a track is ever turned down. Real measurements are a few dB either side of zero; a
/// number beyond this is a bad measurement, and silencing a track over it would be worse.
pub const MAX_ATTENUATION_DB: f32 = 24.0;

/// The gain in dB to apply to a track measured at `loudness_db`: `0.0` (unity) or negative, never
/// below `-MAX_ATTENUATION_DB`. `None` and non-finite values are unity.
pub fn gain_db(loudness_db: Option<f32>) -> f32 {
    match loudness_db {
        Some(l) if l.is_finite() && l > 0.0 => -l.min(MAX_ATTENUATION_DB),
        _ => 0.0,
    }
}

/// The linear amplitude factor for a gain in dB.
pub fn gain_factor(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loud_track_is_turned_down_by_its_excess() {
        assert_eq!(gain_db(Some(4.0)), -4.0);
        assert!((gain_db(Some(0.5)) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_quiet_track_is_never_boosted() {
        assert_eq!(gain_db(Some(-6.5)), 0.0);
        assert_eq!(gain_db(Some(0.0)), 0.0);
    }

    #[test]
    fn an_unknown_or_nonsense_measurement_is_unity() {
        assert_eq!(gain_db(None), 0.0);
        assert_eq!(gain_db(Some(f32::NAN)), 0.0);
        assert_eq!(gain_db(Some(f32::INFINITY)), 0.0);
        assert_eq!(gain_db(Some(f32::NEG_INFINITY)), 0.0);
    }

    #[test]
    fn attenuation_is_bounded() {
        assert_eq!(gain_db(Some(60.0)), -MAX_ATTENUATION_DB);
    }

    #[test]
    fn the_result_never_exceeds_unity() {
        for tenths in -400..=400 {
            let g = gain_db(Some(tenths as f32 / 10.0));
            assert!((-MAX_ATTENUATION_DB..=0.0).contains(&g), "{tenths}: {g}");
        }
    }

    #[test]
    fn factor_matches_decibels() {
        assert!((gain_factor(0.0) - 1.0).abs() < 1e-6);
        assert!((gain_factor(-6.0206) - 0.5).abs() < 1e-3);
    }
}
