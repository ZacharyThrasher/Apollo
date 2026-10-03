//! The window's root view: title bar, nav rail, page and player bar.

use std::rc::Rc;
use std::time::Instant;

use apollo_theme::tokens::{layout, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, Window,
};

use crate::motion::PanelSlide;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::nav::{NavRail, Route};
use crate::ui::now_playing::NowPlaying;
use crate::ui::player_bar::{BarFocus, PlayerBar};
use crate::ui::queue_panel::QueuePanel;
use crate::ui::slider::{SliderState, StepBack, StepForward};
use crate::ui::titlebar::TitleBar;

actions!(
    apollo,
    [
        TogglePlay,
        NextTrack,
        PreviousTrack,
        CloseNowPlaying,
        FocusNextControl,
        FocusPreviousControl,
        Quit
    ]
);

/// Key context of the root view; global shortcuts bind here.
pub const KEY_CONTEXT: &str = "Shell";

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-right", NextTrack, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-left", PreviousTrack, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseNowPlaying, Some(KEY_CONTEXT)),
        KeyBinding::new("tab", FocusNextControl, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(KEY_CONTEXT)),
        KeyBinding::new("left", StepBack, Some(crate::ui::slider::KEY_CONTEXT)),
        KeyBinding::new("right", StepForward, Some(crate::ui::slider::KEY_CONTEXT)),
    ]);
    crate::ui::queue_panel::bind_keys(cx);
}

pub struct Shell {
    player: Entity<PlayerModel>,
    route: Route,
    nav_expanded: bool,
    nav_toggled: bool,
    queue: PanelSlide,
    queue_panel: Entity<QueuePanel>,
    now_playing_open: bool,
    now_playing: Entity<NowPlaying>,
    root_focus: FocusHandle,
    nav_toggle: FocusHandle,
    nav_items: [FocusHandle; 3],
    bar: BarFocus,
    progress: Entity<SliderState>,
    volume: Entity<SliderState>,
}

impl Shell {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        // Tab order: nav toggle, nav items, then the player bar left to right.
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        let nav_toggle = stop(cx, 1);
        let nav_items = [stop(cx, 2), stop(cx, 3), stop(cx, 4)];
        // Nav 1-4, Now Playing 5-11, queue panel 12-16, then the player bar.
        let bar = BarFocus {
            now_playing: stop(cx, 19),
            shuffle: stop(cx, 20),
            previous: stop(cx, 21),
            play: stop(cx, 22),
            next: stop(cx, 23),
            repeat: stop(cx, 24),
            progress: stop(cx, 25),
            retry: stop(cx, 25),
            mute: stop(cx, 26),
            volume: stop(cx, 27),
            queue: stop(cx, 28),
        };
        let progress = cx.new(|_| SliderState::default());
        let volume = cx.new(|_| SliderState::default());
        let root_focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let queue_panel = cx.new(|cx| {
            QueuePanel::new(
                player.clone(),
                {
                    let weak = weak.clone();
                    move |_, cx| {
                        let _ = weak.update(cx, |s, cx| s.show_route(Route::Search, cx));
                    }
                },
                cx,
            )
        });
        let now_playing = cx.new(|cx| {
            NowPlaying::new(
                player.clone(),
                {
                    let (weak, root) = (weak.clone(), root_focus.clone());
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.set_now_playing(false, cx));
                        window.focus(&root, cx);
                    }
                },
                cx,
            )
        });
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&progress, |_, _, cx| cx.notify()).detach();
        cx.observe(&volume, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            route: Route::Home,
            nav_expanded: false,
            nav_toggled: false,
            queue: PanelSlide::default(),
            queue_panel,
            now_playing_open: false,
            now_playing,
            root_focus,
            nav_toggle,
            nav_items,
            bar,
            progress,
            volume,
        }
    }

    pub fn route(&self) -> Route {
        self.route
    }

    pub fn nav_expanded(&self) -> bool {
        self.nav_expanded
    }

    pub fn queue_open(&self) -> bool {
        self.queue.is_open()
    }

    pub fn queue_panel(&self) -> &Entity<QueuePanel> {
        &self.queue_panel
    }

    pub fn now_playing_open(&self) -> bool {
        self.now_playing_open
    }

    fn show_route(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        self.now_playing_open = false;
        cx.notify();
    }

    pub fn set_now_playing(&mut self, open: bool, cx: &mut Context<Self>) {
        self.now_playing_open = open;
        cx.notify();
    }

    pub fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        let open = !self.queue.is_open();
        self.queue.set(open, Instant::now(), !cx.reduce_motion());
        if open {
            self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
        }
        cx.notify();
    }

    /// Put keyboard focus on the root so shortcuts work before anything is clicked.
    pub fn focus_root(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.root_focus, cx);
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.player.read(cx).view();
        if view.accent_moving {
            window.request_animation_frame();
        }
        let accent = view.accent;
        let shell = cx.entity();

        let nav = NavRail::new(
            self.nav_expanded,
            self.nav_toggled,
            self.route,
            accent,
            &self.nav_toggle,
            &self.nav_items,
        )
        .on_toggle({
            let shell = shell.clone();
            move |_, cx| {
                shell.update(cx, |s, cx| {
                    s.nav_expanded = !s.nav_expanded;
                    s.nav_toggled = true;
                    cx.notify();
                });
            }
        })
        .on_route({
            let shell = shell.clone();
            move |route, _, cx| {
                shell.update(cx, |s, cx| s.show_route(route, cx));
            }
        });

        let page = div()
            .id("page")
            .flex_1()
            .min_w_0()
            .h_full()
            .when(!self.now_playing_open, |d| {
                d.overflow_y_scroll()
                    .px(px(spacing::S8))
                    .pt(px(spacing::S6))
                    .child(
                        style::text_style(div(), typography::PAGE_TITLE, cx)
                            .text_color(color::text_primary())
                            .child(self.route.title()),
                    )
            })
            .when(self.now_playing_open, |d| d.child(self.now_playing.clone()));

        // The queue panel slides in from the right edge over 200 ms, then stays mounted only
        // while it is open or moving.
        let (openness, moving) = self.queue.level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let queue = (openness > 0.0).then(|| {
            div()
                .id("queue-slot")
                .relative()
                .flex_none()
                .h_full()
                .w(px(layout::QUEUE_PANEL_WIDTH * openness))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .child(self.queue_panel.clone()),
                )
        });

        let bar = PlayerBar {
            player: self.player.clone(),
            view,
            focus: self.bar.clone(),
            progress: self.progress.clone(),
            volume: self.volume.clone(),
            queue_open: self.queue.is_open(),
            on_queue_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| shell.update(cx, |s, cx| s.toggle_queue(cx))
            })),
            now_playing_open: self.now_playing_open,
            on_now_playing_toggle: Rc::new(Box::new(move |_, cx| {
                shell.update(cx, |s, cx| {
                    let open = !s.now_playing_open;
                    s.set_now_playing(open, cx);
                });
            })),
        };

        let fonts = cx.global::<style::Fonts>().text.clone();
        div()
            .id("shell")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.root_focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.player.update(cx, |m, cx| m.toggle_play(cx));
            }))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| {
                this.player.update(cx, |m, cx| m.next(cx));
            }))
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| {
                this.player.update(cx, |m, cx| m.previous(cx));
            }))
            .on_action(cx.listener(|this, _: &CloseNowPlaying, window, cx| {
                if this.now_playing_open {
                    this.set_now_playing(false, cx);
                    this.focus_root(window, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| window.focus_prev(cx)))
            .size_full()
            .flex()
            .flex_col()
            .font_family(fonts)
            .text_size(px(typography::BODY.size_px))
            .text_color(color::text_primary())
            .bg(color::bg_base())
            .child(TitleBar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(nav)
                    .child(page)
                    .children(queue),
            )
            .child(bar)
    }
}
