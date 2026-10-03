//! The window's root view: title bar, nav rail, page and player bar.

use std::rc::Rc;

use apollo_theme::tokens::{spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, Window,
};

use crate::account::AccountModel;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::account_page::AccountPage;
use crate::ui::nav::{NavRail, Route};
use crate::ui::player_bar::{BarFocus, PlayerBar};
use crate::ui::slider::{SliderState, StepBack, StepForward};
use crate::ui::titlebar::TitleBar;

actions!(
    apollo,
    [
        TogglePlay,
        NextTrack,
        PreviousTrack,
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
        KeyBinding::new("tab", FocusNextControl, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(KEY_CONTEXT)),
        KeyBinding::new("left", StepBack, Some(crate::ui::slider::KEY_CONTEXT)),
        KeyBinding::new("right", StepForward, Some(crate::ui::slider::KEY_CONTEXT)),
    ]);
}

pub struct Shell {
    player: Entity<PlayerModel>,
    account: Entity<AccountModel>,
    account_page: Entity<AccountPage>,
    /// The first time the account turns out to be signed out, the window opens on Settings.
    routed_to_sign_in: bool,
    route: Route,
    nav_expanded: bool,
    nav_toggled: bool,
    queue_open: bool,
    root_focus: FocusHandle,
    nav_toggle: FocusHandle,
    nav_items: [FocusHandle; 4],
    bar: BarFocus,
    progress: Entity<SliderState>,
    volume: Entity<SliderState>,
}

impl Shell {
    /// A shell over the simulated account, already signed in (windows that only care about the
    /// player, and tests).
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        let ports = crate::backend::account::ports();
        crate::backend::account::start(&ports, true);
        let account = cx.new(|cx| AccountModel::new(ports, cx));
        account.update(cx, |m, cx| m.startup_finished(cx));
        Self::with_account(player, account, cx)
    }

    pub fn with_account(
        player: Entity<PlayerModel>,
        account: Entity<AccountModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Tab order: nav toggle, nav items, then the player bar left to right.
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        let nav_toggle = stop(cx, 1);
        let nav_items = [stop(cx, 2), stop(cx, 3), stop(cx, 4), stop(cx, 5)];
        let bar = BarFocus {
            shuffle: stop(cx, 10),
            previous: stop(cx, 11),
            play: stop(cx, 12),
            next: stop(cx, 13),
            repeat: stop(cx, 14),
            progress: stop(cx, 15),
            retry: stop(cx, 15),
            mute: stop(cx, 16),
            volume: stop(cx, 17),
            queue: stop(cx, 18),
        };
        let progress = cx.new(|_| SliderState::default());
        let volume = cx.new(|_| SliderState::default());
        let account_page = cx.new(|cx| AccountPage::new(account.clone(), player.clone(), cx));
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&account, |this, _, cx| this.account_changed(cx))
            .detach();
        cx.observe(&progress, |_, _, cx| cx.notify()).detach();
        cx.observe(&volume, |_, _, cx| cx.notify()).detach();
        let mut shell = Self {
            player,
            account,
            account_page,
            routed_to_sign_in: false,
            route: Route::Home,
            nav_expanded: false,
            nav_toggled: false,
            queue_open: false,
            root_focus: cx.focus_handle(),
            nav_toggle,
            nav_items,
            bar,
            progress,
            volume,
        };
        shell.account_changed(cx);
        shell
    }

    /// Opens Settings once if the startup check ends with nobody signed in.
    fn account_changed(&mut self, cx: &mut Context<Self>) {
        if !self.routed_to_sign_in && self.account.read(cx).is_signed_out() {
            self.routed_to_sign_in = true;
            self.route = Route::Settings;
        }
        cx.notify();
    }

    pub fn route(&self) -> Route {
        self.route
    }

    pub fn nav_expanded(&self) -> bool {
        self.nav_expanded
    }

    pub fn queue_open(&self) -> bool {
        self.queue_open
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
        .attention(self.account.read(cx).attention())
        .on_route({
            let shell = shell.clone();
            move |route, _, cx| {
                shell.update(cx, |s, cx| {
                    s.route = route;
                    cx.notify();
                });
            }
        });

        let page = div()
            .id("page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(spacing::S8))
            .pt(px(spacing::S6))
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .text_color(color::text_primary())
                    .child(self.route.title()),
            )
            .when(self.route == Route::Settings, |page| {
                page.child(self.account_page.clone())
            });

        let bar = PlayerBar {
            player: self.player.clone(),
            view,
            focus: self.bar.clone(),
            progress: self.progress.clone(),
            volume: self.volume.clone(),
            queue_open: self.queue_open,
            on_queue_toggle: Rc::new(Box::new(move |_, cx| {
                shell.update(cx, |s, cx| {
                    s.queue_open = !s.queue_open;
                    cx.notify();
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
                    .child(page),
            )
            .child(bar)
    }
}
