//! The notification-area icon: what it says, what its menu does, and when closing the window
//! hides it instead. Kept apart from the window system so the decisions are testable anywhere;
//! the Windows end is `tray_win.rs` (the `tray-icon` crate on the thread GPUI already pumps, so
//! there is no second event loop).
//!
//! * **State out.** [`TrayBridge`] turns the player's [`MediaState`] into as few calls on a
//!   [`TraySink`] as keep the icon right: the tooltip ("Title · Artist", or "Apollo" when
//!   nothing is loaded) and the first menu item ("Pause" while playing, "Play" otherwise).
//! * **Commands in.** [`TrayCommand`] is what a click or a menu item asks for.

use std::time::{Duration, Instant};

use apollo_theme::Accent;

use crate::os_media::{MediaState, MediaTrack, OsCommand};

/// What the tooltip says when nothing is loaded, and the name on the tray menu's window item.
pub const APP_NAME: &str = "Apollo";

/// The notification area shows at most 127 UTF-16 units of tooltip (the 128th is the NUL).
const TOOLTIP_UNITS: usize = 127;
/// A left click arriving this soon after the last one is the second half of a double click,
/// which would put the window straight back where it was.
const CLICK_GAP: Duration = Duration::from_millis(400);

/// A request from the tray icon or its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    /// Left click: hide the window if it is showing, show it otherwise.
    ToggleWindow,
    /// "Show Apollo".
    ShowWindow,
    /// "Mini player".
    MiniPlayer,
    PlayPause,
    Next,
    Previous,
    Quit,
}

impl TrayCommand {
    /// The player command for the transport items; the rest are the window's and the app's.
    pub fn player_command(self) -> Option<OsCommand> {
        match self {
            TrayCommand::PlayPause => Some(OsCommand::Toggle),
            TrayCommand::Next => Some(OsCommand::Next),
            TrayCommand::Previous => Some(OsCommand::Previous),
            TrayCommand::ToggleWindow
            | TrayCommand::ShowWindow
            | TrayCommand::MiniPlayer
            | TrayCommand::Quit => None,
        }
    }
}

/// The platform end: the real tray icon on Windows, a recorder in tests.
pub trait TraySink {
    fn set_tooltip(&mut self, text: &str);
    fn set_playing(&mut self, playing: bool);
}

/// Tooltip for the current track. The title alone when there is no artist; `Apollo` when
/// nothing is loaded. Long text is cut with an ellipsis so the notification area accepts it.
pub fn tooltip(track: Option<&MediaTrack>) -> String {
    let text = match track {
        Some(t) if !t.title.is_empty() && !t.artist.is_empty() => {
            format!("{} \u{b7} {}", t.title, t.artist)
        }
        Some(t) if !t.title.is_empty() => t.title.clone(),
        _ => APP_NAME.to_owned(),
    };
    clip_utf16(&text, TOOLTIP_UNITS)
}

fn clip_utf16(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        // Leave room for the ellipsis (one unit).
        if units + c.len_utf16() > max_units - 1 {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

/// Decides what to tell the sink, and when.
pub struct TrayBridge<S: TraySink> {
    sink: S,
    tooltip: Option<String>,
    playing: Option<bool>,
}

impl<S: TraySink> TrayBridge<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            tooltip: None,
            playing: None,
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn apply(&mut self, state: &MediaState) {
        let tooltip = tooltip(state.track.as_ref());
        if self.tooltip.as_deref() != Some(tooltip.as_str()) {
            self.sink.set_tooltip(&tooltip);
            self.tooltip = Some(tooltip);
        }
        let playing = state.playing && state.track.is_some();
        if self.playing != Some(playing) {
            self.sink.set_playing(playing);
            self.playing = Some(playing);
        }
    }
}

/// What the window's close button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide the window; playback and the app carry on.
    Hide,
    /// Close it, which ends the app.
    Close,
}

/// Close hides to the tray only when the owner asked for that *and* there is a tray icon to get
/// the window back from; without one a hidden window could never be found again.
pub fn on_close_requested(close_to_tray: bool, tray_present: bool) -> CloseAction {
    if close_to_tray && tray_present {
        CloseAction::Hide
    } else {
        CloseAction::Close
    }
}

/// Drops a left click that follows another too closely (a double click's second half).
#[derive(Default)]
pub struct ClickGate {
    last: Option<Instant>,
}

impl ClickGate {
    /// Whether a click at `now` should act.
    pub fn accept(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|at| now.saturating_duration_since(at) < CLICK_GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// The icon: the accent as a disc with a play triangle cut in the app's base colour, as straight
/// RGBA, `size` pixels square. Edges are anti-aliased by 4x4 supersampling.
pub fn icon_rgba(size: u32, accent: Accent) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let s = size as f32;
    let (center, radius) = (s / 2.0, s * 0.47);
    // The triangle sits a little right of centre so it reads as centred.
    let tri = [
        (0.39 * s, 0.27 * s),
        (0.39 * s, 0.73 * s),
        (0.75 * s, 0.50 * s),
    ];
    let inside_triangle = |x: f32, y: f32| {
        let edge = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
        let (d1, d2, d3) = (
            edge(tri[0], tri[1]),
            edge(tri[1], tri[2]),
            edge(tri[2], tri[0]),
        );
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut disc, mut glyph) = (0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = px as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                    if (x - center).powi(2) + (y - center).powi(2) <= radius * radius {
                        disc += 1;
                        if inside_triangle(x, y) {
                            glyph += 1;
                        }
                    }
                }
            }
            let total = (SAMPLES * SAMPLES) as f32;
            let alpha = disc as f32 / total;
            // Share of the disc coverage that is triangle: blend fg over the accent.
            let mix = if disc == 0 {
                0.0
            } else {
                glyph as f32 / disc as f32
            };
            let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * mix).round() as u8;
            out.extend_from_slice(&[
                blend(accent.rgb.r, accent.fg.r),
                blend(accent.rgb.g, accent.fg.g),
                blend(accent.rgb.b, accent.fg.b),
                (alpha * 255.0).round() as u8,
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str) -> MediaTrack {
        MediaTrack {
            video_id: "id".into(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_ms: None,
        }
    }

    #[derive(Default)]
    struct Recorder {
        tooltips: Vec<String>,
        playing: Vec<bool>,
    }

    impl TraySink for Recorder {
        fn set_tooltip(&mut self, text: &str) {
            self.tooltips.push(text.to_owned());
        }
        fn set_playing(&mut self, playing: bool) {
            self.playing.push(playing);
        }
    }

    fn state(track: Option<MediaTrack>, playing: bool) -> MediaState {
        MediaState {
            track,
            playing,
            ..MediaState::default()
        }
    }

    #[test]
    fn the_tooltip_is_title_dot_artist_or_the_app_name() {
        assert_eq!(tooltip(Some(&track("Song", "Band"))), "Song \u{b7} Band");
        assert_eq!(tooltip(Some(&track("Song", ""))), "Song");
        assert_eq!(tooltip(Some(&track("", "Band"))), "Apollo");
        assert_eq!(tooltip(None), "Apollo");
    }

    #[test]
    fn a_long_tooltip_is_cut_to_what_the_notification_area_takes() {
        let long = "x".repeat(300);
        let text = tooltip(Some(&track(&long, "Band")));
        assert_eq!(text.encode_utf16().count(), TOOLTIP_UNITS);
        assert!(text.ends_with('\u{2026}'));
        // Characters outside the BMP count as two units and are never split.
        let emoji = "\u{1f3b5}".repeat(100);
        let text = tooltip(Some(&track(&emoji, "")));
        assert!(text.encode_utf16().count() <= TOOLTIP_UNITS);
        assert!(text.ends_with('\u{2026}'));
    }

    #[test]
    fn the_bridge_only_speaks_when_something_changed() {
        let mut bridge = TrayBridge::new(Recorder::default());
        bridge.apply(&state(None, false));
        bridge.apply(&state(None, false));
        assert_eq!(bridge.sink().tooltips, ["Apollo"]);
        assert_eq!(bridge.sink().playing, [false]);

        let t = track("Song", "Band");
        bridge.apply(&state(Some(t.clone()), true));
        bridge.apply(&state(Some(t.clone()), true));
        assert_eq!(bridge.sink().tooltips, ["Apollo", "Song \u{b7} Band"]);
        assert_eq!(bridge.sink().playing, [false, true]);

        // Pausing changes the menu label, not the tooltip.
        bridge.apply(&state(Some(t), false));
        assert_eq!(bridge.sink().tooltips.len(), 2);
        assert_eq!(bridge.sink().playing, [false, true, false]);
    }

    #[test]
    fn nothing_loaded_never_shows_pause() {
        let mut bridge = TrayBridge::new(Recorder::default());
        bridge.apply(&state(None, true));
        assert_eq!(bridge.sink().playing, [false]);
    }

    #[test]
    fn close_hides_only_with_the_setting_and_a_tray_to_come_back_from() {
        assert_eq!(on_close_requested(true, true), CloseAction::Hide);
        assert_eq!(on_close_requested(false, true), CloseAction::Close);
        assert_eq!(on_close_requested(true, false), CloseAction::Close);
        assert_eq!(on_close_requested(false, false), CloseAction::Close);
    }

    #[test]
    fn the_second_half_of_a_double_click_is_dropped() {
        let t0 = Instant::now();
        let mut gate = ClickGate::default();
        assert!(gate.accept(t0));
        assert!(!gate.accept(t0 + Duration::from_millis(150)));
        assert!(!gate.accept(t0 + Duration::from_millis(300)));
        assert!(gate.accept(t0 + Duration::from_millis(900)));
    }

    #[test]
    fn transport_items_map_to_player_commands() {
        assert_eq!(
            TrayCommand::PlayPause.player_command(),
            Some(OsCommand::Toggle)
        );
        assert_eq!(TrayCommand::Next.player_command(), Some(OsCommand::Next));
        assert_eq!(
            TrayCommand::Previous.player_command(),
            Some(OsCommand::Previous)
        );
        assert_eq!(TrayCommand::Quit.player_command(), None);
        assert_eq!(TrayCommand::MiniPlayer.player_command(), None);
        assert_eq!(TrayCommand::ToggleWindow.player_command(), None);
    }

    #[test]
    fn the_icon_is_a_round_accent_disc_with_a_dark_play_triangle() {
        let accent = Accent::fallback();
        let size = 32u32;
        let px = icon_rgba(size, accent);
        assert_eq!(px.len(), (size * size * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0, "corners are transparent");
        // Left of the triangle, inside the disc: the accent.
        assert_eq!(at(8, 16), [accent.rgb.r, accent.rgb.g, accent.rgb.b, 255]);
        // Inside the triangle: the dark base colour.
        assert_eq!(at(16, 16), [accent.fg.r, accent.fg.g, accent.fg.b, 255]);
    }
}
