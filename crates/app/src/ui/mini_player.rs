//! The mini player: a compact borderless window (360x96) on the same dark chrome and Mica as the
//! main window, for keeping music in a corner of the screen while the main window is away.
//!
//! Square art on the left (double-click it to go back), title and artist in the middle, previous,
//! play/pause and next on the right, a thin accent progress line along the bottom edge, and an
//! expand button in the top-right corner that shows while the pointer is over the window.
//! Everything that is not art or a button drags the window.
//!
//! **HUD mode** (Settings > Window) turns the same window into an overlay for watching and
//! controlling music over a game: always on top, slightly translucent, never the active window
//! (it cannot take the keyboard), and click-through until the pointer is over it. Over it, it
//! goes solid and takes clicks. It snaps to screen edges and corners where it is dropped, and
//! with auto-hide on it dims after a few seconds without a track change (`hud.rs`).
//!
//! Which window it is, where it opens and how it gives way to the main window is
//! `crate::mini`; this file is the view.

use std::rc::Rc;
use std::time::{Duration, Instant};

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, relative, App, ClickEvent, Context, Entity, FocusHandle,
    Focusable, IntoElement, KeyBinding, ParentElement, Render, Styled, Subscription, Window,
    WindowControlArea,
};

use crate::assets::Icon;
use crate::hud::{HudFade, SETTLE};
use crate::player::{PlayerModel, Status};
use crate::settings::{SettingsModel, HUD_AUTO_HIDE, MINI_PLAYER_ON_TOP};
use crate::style::color;
use crate::ui::art_tile::art_tile;
use crate::ui::button::IconButton;
use crate::ui::player_bar::text_line;
use crate::ui::shell::{NextTrack, PreviousTrack, TogglePlay};
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
/// How often the HUD looks at where the pointer is (clicks fall through it, so it gets no
/// mouse events to tell it).
pub const POLL: Duration = Duration::from_millis(50);
/// The HUD's fill: bg.overlay, a little see-through.
const HUD_FILL_ALPHA: f32 = 0.85;

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

pub struct MiniPlayer {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
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
    /// The title and artist last shown, to tell a new track from a changed state.
    track: (gpui::SharedString, gpui::SharedString),
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
        hud: bool,
        on_expand: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = vec![
            cx.observe(&player, |this, player, cx| {
                let view = player.read(cx).view();
                let track = (view.title, view.artist);
                if track != this.track {
                    this.track = track;
                    // A new song: the HUD shows itself again for a moment.
                    let now = (this.clock)();
                    this.fade.wake(now);
                }
                cx.notify();
            }),
            cx.observe(&settings, |_, _, cx| cx.notify()),
        ];
        let view = player.read(cx).view();
        let auto_hide = settings.read(cx).get(&HUD_AUTO_HIDE);
        Self {
            hud,
            fade: HudFade::new(Instant::now(), auto_hide),
            pointer_inside: false,
            pointer: Rc::new(crate::platform::pointer_over),
            clock: Rc::new(Instant::now),
            click_through: None,
            track: (view.title, view.artist),
            settle: None,
            snapped: None,
            player,
            settings,
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
        }
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
        let auto_hide = self.settings.read(cx).get(&HUD_AUTO_HIDE);
        self.fade = HudFade::new((self.clock)(), auto_hide);
    }

    /// Starts what HUD mode needs: the window never becomes the active one, clicks pass
    /// through it, and a poll follows the pointer, the fade and where the window is dropped.
    pub fn start_hud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.hud {
            return;
        }
        crate::platform::set_no_activate(window);
        self.click_through = Some(true);
        crate::platform::set_click_through(window, true);
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
            self.fade.set_hovered(inside);
            // Solid and clickable under the pointer; otherwise the game gets every click.
            self.click_through = Some(!inside);
            crate::platform::set_click_through(window, !inside);
            changed = true;
        }
        let auto_hide = self.settings.read(cx).get(&HUD_AUTO_HIDE);
        self.fade.set_auto_hide(auto_hide, now);
        changed |= self.fade.step(now);

        let rect = Rect::from_bounds(window.bounds());
        match self.settle {
            Some((at, since)) if at == rect => {
                if now.saturating_duration_since(since) >= SETTLE && self.snapped != Some(rect) {
                    let displays = window_state::usable_displays(cx, window.scale_factor());
                    let target = window_state::snap(rect, &displays);
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
        let v = self.player.read(cx).view();
        if v.accent_moving {
            window.request_animation_frame();
        }
        self.style_window(window, cx);
        let accent = v.accent;
        let ready = v.status == Status::Ready;
        let controls = ready && v.has_track;

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
            .id("mini-controls")
            .occlude()
            .on_hover(self.hover_listener(Region::Controls, cx))
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

        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
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
            ));

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
            .when(!self.hud, |d| d.bg(color::bg_base()))
            .when(self.hud, |d| {
                d.bg(color::bg_overlay().opacity(HUD_FILL_ALPHA))
                    .rounded(px(radius::MEDIUM))
                    .opacity(self.fade.level())
            })
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
                    .child(text)
                    .child(transport),
            )
            .child(progress)
            .children(expand)
    }
}
