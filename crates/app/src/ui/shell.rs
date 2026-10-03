//! The window's root view: title bar, nav rail, page and player bar.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_theme::tokens::{layout, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, Window,
};

use crate::account::AccountModel;
use crate::library::Target;
use crate::motion::PanelSlide;
use crate::player::PlayerModel;
use crate::search::{SearchModel, SearchSource};
use crate::style::{self, color};
use crate::ui::account_page::AccountPage;
use crate::ui::library::{self, LibraryView};
use crate::ui::nav::{NavRail, Route};
use crate::ui::now_playing::NowPlaying;
use crate::ui::player_bar::{BarFocus, PlayerBar};
use crate::ui::queue_panel::QueuePanel;
use crate::ui::search_page::{self, SearchPage};
use crate::ui::slider::{SliderState, StepBack, StepForward};
use crate::ui::titlebar::TitleBar;
use crate::url_thumbs::{ThumbSource, ThumbStore};

actions!(
    apollo,
    [
        TogglePlay,
        NextTrack,
        PreviousTrack,
        CloseNowPlaying,
        FocusNextControl,
        FocusPreviousControl,
        FocusSearch,
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
        KeyBinding::new("ctrl-k", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-f", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("left", StepBack, Some(crate::ui::slider::KEY_CONTEXT)),
        KeyBinding::new("right", StepForward, Some(crate::ui::slider::KEY_CONTEXT)),
    ]);
    crate::ui::queue_panel::bind_keys(cx);
    crate::ui::text_field::bind_keys(cx);
    crate::ui::card::bind_keys(cx);
    search_page::bind_keys(cx);
    library::bind_keys(cx);
}

pub struct Shell {
    player: Entity<PlayerModel>,
    account: Entity<AccountModel>,
    account_page: Entity<AccountPage>,
    library: Entity<LibraryView>,
    /// Where a page opened from another route (a Search card) returns to when Back closes the
    /// last of them. `None` while the pages were opened from the Library itself.
    detail_origin: Option<Route>,
    /// The account epoch the library was last loaded under.
    library_epoch: u64,
    /// The first time the account turns out to be signed out, the window opens on Settings.
    routed_to_sign_in: bool,
    route: Route,
    nav_expanded: bool,
    nav_toggled: bool,
    queue: PanelSlide,
    queue_panel: Entity<QueuePanel>,
    now_playing_open: bool,
    now_playing: Entity<NowPlaying>,
    root_focus: FocusHandle,
    search: Entity<SearchPage>,
    search_model: Entity<SearchModel>,
    thumbs: Entity<ThumbStore>,
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
        // Nav 1-5, Now Playing 6-12, queue panel 13-17, then the player bar.
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
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_search(window, cx));
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
        let account_page = cx.new(|cx| AccountPage::new(account.clone(), player.clone(), cx));
        let library = cx.new(|cx| {
            LibraryView::new(
                player.clone(),
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_search(window, cx));
                    }
                },
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.pages_closed(window, cx));
                    }
                },
                cx,
            )
        });
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&account, |this, _, cx| this.account_changed(cx))
            .detach();
        cx.observe(&progress, |_, _, cx| cx.notify()).detach();
        cx.observe(&volume, |_, _, cx| cx.notify()).detach();
        let search_model = cx.new(|_| SearchModel::new());
        let thumbs = cx.new(|_| ThumbStore::new());
        let search = cx.new(|cx| {
            SearchPage::new(
                player.clone(),
                search_model.clone(),
                thumbs.clone(),
                root_focus.clone(),
                {
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                },
                cx,
            )
        });
        let mut shell = Self {
            player,
            account,
            account_page,
            library,
            detail_origin: None,
            library_epoch: 0,
            routed_to_sign_in: false,
            route: Route::Home,
            nav_expanded: false,
            nav_toggled: false,
            queue: PanelSlide::default(),
            queue_panel,
            now_playing_open: false,
            now_playing,
            root_focus,
            search,
            search_model,
            thumbs,
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
        // A different login, identity or sign-out: nothing listed under the old one stays.
        let epoch = self.account.read(cx).service().view().epoch;
        if epoch != self.library_epoch {
            self.library_epoch = epoch;
            self.detail_origin = None;
            self.library.update(cx, |l, cx| l.reset(cx));
        }
        if !self.routed_to_sign_in && self.account.read(cx).is_signed_out() {
            self.routed_to_sign_in = true;
            self.route = Route::Settings;
            self.now_playing_open = false;
        }
        cx.notify();
    }

    pub fn route(&self) -> Route {
        self.route
    }

    pub fn library(&self) -> &Entity<LibraryView> {
        &self.library
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

    /// Switch page (what the nav rail and the empty states' actions do).
    pub fn show_route(&mut self, route: Route, cx: &mut Context<Self>) {
        self.leave_opened_pages(cx);
        self.route = route;
        self.now_playing_open = false;
        cx.notify();
    }

    /// The nav item that is lit: pages opened from Search sit on the Library page but keep
    /// Search lit.
    pub fn nav_route(&self) -> Route {
        match self.detail_origin {
            Some(origin) if self.route == Route::Library => origin,
            _ => self.route,
        }
    }

    /// What choosing an item of the nav rail does.
    pub fn choose_route(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        // Choosing Library while on it returns to its lists, whatever is open.
        if route == Route::Library && self.route == Route::Library {
            self.detail_origin = None;
            self.library.update(cx, |l, cx| l.go_home(window, cx));
        }
        if route == Route::Search {
            self.open_search(window, cx);
        } else {
            self.show_route(route, cx);
        }
    }

    /// Where Back from the open pages will land when they were opened from another route.
    pub fn detail_origin(&self) -> Option<Route> {
        self.detail_origin
    }

    /// Show an album, artist or playlist page over the current route's place: the Library page
    /// hosts it, the nav keeps the route it was opened from lit, and Back returns there.
    pub fn open_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.route != Route::Library {
            self.detail_origin = Some(self.route);
        }
        self.route = Route::Library;
        self.now_playing_open = false;
        self.library
            .update(cx, |l, cx| l.open_target(target, window, cx));
        cx.notify();
    }

    /// Navigating away from pages that were opened from another route closes them: they have no
    /// place of their own in the nav. Pages opened from the Library itself stay, as they always
    /// have.
    fn leave_opened_pages(&mut self, cx: &mut Context<Self>) {
        if self.detail_origin.take().is_some() {
            self.library.update(cx, |l, cx| l.close_all_pages(cx));
        }
    }

    /// Back closed the last open page: return to the route it was opened from, if any.
    fn pages_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(origin) = self.detail_origin.take() else {
            return;
        };
        self.route = origin;
        if origin == Route::Search {
            self.search.update(cx, |s, cx| s.restore_focus(window, cx));
        } else {
            self.focus_root(window, cx);
        }
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

    /// The backend is ready: search and artwork start answering (a query typed earlier goes out).
    pub fn install_search(
        &mut self,
        search: Arc<dyn SearchSource>,
        thumbs: Arc<dyn ThumbSource>,
        cx: &mut Context<Self>,
    ) {
        self.search_model.update(cx, |m, cx| m.install(search, cx));
        self.thumbs.update(cx, |t, cx| t.install(thumbs, cx));
    }

    pub fn search_page(&self) -> &Entity<SearchPage> {
        &self.search
    }

    pub fn search_model(&self) -> &Entity<SearchModel> {
        &self.search_model
    }

    /// Show Search with the caret in its field.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_opened_pages(cx);
        self.route = Route::Search;
        self.now_playing_open = false;
        let field = self.search.read(cx).field().clone();
        crate::ui::text_field::focus(&field, window, cx);
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

        let nav_route = self.nav_route();
        let nav = NavRail::new(
            self.nav_expanded,
            self.nav_toggled,
            nav_route,
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
            move |route, window, cx| {
                shell.update(cx, |s, cx| s.choose_route(route, window, cx));
            }
        });

        let page = div().id("page").flex_1().min_w_0().h_full();
        let page = if self.now_playing_open {
            page.child(self.now_playing.clone())
        } else if self.route == Route::Library {
            // The library's lists scroll themselves (virtualized), so the page does not.
            page.px(px(spacing::S8))
                .pt(px(spacing::S6))
                .child(self.library.clone())
        } else {
            page.overflow_y_scroll()
                .px(px(spacing::S8))
                .pt(px(spacing::S6))
                .child(
                    style::text_style(div(), typography::PAGE_TITLE, cx)
                        .text_color(color::text_primary())
                        .child(self.route.title()),
                )
                .when(self.route == Route::Settings, |page| {
                    page.child(self.account_page.clone())
                })
        };

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

        let page = match self.route {
            Route::Search if !self.now_playing_open => self.search.clone().into_any_element(),
            _ => page.into_any_element(),
        };

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
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.open_search(window, cx)),
            )
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
