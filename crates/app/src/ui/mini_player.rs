//! The mini player: a compact borderless window (360x96) on the same dark chrome and Mica as the
//! main window, for keeping music in a corner of the screen while the main window is away.
//!
//! Square art on the left (double-click it to go back), title and artist in the middle, previous,
//! play/pause and next on the right with a compact volume slider under them, a thin accent
//! progress line along the bottom edge, and an expand button in the top-right corner that shows
//! while the pointer is over the window. Everything that is not art or a button drags the
//! window, and the mouse wheel anywhere over it changes the volume by 5%.
//!
//! Left alone for a few seconds the controls fade out and then the whole window dims; the
//! pointer coming back restores both at once (`hud.rs`, `IdleFade`). With "Show lyrics" on, the
//! current synced lyric line sits under the artist (here and in the HUD), cross-fading as it
//! changes. All of it is Settings > Mini player.
//!
//! **HUD mode** (Settings > Mini player) turns the same window into an overlay for watching and
//! controlling music over a game: a 320x64 panel (art, title, artist, a thin progress line) on
//! a window of exactly that size (no margin, no shadow, no frame), always on top, never the active
//! window (it cannot take the keyboard), and click-through until the pointer is over it. Over
//! it, the panel goes opaque, takes clicks and shows previous, play/pause, next and the heart.
//! It snaps to screen edges and corners where it is dropped, and with auto-hide on it dims
//! after a few seconds without a track change (`hud.rs`). The look is DIRECTION.md, HUD.
//!
//! Which window it is, where it opens and how it gives way to the main window is
//! `crate::mini`; this file is the view.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_lyrics::Lyrics;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, canvas, div, prelude::*, px, relative, white, AnyElement, App, ClickEvent, Context,
    DispatchPhase, Div, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, ParentElement,
    Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Styled, Subscription, Window,
    WindowControlArea,
};

use crate::assets::Icon;
use crate::hud::{
    wheel_steps, HudFade, IdleFade, LineFade, Swap, SETTLE, SWAP_SLIDE, WHEEL_VOLUME_STEP,
};
use crate::library::LibraryWrites;
use crate::lyrics::{
    current_line, page_chunks, page_index, page_window_ms, LyricsModel, LyricsState,
};
use crate::player::{PlayerModel, Status};
use crate::settings::{
    SettingsModel, HUD_AUTO_HIDE, MINI_DIM, MINI_FADE_CONTROLS, MINI_PLAYER_ON_TOP,
    MINI_SHOW_LYRICS, MINI_SHOW_VOLUME,
};
use crate::style::{self, color};
use crate::ui::art_tile::{art_tile, art_tile_rounded};
use crate::ui::button::IconButton;
use crate::ui::player_bar::text_line;
use crate::ui::row_actions::{heart_icon, heart_label};
use crate::ui::shell::{NextTrack, PreviousTrack, TogglePlay};
use crate::ui::slider::{Slider, SliderState};
use crate::window_state::{self, Rect};

actions!(apollo_mini, [ExpandMiniPlayer]);

/// Key context of the mini player's root view.
pub const KEY_CONTEXT: &str = "MiniPlayer";

/// The window's size.
pub const WIDTH: f32 = 360.0;
pub const HEIGHT: f32 = 96.0;
/// The art is square and fills the height less the padding either side.
const ART: f32 = 80.0;
const SIDE_BUTTON: f32 = layout::MIN_HIT_TARGET;
const PLAY_BUTTON: f32 = 36.0;
/// The expand button is a small corner glyph, not a main control.
const EXPAND_BUTTON: f32 = 24.0;
const PROGRESS_HEIGHT: f32 = 2.0;
/// The compact volume slider under the transport buttons.
const VOLUME_WIDTH: f32 = 72.0;
/// The transport row: previous, play/pause, next and the gaps between.
const CONTROLS_W: f32 = 2.0 * SIDE_BUTTON + PLAY_BUTTON + 2.0 * spacing::S1;
/// One line of lyrics: 12px type, so a 16px row.
const LYRIC_LINE_H: f32 = 16.0;
/// Room kept back from a lyric line's width, so a page that just fits never gets an ellipsis.
const LYRIC_SLACK: f32 = 6.0;
/// How often the HUD looks at where the pointer is (clicks fall through it, so it gets no
/// mouse events to tell it).
pub const POLL: Duration = Duration::from_millis(50);

// ---- HUD (DIRECTION.md, HUD) ----
/// The panel's size, which is also the window's: no margin, so nothing but the panel is ever
/// on screen (a see-through margin showed up as a translucent frame on Windows).
pub const HUD_PANEL_W: f32 = 320.0;
pub const HUD_PANEL_H: f32 = 64.0;
pub const HUD_WIDTH: f32 = HUD_PANEL_W;
pub const HUD_HEIGHT: f32 = HUD_PANEL_H;
/// The panel's fill at rest: bg.overlay, a little see-through. (No acrylic blur: see
/// DIRECTION.md, HUD, for why this is the 92% fallback.)
const HUD_FILL_ALPHA: f32 = 0.92;
const HUD_ART: f32 = 48.0;
const HUD_ART_CORNER: f32 = 6.0;
const HUD_EDGE: f32 = 8.0;
const HUD_BUTTON: f32 = 28.0;
/// The four controls and the gaps between them.
const HUD_CONTROLS_W: f32 = 4.0 * HUD_BUTTON + 3.0 * spacing::S1;
const HUD_TRACK_ALPHA: f32 = 0.10;

/// The transport keys that work while the mini player has the keyboard, and Esc to go back.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-right", NextTrack, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-left", PreviousTrack, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ExpandMiniPlayer, Some(KEY_CONTEXT)),
    ]);
}

/// The pieces of the window the pointer can be over; the expand button shows while it is over
/// any of them. (The drag layer sits under the art and the buttons, which block the pointer
/// from it, so each reports for itself.)
#[derive(Clone, Copy)]
enum Region {
    Drag,
    Art,
    Controls,
    Expand,
}

/// The lyric lines on screen for one frame, each with its opacity: the current one, and the
/// one leaving while the cross-fade runs. Empty still takes the row's room.
#[derive(Clone, Default)]
struct LyricParts {
    lines: Vec<(SharedString, f32)>,
}

/// What the pages of a lyric line were worked out for: change any and they are worked out again.
#[derive(Clone, PartialEq, Eq)]
struct PagesKey {
    video: String,
    line: usize,
    room: i32,
}

/// The width of one line of `text` in the lyric style: the letters' advances added up. (Kerning
/// is ignored, hence [`LYRIC_SLACK`].)
fn text_width(cx: &App, text: &str) -> f32 {
    let system = cx.text_system();
    let family = cx
        .global::<style::Fonts>()
        .family_for(typography::HUD_ARTIST);
    let font = system.resolve_font(&gpui::font(family));
    let size = px(typography::HUD_ARTIST.size_px);
    text.chars()
        .map(|c| f32::from(system.layout_width(font, size, c)))
        .sum()
}

/// What the HUD shows of a track, kept so a change can cross-fade from the last one.
#[derive(Clone)]
struct Shown {
    title: SharedString,
    artist: SharedString,
    art: Option<Arc<RenderImage>>,
}

impl Shown {
    fn text_differs(&self, other: &Shown) -> bool {
        self.title != other.title || self.artist != other.artist
    }

    fn art_differs(&self, other: &Shown) -> bool {
        match (&self.art, &other.art) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        }
    }
}

pub struct MiniPlayer {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    writes: Entity<LibraryWrites>,
    lyrics: Entity<LyricsModel>,
    /// Goes back to the main window.
    on_expand: Rc<dyn Fn(&mut Window, &mut App)>,
    focus: FocusHandle,
    previous: FocusHandle,
    play: FocusHandle,
    next: FocusHandle,
    expand: FocusHandle,
    hover: [bool; 4],
    /// HUD mode (fixed for the life of the window; the setting is read when it opens).
    hud: bool,
    fade: HudFade,
    /// The pointer is over the HUD (looked up by polling, since clicks pass through it).
    pointer_inside: bool,
    pointer: Rc<dyn Fn(&Window) -> bool>,
    /// The time the HUD's fade and settling run on (tests move it by hand).
    clock: Rc<dyn Fn() -> Instant>,
    /// What click-through was last asked of the platform.
    click_through: Option<bool>,
    /// What was last shown, to tell a new track from a changed state, and what it replaced
    /// while the change-over runs.
    shown: Shown,
    prev: Option<Shown>,
    swap: Swap,
    like: FocusHandle,
    /// How visible the controls and the window are (the plain mini player).
    idle: IdleFade,
    volume_state: Entity<SliderState>,
    volume_focus: FocusHandle,
    /// Touchpad scroll not yet worth a volume step.
    wheel_carry: f32,
    /// The lyric line showing, and the one it is cross-fading away from.
    lyric: Option<SharedString>,
    lyric_prev: Option<SharedString>,
    lyric_fade: LineFade,
    /// The pages the current line was last split into, for the track, line and width they were
    /// worked out for.
    lyric_pages: Option<(PagesKey, Vec<String>)>,
    /// Where the window has sat still since when, and the spot it was last snapped for.
    settle: Option<(Rect, Instant)>,
    snapped: Option<Rect>,
    /// Window styling already asked of the platform: rounded corners once, and the last
    /// always-on-top choice applied.
    rounded: bool,
    on_top: Option<bool>,
    _subs: Vec<Subscription>,
}

impl MiniPlayer {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        writes: Entity<LibraryWrites>,
        lyrics: Entity<LyricsModel>,
        hud: bool,
        on_expand: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = vec![
            cx.observe(&player, |this, player, cx| {
                let view = player.read(cx).view();
                let now_shown = Shown {
                    title: view.title,
                    artist: view.artist,
                    art: view.art,
                };
                let text = now_shown.text_differs(&this.shown);
                let art = now_shown.art_differs(&this.shown);
                if this.hud && (text || art) {
                    // The old track gives way to the new over a moment.
                    this.prev = Some(std::mem::replace(&mut this.shown, now_shown));
                    let now = (this.clock)();
                    this.swap.start(now);
                } else {
                    this.shown = now_shown;
                }
                if text {
                    // A new song: it shows itself again for a moment.
                    let now = (this.clock)();
                    this.fade.wake(now);
                    this.idle.wake(now);
                }
                this.sync_lyric(cx);
                cx.notify();
            }),
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&writes, |_, _, cx| cx.notify()),
            cx.observe(&lyrics, |this, _, cx| {
                this.sync_lyric(cx);
                cx.notify();
            }),
        ];
        let view = player.read(cx).view();
        let (auto_hide, fade_controls, dim) = {
            let s = settings.read(cx);
            (
                s.get(&HUD_AUTO_HIDE),
                s.get(&MINI_FADE_CONTROLS),
                s.get(&MINI_DIM),
            )
        };
        let now = Instant::now();
        let mut this = Self {
            hud,
            fade: HudFade::new(now, auto_hide),
            idle: IdleFade::new(now, fade_controls, dim),
            volume_state: cx.new(|_| SliderState::default()),
            volume_focus: cx.focus_handle(),
            wheel_carry: 0.0,
            lyric: None,
            lyric_prev: None,
            lyric_fade: LineFade::default(),
            lyric_pages: None,
            lyrics,
            pointer_inside: false,
            pointer: Rc::new(|window| crate::platform::pointer_over(window, 0.0)),
            clock: Rc::new(Instant::now),
            click_through: None,
            shown: Shown {
                title: view.title,
                artist: view.artist,
                art: view.art,
            },
            prev: None,
            swap: Swap::default(),
            like: cx.focus_handle(),
            settle: None,
            snapped: None,
            player,
            settings,
            writes,
            on_expand: Rc::new(on_expand),
            focus: cx.focus_handle(),
            previous: cx.focus_handle(),
            play: cx.focus_handle(),
            next: cx.focus_handle(),
            expand: cx.focus_handle(),
            hover: [false; 4],
            rounded: false,
            on_top: None,
            _subs: subs,
        };
        this.lyric = this.current_lyric(cx);
        this
    }

    /// The room a lyric line has: the text column's width, less a little for shaping that
    /// differs from the sum of the letters. The column is narrower while the buttons are
    /// showing.
    fn lyric_room(&self) -> f32 {
        let (text_left, controls_showing, controls_w) = if self.hud {
            (
                2.0 * HUD_EDGE + HUD_ART + spacing::S3,
                self.fade.solid() > 0.0,
                HUD_CONTROLS_W,
            )
        } else {
            (
                2.0 * spacing::S2 + ART + spacing::S3,
                self.idle.controls() > 0.0,
                CONTROLS_W,
            )
        };
        let width = if self.hud { HUD_PANEL_W } else { WIDTH };
        let reserve = if controls_showing {
            controls_w + spacing::S3
        } else {
            0.0
        };
        width - text_left - reserve - LYRIC_SLACK
    }

    /// The synced lyric line being sung now, if the track has synced lyrics and a line has
    /// started. A blank line (an instrumental gap) shows nothing. A line too long for the room
    /// is shown in pages, one after another over the time it is sung (`lyrics::page_*`).
    fn current_lyric(&mut self, cx: &App) -> Option<SharedString> {
        let (video, position_ms) = {
            let player = self.player.read(cx);
            (player.current_video()?, player.view().position_ms)
        };
        let LyricsState::Ready(lyrics) = self.lyrics.read(cx).state(Some(video.as_str())) else {
            return None;
        };
        let Lyrics::Synced(lines) = &*lyrics else {
            return None;
        };
        let index = current_line(lines, position_ms)?;
        let line = &lines[index];
        let text = line.text.trim();
        if text.is_empty() {
            return None;
        }
        let room = self.lyric_room();
        let key = PagesKey {
            video,
            line: index,
            room: room.round() as i32,
        };
        if self.lyric_pages.as_ref().map(|(k, _)| k) != Some(&key) {
            let pages = page_chunks(text, |candidate| text_width(cx, candidate) <= room);
            self.lyric_pages = Some((key, pages));
        }
        let pages = &self.lyric_pages.as_ref()?.1;
        let shown = match pages.len() {
            0 | 1 => text,
            _ => {
                let window = page_window_ms(
                    text.chars().count(),
                    line.start_ms,
                    lines.get(index + 1).map(|next| next.start_ms),
                );
                let elapsed = position_ms.saturating_sub(line.start_ms);
                &pages[page_index(pages, window, elapsed)]
            }
        };
        Some(SharedString::from(shown.to_owned()))
    }

    /// Follows the lyrics to the current line, cross-fading when it changes.
    fn sync_lyric(&mut self, cx: &mut Context<Self>) {
        let line = self.current_lyric(cx);
        if line != self.lyric {
            self.lyric_prev = std::mem::replace(&mut self.lyric, line);
            // Nothing crosses over while the line is not on screen.
            if self.settings.read(cx).get(&MINI_SHOW_LYRICS) {
                self.lyric_fade.start((self.clock)());
            }
            cx.notify();
        }
    }

    /// The lyric line the window is showing (tests).
    pub fn lyric_line(&self) -> Option<&str> {
        self.lyric.as_deref()
    }

    /// How visible the plain mini player's controls are, 0 to 1.
    pub fn controls_level(&self) -> f32 {
        self.idle.controls()
    }

    /// How visible the plain mini player's window is, 1.0 solid to 0.6 dimmed.
    pub fn window_level(&self) -> f32 {
        self.idle.window()
    }

    /// Gives the keyboard to the window (Space, Ctrl+arrows and Esc work at once).
    pub fn focus_root(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// Keeps `subscription` (the position recorder) alive as long as the window.
    pub fn keep(&mut self, subscription: Option<Subscription>) {
        self._subs.extend(subscription);
    }

    /// Whether the expand button is showing (the pointer is over the window).
    pub fn expand_visible(&self) -> bool {
        self.pointer_inside || self.hover.iter().any(|h| *h)
    }

    pub fn is_hud(&self) -> bool {
        self.hud
    }

    /// How visible the HUD is, 1.0 solid.
    pub fn hud_level(&self) -> f32 {
        self.fade.level()
    }

    /// How opaque the HUD panel is under the pointer, 0 at rest to 1 solid.
    pub fn hud_solid(&self) -> f32 {
        self.fade.solid()
    }

    /// A new track's title, artist and art are still changing over.
    pub fn changing_over(&self) -> bool {
        self.swap.is_running()
    }

    /// What click-through was last asked of the platform (`None` before the HUD started).
    pub fn click_through_requested(&self) -> Option<bool> {
        self.click_through
    }

    /// The spot the HUD was last snapped to, if it has been.
    pub fn last_snap(&self) -> Option<Rect> {
        self.snapped
    }

    /// Replaces how the pointer is found (tests).
    pub fn set_pointer_probe(&mut self, probe: impl Fn(&Window) -> bool + 'static) {
        self.pointer = Rc::new(probe);
    }

    /// Replaces the clock the fade runs on (tests).
    pub fn set_clock(&mut self, clock: impl Fn() -> Instant + 'static, cx: &App) {
        self.clock = Rc::new(clock);
        let (auto_hide, fade_controls, dim) = {
            let s = self.settings.read(cx);
            (
                s.get(&HUD_AUTO_HIDE),
                s.get(&MINI_FADE_CONTROLS),
                s.get(&MINI_DIM),
            )
        };
        let now = (self.clock)();
        self.fade = HudFade::new(now, auto_hide);
        self.idle = IdleFade::new(now, fade_controls, dim);
    }

    /// Starts what the window needs to move on its own: a poll follows the pointer and drives
    /// the fades. In HUD mode the window also never becomes the active one, clicks pass through
    /// it, and the poll watches where the window is dropped.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hud {
            crate::platform::set_no_activate(window);
            self.click_through = Some(true);
            crate::platform::set_click_through(window, true);
        }
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            if this
                .update_in(cx, |this, window, cx| this.tick(window, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    /// One look at the world: the pointer, the fade, and whether the window has been left
    /// somewhere it should snap from.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = (self.clock)();
        let mut changed = false;
        let inside = (self.pointer)(window);
        if inside != self.pointer_inside {
            self.pointer_inside = inside;
            changed = true;
            if self.hud {
                self.fade.set_hovered(inside);
                // Solid and clickable under the pointer; otherwise the game gets every click.
                self.click_through = Some(!inside);
                crate::platform::set_click_through(window, !inside);
            }
        }
        if !self.hud {
            changed |= self.step_idle(now, cx);
            // The buttons coming or going changes how much room a lyric line has.
            self.sync_lyric(cx);
            if changed {
                cx.notify();
            }
            return;
        }
        let auto_hide = self.settings.read(cx).get(&HUD_AUTO_HIDE);
        self.fade.set_auto_hide(auto_hide, now);
        self.fade.set_animated(!cx.reduce_motion());
        changed |= self.fade.step(now);
        self.sync_lyric(cx);
        // The change-over of a new track is drawn from the clock; keep it coming.
        if self.swap.is_running() {
            let _ = self.swap.progress(now, !cx.reduce_motion());
            changed = true;
        }
        changed |= self.step_lyric_fade(now, cx);

        let rect = Rect::from_bounds(window.bounds());
        match self.settle {
            Some((at, since)) if at == rect => {
                if now.saturating_duration_since(since) >= SETTLE && self.snapped != Some(rect) {
                    let displays = window_state::usable_displays(cx, window.scale_factor());
                    let panel = window_state::snap(rect, &displays);
                    let target = panel;
                    self.snapped = Some(target);
                    if target != rect {
                        crate::platform::move_window(window, target.x, target.y);
                    }
                }
            }
            _ => self.settle = Some((rect, now)),
        }
        if changed {
            cx.notify();
        }
    }

    /// Moves the plain mini player's idle fade on: true when anything changed.
    fn step_idle(&mut self, now: Instant, cx: &App) -> bool {
        let (fade_controls, dim) = {
            let s = self.settings.read(cx);
            (s.get(&MINI_FADE_CONTROLS), s.get(&MINI_DIM))
        };
        let before = (self.idle.controls(), self.idle.window());
        self.idle.set_options(fade_controls, dim, now);
        self.idle.set_animated(!cx.reduce_motion());
        self.idle.set_hovered(self.expand_visible(), now);
        self.idle.step(now);
        let moved = before != (self.idle.controls(), self.idle.window());
        moved | self.step_lyric_fade(now, cx)
    }

    /// Keeps a lyric cross-fade coming (and ends it) from the poll as well as from drawing.
    fn step_lyric_fade(&mut self, now: Instant, cx: &App) -> bool {
        let running = self.lyric_fade.is_running();
        if running {
            let _ = self.lyric_fade.progress(now, !cx.reduce_motion());
        }
        running
    }

    /// A turn of the mouse wheel or a touchpad scroll: five percent of volume a notch.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.hud || !self.settings.read(cx).get(&MINI_SHOW_VOLUME) {
            return;
        }
        let (y, lines) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.y, true),
            ScrollDelta::Pixels(delta) => (f32::from(delta.y), false),
        };
        let (steps, carry) = wheel_steps(self.wheel_carry, y, lines);
        self.wheel_carry = carry;
        if steps != 0 {
            let delta = (steps.clamp(-10, 10) as i8) * WHEEL_VOLUME_STEP;
            self.player.update(cx, |m, cx| m.nudge_volume(delta, cx));
        }
    }

    /// The always-on-top choice last handed to the platform; `None` before the first frame.
    pub fn on_top_applied(&self) -> Option<bool> {
        self.on_top
    }

    fn hover_listener(
        &self,
        region: Region,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let this = cx.weak_entity();
        move |hovered, _, cx| {
            let _ = this.update(cx, |m, cx| {
                if m.hover[region as usize] != *hovered {
                    m.hover[region as usize] = *hovered;
                    // Coming back restores the controls and the window at once, not on the
                    // next poll.
                    let now = (m.clock)();
                    let over = m.expand_visible();
                    m.idle.set_hovered(over, now);
                    cx.notify();
                }
            });
        }
    }

    /// What the platform window needs once it exists, and again when a setting changes.
    fn style_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.rounded {
            self.rounded = true;
            crate::platform::round_corners(window);
            if self.hud {
                // A clean edge: no frame drawn round the panel.
                crate::platform::hide_border(window);
            }
        }
        // The HUD is always above everything; the setting is for the plain mini player.
        let on_top = self.hud || self.settings.read(cx).get(&MINI_PLAYER_ON_TOP);
        if self.on_top != Some(on_top) {
            self.on_top = Some(on_top);
            crate::platform::set_topmost(window, on_top);
        }
    }
}

impl Focusable for MiniPlayer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.hud {
            self.render_hud(window, cx)
        } else {
            self.render_plain(window, cx).into_any_element()
        }
    }
}

impl MiniPlayer {
    /// The mini player proper: art, text, transport and the expand corner on the main window's
    /// chrome.
    fn render_plain(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.player.read(cx).view();
        if v.accent_moving {
            window.request_animation_frame();
        }
        self.style_window(window, cx);
        let accent = v.accent;
        let ready = v.status == Status::Ready;
        let controls = ready && v.has_track;
        let (show_volume, show_lyrics) = {
            let s = self.settings.read(cx);
            (s.get(&MINI_SHOW_VOLUME), s.get(&MINI_SHOW_LYRICS))
        };
        let controls_level = self.idle.controls();

        let (play_icon, play_label) = if v.busy {
            (Icon::Spinner, "Pause")
        } else if v.playing {
            (Icon::Pause, "Pause")
        } else {
            (Icon::Play, "Play")
        };
        let button = |id: &'static str, focus: &FocusHandle, icon: Icon, label: &str, size: f32| {
            IconButton::new(id, focus, icon, label.to_owned(), accent)
                .size(size)
                .enabled(controls)
        };
        let transport = div()
            .id("mini-transport")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S1))
            .child(
                button(
                    "mini-previous",
                    &self.previous,
                    Icon::Previous,
                    "Previous",
                    SIDE_BUTTON,
                )
                .icon_color(color::text_primary())
                .on_click({
                    let p = self.player.clone();
                    move |_, cx| p.update(cx, |m, cx| m.previous(cx))
                }),
            )
            .child(
                button("mini-play", &self.play, play_icon, play_label, PLAY_BUTTON)
                    .circle()
                    .filled(if controls {
                        color::accent(accent)
                    } else {
                        color::bg_overlay()
                    })
                    .icon_color(color::accent_fg(accent))
                    .spinning(v.busy)
                    .on_click({
                        let p = self.player.clone();
                        move |_, cx| p.update(cx, |m, cx| m.toggle_play(cx))
                    }),
            )
            .child(
                button("mini-next", &self.next, Icon::Next, "Next", SIDE_BUTTON)
                    .icon_color(color::text_primary())
                    .on_click({
                        let p = self.player.clone();
                        move |_, cx| p.update(cx, |m, cx| m.next(cx))
                    }),
            );

        // The volume slider shares the main app's volume: same value, same saving.
        let volume_value = if v.muted {
            0.0
        } else {
            f32::from(v.volume) / 100.0
        };
        let volume = div()
            .id("mini-volume")
            .debug_selector(|| "mini-volume".into())
            .w(px(VOLUME_WIDTH))
            .child(
                Slider::new(
                    "mini-volume-slider",
                    &self.volume_state,
                    &self.volume_focus,
                    "Volume",
                    accent,
                )
                .value(volume_value)
                .value_text(format!("{}%", (volume_value * 100.0).round() as u32))
                .enabled(ready)
                .on_change({
                    let p = self.player.clone();
                    move |frac, committed, _, cx| {
                        let percent = (frac * 100.0).round() as u8;
                        p.update(cx, |m, cx| {
                            m.set_volume(percent, cx);
                            if committed {
                                m.save_volume();
                            }
                        });
                    }
                }),
            );

        // On the right: the buttons with the volume under them. They fade out when idle, and
        // while they are gone the text takes their room.
        let right = (controls_level > 0.0).then(|| {
            div()
                .id("mini-controls")
                .debug_selector(|| "mini-controls".into())
                .occlude()
                .on_hover(self.hover_listener(Region::Controls, cx))
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(spacing::S2))
                .w(px(CONTROLS_W))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .opacity(controls_level)
                .child(transport)
                .when(show_volume, |d| d.child(volume))
        });
        let reserve = (CONTROLS_W + spacing::S3) * controls_level;

        // Double-clicking the art goes back to the main window.
        let art = div()
            .id("mini-art")
            .occlude()
            .on_hover(self.hover_listener(Region::Art, cx))
            .cursor_pointer()
            .flex_none()
            .on_click({
                let expand = self.on_expand.clone();
                move |event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        expand(window, cx);
                    }
                }
            })
            .child(art_tile(ART, v.art.clone()));

        let lyric = show_lyrics.then(|| {
            let parts = self.lyric_parts(window, cx);
            Self::lyric_row(&parts, cx)
        });
        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .mr(px(reserve))
            .child(text_line(
                "mini-title",
                v.title.clone(),
                typography::BODY_STRONG,
                color::text_primary(),
                cx,
            ))
            .child(text_line(
                "mini-artist",
                v.artist.clone(),
                typography::BODY,
                color::text_secondary(),
                cx,
            ))
            .children(lyric);

        let fraction = match v.duration_ms {
            Some(total) if total > 0 => (v.position_ms as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let progress = div()
            .id("mini-progress")
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(PROGRESS_HEIGHT))
            .bg(color::state_hover())
            .child(
                div()
                    .h_full()
                    .w(relative(fraction))
                    .bg(color::accent(accent)),
            );

        let expand = self.expand_visible().then(|| {
            div()
                .id("mini-expand")
                .occlude()
                .on_hover(self.hover_listener(Region::Expand, cx))
                .absolute()
                .top(px(spacing::S1))
                .right(px(spacing::S1))
                .child(
                    IconButton::new(
                        "mini-expand-button",
                        &self.expand,
                        Icon::ArrowMaximize,
                        "Expand",
                        accent,
                    )
                    .size(EXPAND_BUTTON)
                    .on_click({
                        let expand = self.on_expand.clone();
                        move |window, cx| expand(window, cx)
                    }),
                )
        });

        // The wheel is read for the whole window: the art and the buttons block the pointer from
        // the root, so a listener there would miss them.
        let wheel = {
            let this = cx.weak_entity();
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let this = this.clone();
                    window.on_mouse_event(
                        move |event: &ScrollWheelEvent, phase: DispatchPhase, _, cx: &mut App| {
                            if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                                let _ = this.update(cx, |m, cx| m.wheel(event, cx));
                            }
                        },
                    );
                },
            )
            .absolute()
            .size_full()
        };

        div()
            .id("mini-player")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.player.update(cx, |m, cx| m.toggle_play(cx));
            }))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| {
                this.player.update(cx, |m, cx| m.next(cx));
            }))
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| {
                this.player.update(cx, |m, cx| m.previous(cx));
            }))
            .on_action(cx.listener(|this, _: &ExpandMiniPlayer, window, cx| {
                let expand = this.on_expand.clone();
                expand(window, cx);
            }))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(color::bg_base())
            // Idle: the whole window dims, content and background together.
            .opacity(self.idle.window())
            // Under everything: wherever nothing else is, the pointer drags the window.
            .child(
                div()
                    .id("mini-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_hover(self.hover_listener(Region::Drag, cx)),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing::S3))
                    .px(px(spacing::S2))
                    .child(art)
                    .child(text),
            )
            .children(right)
            .child(progress)
            .child(wheel)
            .children(expand)
    }
}

impl MiniPlayer {
    /// The lyric lines to draw now, moving the cross-fade on and asking for frames while it runs.
    fn lyric_parts(&mut self, window: &mut Window, cx: &App) -> LyricParts {
        let progress = self
            .lyric_fade
            .progress((self.clock)(), !cx.reduce_motion());
        if progress.is_some() {
            window.request_animation_frame();
        } else {
            self.lyric_prev = None;
        }
        let t = progress.unwrap_or(1.0);
        let mut lines = Vec::new();
        if let Some(line) = &self.lyric {
            lines.push((line.clone(), t));
        }
        if let (Some(prev), Some(_)) = (&self.lyric_prev, progress) {
            lines.push((prev.clone(), 1.0 - t));
        }
        LyricParts { lines }
    }

    /// One line of lyrics, 12px text.secondary, single line with an ellipsis; the lines in
    /// `parts` are laid over each other so they cross-fade in place.
    fn lyric_row(parts: &LyricParts, cx: &App) -> Div {
        let mut row = div()
            .debug_selector(|| "mini-lyric".into())
            .relative()
            .w_full()
            .h(px(LYRIC_LINE_H))
            .flex_none();
        for (text, opacity) in &parts.lines {
            row = row.child(
                style::text_style(
                    div().absolute().top_0().left_0().right_0(),
                    typography::HUD_ARTIST,
                    cx,
                )
                .truncate()
                .text_color(color::text_secondary())
                .opacity(*opacity)
                .child(text.clone()),
            );
        }
        row
    }

    /// One title-over-artist column, centred, `dy` pixels down from its place and at `opacity`,
    /// with the lyric row under the artist when lyrics are on.
    fn hud_column(
        shown: &Shown,
        dy: f32,
        opacity: f32,
        right: f32,
        lyric: Option<&LyricParts>,
        cx: &App,
    ) -> Div {
        let line = |text: SharedString, style: typography::TextStyle, c: gpui::Hsla| {
            crate::style::text_style(div(), style, cx)
                .truncate()
                .text_color(c)
                .child(text)
        };
        div()
            .absolute()
            .left_0()
            .right(px(right))
            .top(px(dy))
            .bottom(px(-dy))
            .flex()
            .flex_col()
            .justify_center()
            .opacity(opacity)
            .child(line(
                shown.title.clone(),
                typography::HUD_TITLE,
                color::text_primary(),
            ))
            .child(line(
                shown.artist.clone(),
                typography::HUD_ARTIST,
                color::text_secondary(),
            ))
            .when_some(lyric, |d, parts| d.child(Self::lyric_row(parts, cx)))
    }

    /// The HUD: the 320x64 panel, which is the whole window.
    fn render_hud(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let v = self.player.read(cx).view();
        if v.accent_moving {
            window.request_animation_frame();
        }
        self.style_window(window, cx);
        let accent = v.accent;
        let controls = v.status == Status::Ready && v.has_track;

        let now = (self.clock)();
        let progress = self.swap.progress(now, !cx.reduce_motion());
        if progress.is_some() {
            window.request_animation_frame();
        } else {
            self.prev = None;
        }
        let show_lyrics = self.settings.read(cx).get(&MINI_SHOW_LYRICS);
        let lyric = show_lyrics.then(|| self.lyric_parts(window, cx));
        // The leaving title keeps the row's room but not the line itself.
        let gap = show_lyrics.then(LyricParts::default);
        let solid = self.fade.solid();
        let fill = HUD_FILL_ALPHA + (1.0 - HUD_FILL_ALPHA) * solid;
        // Room for the controls over the end of the text, eased in with them.
        let reserve = (HUD_CONTROLS_W + spacing::S3) * solid;

        // Title and artist: the old pair slides up and fades out as the new one rises into place.
        let text = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .when_some(
                self.prev
                    .as_ref()
                    .filter(|prev| prev.text_differs(&self.shown))
                    .zip(progress),
                |d, (prev, t)| {
                    d.child(Self::hud_column(
                        prev,
                        -SWAP_SLIDE * t,
                        1.0 - t,
                        reserve,
                        gap.as_ref(),
                        cx,
                    ))
                    .child(Self::hud_column(
                        &self.shown,
                        SWAP_SLIDE * (1.0 - t),
                        t,
                        reserve,
                        lyric.as_ref(),
                        cx,
                    ))
                },
            )
            .when(
                !(progress.is_some()
                    && self
                        .prev
                        .as_ref()
                        .is_some_and(|prev| prev.text_differs(&self.shown))),
                |d| {
                    d.child(Self::hud_column(
                        &self.shown,
                        0.0,
                        1.0,
                        reserve,
                        lyric.as_ref(),
                        cx,
                    ))
                },
            );

        // Double-clicking the art goes back to the main window; a new cover fades in over the old.
        let art = div()
            .id("mini-art")
            .occlude()
            .cursor_pointer()
            .relative()
            .flex_none()
            .size(px(HUD_ART))
            .on_click({
                let expand = self.on_expand.clone();
                move |event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        expand(window, cx);
                    }
                }
            })
            .child(art_tile_rounded(
                HUD_ART,
                self.shown.art.clone(),
                HUD_ART_CORNER,
            ))
            .when_some(
                self.prev
                    .as_ref()
                    .filter(|prev| prev.art_differs(&self.shown))
                    .zip(progress),
                |d, (prev, t)| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .opacity(1.0 - t)
                            .child(art_tile_rounded(HUD_ART, prev.art.clone(), HUD_ART_CORNER)),
                    )
                },
            );

        let playing = self.player.read(cx).current_video();
        let liked = playing
            .as_deref()
            .is_some_and(|id| self.writes.read(cx).is_liked(id));
        let (play_icon, play_label) = if v.busy {
            (Icon::Spinner, "Pause")
        } else if v.playing {
            (Icon::Pause, "Pause")
        } else {
            (Icon::Play, "Play")
        };
        let button = |id: &'static str, focus: &FocusHandle, icon: Icon, label: &str| {
            IconButton::new(id, focus, icon, label.to_owned(), accent)
                .size(HUD_BUTTON)
                .icon_color(color::text_primary())
                .enabled(controls)
        };
        let buttons = (solid > 0.0).then(|| {
            div()
                .id("mini-controls")
                .debug_selector(|| "hud-controls".into())
                .occlude()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(HUD_EDGE))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S1))
                .opacity(solid)
                .child(
                    button("mini-previous", &self.previous, Icon::Previous, "Previous").on_click({
                        let p = self.player.clone();
                        move |_, cx| p.update(cx, |m, cx| m.previous(cx))
                    }),
                )
                .child(
                    button("mini-play", &self.play, play_icon, play_label)
                        .spinning(v.busy)
                        .on_click({
                            let p = self.player.clone();
                            move |_, cx| p.update(cx, |m, cx| m.toggle_play(cx))
                        }),
                )
                .child(
                    button("mini-next", &self.next, Icon::Next, "Next").on_click({
                        let p = self.player.clone();
                        move |_, cx| p.update(cx, |m, cx| m.next(cx))
                    }),
                )
                .child(
                    button(
                        "mini-like",
                        &self.like,
                        heart_icon(liked),
                        heart_label(liked),
                    )
                    .icon_color(if liked {
                        color::accent(accent)
                    } else {
                        color::text_primary()
                    })
                    .accessible_name("Like")
                    .toggled(liked)
                    .enabled(controls && playing.is_some())
                    .on_click({
                        let writes = self.writes.clone();
                        move |_, cx| {
                            if let Some(video) = &playing {
                                writes.update(cx, |w, cx| w.toggle_like(video, cx));
                            }
                        }
                    }),
                )
        });

        // The line along the bottom edge, drawn in by the corner radius so it clears the curve.
        let fraction = match v.duration_ms {
            Some(total) if total > 0 => (v.position_ms as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let line = div()
            .id("mini-progress")
            .absolute()
            .left(px(radius::MEDIUM))
            .right(px(radius::MEDIUM))
            .bottom_0()
            .h(px(PROGRESS_HEIGHT))
            .bg(white().opacity(HUD_TRACK_ALPHA))
            .child(
                div()
                    .h_full()
                    .w(relative(fraction))
                    .bg(color::accent(accent)),
            );

        let panel = div()
            .id("mini-hud")
            .debug_selector(|| "hud-panel".into())
            .absolute()
            .top_0()
            .left_0()
            .w(px(HUD_PANEL_W))
            .h(px(HUD_PANEL_H))
            // The corner Windows 11 gives the window itself, so the two agree.
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay().opacity(fill))
            .opacity(self.fade.level())
            // Under everything: wherever nothing else is, the pointer drags the window.
            .child(
                div()
                    .id("mini-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing::S3))
                    .px(px(HUD_EDGE))
                    .child(art)
                    .child(text),
            )
            .children(buttons)
            .child(line);

        div()
            .id("mini-player")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.player.update(cx, |m, cx| m.toggle_play(cx));
            }))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| {
                this.player.update(cx, |m, cx| m.next(cx));
            }))
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| {
                this.player.update(cx, |m, cx| m.previous(cx));
            }))
            .on_action(cx.listener(|this, _: &ExpandMiniPlayer, window, cx| {
                let expand = this.on_expand.clone();
                expand(window, cx);
            }))
            .relative()
            .size_full()
            .child(panel)
            .into_any_element()
    }
}
