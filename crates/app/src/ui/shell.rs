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
use crate::explore::ExploreModel;
use crate::home::{Access, HomeModel};
use crate::library::{LibraryEdits, LibraryWrites, Notice, Target};
use crate::page_cache::PageCache;
use crate::lyrics::LyricsModel;
use crate::keymap::{
    LikeCurrent, SeekBack, SeekForward, ToggleLyrics, ToggleMute, VolumeDown, VolumeUp,
    SEEK_STEP_MS, VOLUME_STEP,
};
use crate::motion::PanelSlide;
use crate::os_media::OsCommand;
use crate::player::PlayerModel;
use crate::related::RelatedModel;
use crate::search::{SearchModel, SearchSource};
use crate::settings::{SettingsModel, PERSONAL_SHELVES};
use crate::settings::{LibrarySortStore, LyricsOpenStore, RelatedOpenStore};
use crate::style::{self, color};
use crate::ui::account_page::AccountPage;
use crate::ui::explore_page::{self, ExploreView};
use crate::ui::home_page::{self, HomeView};
use crate::ui::library::{self, LibraryView};
use crate::ui::lyrics_pane::LyricsPane;
use crate::ui::nav::{NavRail, Route};
use crate::ui::now_playing::NowPlaying;
use crate::ui::audio_page::AudioPage;
use crate::ui::choice_menu::{self, ChoiceHandle, ChoiceMenu};
use crate::ui::pane_tabs::{OnPick, Pane, PaneFocus};
use crate::ui::playback_page::PlaybackPage;
use crate::ui::player_bar::{BarFocus, PlayerBar};
use crate::ui::playlist_dialog::{self, DialogHandle, PlaylistDialog};
use crate::ui::playlist_picker::{self, PlaylistPicker};
use crate::ui::queue_panel::QueuePanel;
use crate::ui::related_pane::RelatedPane;
use crate::ui::search_page::{self, SearchPage};
use crate::ui::settings_page::{self, SettingsPage};
use crate::ui::slider::{SliderState, StepBack, StepForward};
use crate::ui::titlebar::TitleBar;
use crate::ui::toast::{self, toast, Fade};
use crate::ui::sleep_menu::{SleepMenu, SleepMenuHandle};
use crate::ui::track_menu::{self, MenuHandle, TrackMenu};
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
    crate::keymap::bind_keys(cx);
    crate::ui::queue_panel::bind_keys(cx);
    crate::ui::text_field::bind_keys(cx);
    crate::ui::card::bind_keys(cx);
    search_page::bind_keys(cx);
    library::bind_keys(cx);
    home_page::bind_keys(cx);
    explore_page::bind_keys(cx);
    playlist_picker::bind_keys(cx);
    track_menu::bind_keys(cx);
    playlist_dialog::bind_keys(cx);
    choice_menu::bind_keys(cx);
    crate::ui::vslider::bind_keys(cx);
    crate::ui::mini_player::bind_keys(cx);
}

/// A toast on screen, possibly on its way out.
#[derive(Clone)]
struct ToastShown {
    notice: Notice,
    leaving: bool,
}

pub struct Shell {
    player: Entity<PlayerModel>,
    account: Entity<AccountModel>,
    settings_page: Entity<SettingsPage>,
    library: Entity<LibraryView>,
    home: Entity<HomeModel>,
    home_page: Entity<HomeView>,
    /// What the account allows Home to do, as of the last account change.
    home_access: Access,
    home_epoch: u64,
    explore: Entity<ExploreModel>,
    explore_page: Entity<ExploreView>,
    /// Likes and playlist adds, and the short error a failed one leaves.
    writes: Entity<LibraryWrites>,
    /// Playlist edits (create, rename, delete, save) and what the account owns.
    edits: Entity<LibraryEdits>,
    picker: Entity<PlaylistPicker>,
    /// The dialog that names, renames and confirms deleting a playlist.
    dialog: Entity<PlaylistDialog>,
    /// The menu a right-click or a row's More button opens.
    menu: Entity<TrackMenu>,
    /// The pick-one popover of the Output device row and the speed button.
    choices: Entity<ChoiceMenu>,
    /// The sleep timer popover the moon button in Now Playing opens.
    sleep_menu: Entity<SleepMenu>,
    /// The error on screen: it outlives the notice by the length of its fade-out.
    toast: Option<ToastShown>,
    /// Where a page opened from another route (a Search card) returns to when Back closes the
    /// last of them. `None` while the pages were opened from the Library itself.
    detail_origin: Option<Route>,
    /// The account epoch the library was last loaded under.
    library_epoch: u64,
    /// Whether the account could serve the library when it was last looked at; the library is
    /// loaded again when it becomes able to (see [`account_usable`]).
    library_usable: bool,
    /// An account was signed in since the shell started: signing out then forgets its recents.
    search_account_live: bool,
    /// The first time the account turns out to be signed out, the window opens on Settings.
    routed_to_sign_in: bool,
    /// Keeps the window-activation hook that dismisses tooltips alive.
    _activation: Option<gpui::Subscription>,
    /// Keeps the recorder of the window's size and position alive.
    _remembered: Option<gpui::Subscription>,
    route: Route,
    nav_expanded: bool,
    nav_toggled: bool,
    queue: PanelSlide,
    queue_panel: Entity<QueuePanel>,
    /// Lyrics of the playing track, and the pane that shows them in Now Playing.
    lyrics: Entity<LyricsModel>,
    lyrics_pane: Entity<LyricsPane>,
    /// Related songs of the playing track, and the pane that shows them in Now Playing.
    related: Entity<RelatedModel>,
    related_pane: Entity<RelatedPane>,
    now_playing_open: bool,
    now_playing: Entity<NowPlaying>,
    root_focus: FocusHandle,
    search: Entity<SearchPage>,
    search_model: Entity<SearchModel>,
    thumbs: Entity<ThumbStore>,
    nav_toggle: FocusHandle,
    nav_items: [FocusHandle; 5],
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
        // Settings that last only for this run (tests, and windows that do not own any).
        let settings = cx.new(|_| SettingsModel::new(None));
        Self::with_settings(player, account, settings, cx)
    }

    pub fn with_settings(
        player: Entity<PlayerModel>,
        account: Entity<AccountModel>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Tab order: nav toggle, nav items, then the player bar left to right.
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        let nav_toggle = stop(cx, 1);
        // Explore shares Search's index: the page's own stops start at 5 and Search's field is one
        // of them, so the rail cannot grow past 5; stops with one index keep the rail's order.
        let nav_items = [
            stop(cx, 2),
            stop(cx, 3),
            stop(cx, 3),
            stop(cx, 4),
            stop(cx, 5),
        ];
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
            like: stop(cx, 26),
            mute: stop(cx, 27),
            volume: stop(cx, 28),
            // Beside the queue button, ahead of it: mini player, then lyrics, then queue.
            mini: stop(cx, 29),
            lyrics: stop(cx, 29),
            queue: stop(cx, 29),
        };
        let progress = cx.new(|_| SliderState::default());
        let volume = cx.new(|_| SliderState::default());
        let root_focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let writes = cx.new(|cx| LibraryWrites::new(player.clone(), cx));
        let edits = cx.new(|_| LibraryEdits::new(player.clone(), writes.clone()));
        let picker = cx.new(|cx| PlaylistPicker::new(player.clone(), writes.clone(), cx));
        let dialog = cx.new(|cx| PlaylistDialog::new(player.clone(), edits.clone(), cx));
        cx.observe(&dialog, |_, _, cx| cx.notify()).detach();
        cx.set_global(DialogHandle(dialog.clone()));
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        let menu = cx.new(|cx| {
            TrackMenu::new(
                player.clone(),
                writes.clone(),
                picker.clone(),
                edits.clone(),
                {
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                },
                {
                    let weak = weak.clone();
                    move |collection, occurrence, cx| {
                        let _ = weak.update(cx, |s, cx| {
                            let model = s.library.read(cx).model().clone();
                            model.update(cx, |m, cx| {
                                m.remove_row(collection.library_key(), occurrence, cx)
                            });
                        });
                    }
                },
                cx,
            )
        });
        cx.observe(&menu, |_, _, cx| cx.notify()).detach();
        cx.set_global(MenuHandle(menu.clone()));
        let choices = cx.new(ChoiceMenu::new);
        cx.observe(&choices, |_, _, cx| cx.notify()).detach();
        cx.set_global(ChoiceHandle(choices.clone()));
        let sleep_menu = cx.new(|cx| SleepMenu::new(player.clone(), cx));
        cx.observe(&sleep_menu, |_, _, cx| cx.notify()).detach();
        cx.set_global(SleepMenuHandle(sleep_menu.clone()));
        let queue_panel = cx.new(|cx| {
            QueuePanel::new(
                player.clone(),
                writes.clone(),
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_search(window, cx));
                    }
                },
                cx,
            )
        });
        let lyrics = cx.new(|cx| LyricsModel::new(player.clone(), cx));
        let lyrics_pane = cx.new(|cx| LyricsPane::new(player.clone(), lyrics.clone(), cx));
        cx.observe(&lyrics_pane, |_, _, cx| cx.notify()).detach();
        let thumbs = cx.new(|_| ThumbStore::new());
        let related = cx.new(|cx| RelatedModel::new(player.clone(), cx));
        let related_pane = cx.new(|cx| {
            RelatedPane::new(
                player.clone(),
                related.clone(),
                thumbs.clone(),
                {
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                },
                cx,
            )
        });
        cx.observe(&related_pane, |_, _, cx| cx.notify()).detach();
        // The Up next · Lyrics · Related control, shared by the three panes.
        let pane_focus = PaneFocus::new(cx);
        let on_pick: OnPick = Rc::new({
            let weak = weak.clone();
            move |pane, _, cx| {
                let _ = weak.update(cx, |s, cx| s.show_pane(pane, cx));
            }
        });
        queue_panel.update(cx, |p, _| p.set_tabs(pane_focus.clone(), on_pick.clone()));
        let now_playing = cx.new(|cx| {
            NowPlaying::new(
                player.clone(),
                writes.clone(),
                picker.clone(),
                lyrics_pane.clone(),
                related_pane.clone(),
                pane_focus.clone(),
                on_pick.clone(),
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
        let playback_page =
            cx.new(|cx| PlaybackPage::new(player.clone(), settings.clone(), cx));
        let audio_page = cx.new(|cx| AudioPage::new(player.clone(), settings.clone(), cx));
        let settings_page = cx.new(|cx| {
            SettingsPage::new(
                account_page,
                playback_page,
                audio_page,
                settings.clone(),
                player.clone(),
                cx,
            )
        });
        let library = cx.new(|cx| {
            LibraryView::new(
                player.clone(),
                writes.clone(),
                picker.clone(),
                edits.clone(),
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
                {
                    let weak = weak.clone();
                    move |_, cx| {
                        let _ = weak.update(cx, |s, cx| s.show_route(Route::Settings, cx));
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
        if let Some(backend) = settings.read(cx).backend() {
            search_model.update(cx, |m, cx| m.install_recents(backend, cx));
        }
        let search = cx.new(|cx| {
            SearchPage::new(
                player.clone(),
                writes.clone(),
                picker.clone(),
                search_model.clone(),
                library.read(cx).model().clone(),
                thumbs.clone(),
                settings.clone(),
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
        let home = cx.new(|cx| HomeModel::new(player.clone(), cx));
        // Home > Personal shelves follows its Settings switch.
        let personal_on = settings.read(cx).get(&PERSONAL_SHELVES);
        home.update(cx, |h, cx| h.set_personal_enabled(personal_on, cx));
        {
            let home = home.clone();
            cx.observe(&settings, move |_, settings, cx| {
                let on = settings.read(cx).get(&PERSONAL_SHELVES);
                home.update(cx, |h, cx| h.set_personal_enabled(on, cx));
            })
            .detach();
        }
        let home_page = cx.new(|cx| {
            HomeView::new(
                player.clone(),
                home.clone(),
                thumbs.clone(),
                {
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                },
                {
                    let weak = weak.clone();
                    move |_, cx| {
                        let _ = weak.update(cx, |s, cx| s.show_route(Route::Settings, cx));
                    }
                },
                cx,
            )
        });
        let explore = cx.new(|cx| ExploreModel::new(player.clone(), cx));
        let explore_page = cx.new(|cx| {
            ExploreView::new(
                player.clone(),
                explore.clone(),
                thumbs.clone(),
                {
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                },
                {
                    let weak = weak.clone();
                    move |_, cx| {
                        let _ = weak.update(cx, |s, cx| s.show_route(Route::Settings, cx));
                    }
                },
                cx,
            )
        });
        let mut shell = Self {
            player,
            account,
            settings_page,
            library,
            home,
            home_page,
            home_access: Access::Waiting,
            home_epoch: 0,
            explore,
            explore_page,
            writes,
            edits,
            picker,
            dialog,
            menu,
            choices,
            sleep_menu,
            toast: None,
            detail_origin: None,
            library_epoch: 0,
            library_usable: false,
            search_account_live: false,
            routed_to_sign_in: false,
            _activation: None,
            _remembered: None,
            route: Route::Home,
            nav_expanded: false,
            nav_toggled: false,
            queue: PanelSlide::default(),
            queue_panel,
            lyrics,
            lyrics_pane,
            related,
            related_pane,
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
        let view = self.account.read(cx).service().view();
        let epoch = view.epoch;
        let usable = account_usable(&view, self.library_usable);
        // The page cache serves one account at a time: before anything below asks it for a page.
        let signed_out_by_choice = self.account.read(cx).is_signed_out_by_choice();
        if let Some(cache) = PageCache::try_global(cx) {
            if signed_out_by_choice {
                cache.sign_out();
            } else {
                cache.set_account(cache_account(&view));
            }
        }
        // A new login imports before the service has confirmed it, so what the library loaded
        // then failed with "sign-in expired": it loads again once the session is confirmed.
        let became_usable = usable && !self.library_usable;
        self.library_usable = usable;
        if epoch != self.library_epoch || became_usable {
            self.library_epoch = epoch;
            self.detail_origin = None;
            self.library.update(cx, |l, cx| l.reset(cx));
            // Likes and the open picker belong to the old account.
            self.writes.update(cx, |w, cx| w.reset(cx));
            self.picker.update(cx, |p, cx| p.reset(cx));
            self.menu.update(cx, |m, cx| m.reset(cx));
            self.edits.update(cx, |e, cx| e.reset(cx));
            self.dialog.update(cx, |d, cx| d.reset(cx));
            self.sleep_menu.update(cx, |m, cx| m.reset(cx));
        }
        let model = self.account.read(cx);
        let (signed_out, by_choice) = (model.is_signed_out(), model.is_signed_out_by_choice());
        if matches!(model.screen(), crate::account::Screen::SignedIn { .. }) {
            self.search_account_live = true;
        } else if by_choice && self.search_account_live {
            // What the account searched for goes with its session.
            self.search_account_live = false;
            self.search_model.update(cx, |m, cx| m.forget_account(cx));
        }
        let access = home_access(&view, signed_out, by_choice, self.home_access);
        if access != self.home_access || epoch != self.home_epoch {
            if epoch != self.home_epoch {
                self.home_page.update(cx, |h, cx| h.rewind(cx));
                self.explore_page.update(cx, |e, cx| e.rewind(cx));
            }
            self.home_access = access;
            self.home_epoch = epoch;
            self.home
                .update(cx, |h, cx| h.set_access(access, epoch, cx));
            self.explore
                .update(cx, |e, cx| e.set_access(access, epoch, cx));
        }
        // Last session's feed is on screen at once, before the session is confirmed.
        self.home.update(cx, |h, cx| h.restore_cached(cx));
        self.library
            .update(cx, |l, cx| l.set_signed_out(by_choice, cx));
        if !self.routed_to_sign_in && signed_out {
            self.routed_to_sign_in = true;
            self.route = Route::Settings;
            self.now_playing_open = false;
        }
        cx.notify();
    }

    pub fn route(&self) -> Route {
        self.route
    }

    /// The shell's own tab stops by name, in tab order: the nav rail, then the player bar. The
    /// page and panels in between own theirs. For the tab-order tests.
    pub fn tab_stops(&self) -> Vec<(&'static str, FocusHandle)> {
        let mut stops = vec![("nav-toggle", self.nav_toggle.clone())];
        for (name, handle) in [
            "nav-home",
            "nav-explore",
            "nav-search",
            "nav-library",
            "nav-settings",
        ]
            .into_iter()
            .zip(&self.nav_items)
        {
            stops.push((name, handle.clone()));
        }
        let bar = &self.bar;
        stops.extend([
            ("bar-now-playing", bar.now_playing.clone()),
            ("bar-shuffle", bar.shuffle.clone()),
            ("bar-previous", bar.previous.clone()),
            ("bar-play", bar.play.clone()),
            ("bar-next", bar.next.clone()),
            ("bar-repeat", bar.repeat.clone()),
            ("bar-progress", bar.progress.clone()),
            ("bar-like", bar.like.clone()),
            ("bar-mute", bar.mute.clone()),
            ("bar-volume", bar.volume.clone()),
            ("bar-mini", bar.mini.clone()),
            ("bar-lyrics", bar.lyrics.clone()),
            ("bar-queue", bar.queue.clone()),
        ]);
        stops
    }

    pub fn library(&self) -> &Entity<LibraryView> {
        &self.library
    }

    pub fn home(&self) -> &Entity<HomeModel> {
        &self.home
    }

    pub fn home_page(&self) -> &Entity<HomeView> {
        &self.home_page
    }

    pub fn explore(&self) -> &Entity<ExploreModel> {
        &self.explore
    }

    pub fn explore_page(&self) -> &Entity<ExploreView> {
        &self.explore_page
    }

    pub fn writes(&self) -> &Entity<LibraryWrites> {
        &self.writes
    }

    pub fn picker(&self) -> &Entity<PlaylistPicker> {
        &self.picker
    }

    /// The app's settings (the model the Settings page edits).
    pub fn settings(&self, cx: &App) -> Entity<SettingsModel> {
        self.settings_page.read(cx).settings().clone()
    }

    pub fn sleep_menu(&self) -> &Entity<SleepMenu> {
        &self.sleep_menu
    }

    pub fn menu(&self) -> &Entity<TrackMenu> {
        &self.menu
    }

    pub fn edits(&self) -> &Entity<LibraryEdits> {
        &self.edits
    }

    pub fn dialog(&self) -> &Entity<PlaylistDialog> {
        &self.dialog
    }

    pub fn choices(&self) -> &Entity<ChoiceMenu> {
        &self.choices
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
        if route == Route::Home {
            self.home.update(cx, |h, cx| h.show(Instant::now(), cx));
        }
        if route == Route::Explore {
            self.explore.update(cx, |e, cx| e.show(Instant::now(), cx));
        }
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
        // Choosing Explore while on it returns to its tiles, whatever mood page is open.
        if route == Route::Explore && self.route == Route::Explore {
            self.explore_page.update(cx, |e, cx| e.back(window, cx));
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
        if origin == Route::Home {
            self.home.update(cx, |h, cx| h.show(Instant::now(), cx));
        }
        if origin == Route::Explore {
            self.explore.update(cx, |e, cx| e.show(Instant::now(), cx));
        }
        if origin == Route::Search {
            self.search.update(cx, |s, cx| s.restore_focus(window, cx));
        } else {
            self.focus_root(window, cx);
        }
        cx.notify();
    }

    pub fn set_now_playing(&mut self, open: bool, cx: &mut Context<Self>) {
        if open && !self.now_playing_open {
            // The lyrics column starts on the current line when the page appears.
            self.lyrics_pane.update(cx, |p, _| p.reset_follow());
        }
        self.now_playing_open = open;
        cx.notify();
    }

    /// Lyrics are on screen: Now Playing is open and lyrics are chosen. They take the right
    /// side of the page, the queue's place, so the queue is hidden meanwhile.
    pub fn lyrics_showing(&self, cx: &App) -> bool {
        self.now_playing_open
            && self.lyrics_pane.read(cx).is_open()
            && !self.related_pane.read(cx).is_open()
    }

    pub fn lyrics_pane(&self) -> &Entity<LyricsPane> {
        &self.lyrics_pane
    }

    pub fn lyrics(&self) -> &Entity<LyricsModel> {
        &self.lyrics
    }

    /// The Related tab is on screen: Now Playing is open and Related is chosen. It takes the
    /// right side of the page like lyrics do, so the queue is hidden meanwhile.
    pub fn related_showing(&self, cx: &App) -> bool {
        self.now_playing_open && self.related_pane.read(cx).is_open()
    }

    pub fn related_pane(&self) -> &Entity<RelatedPane> {
        &self.related_pane
    }

    pub fn related(&self) -> &Entity<RelatedModel> {
        &self.related
    }

    /// What Now Playing's right side shows (the queue when nothing else is chosen).
    pub fn pane(&self, cx: &App) -> Pane {
        if self.related_showing(cx) {
            Pane::Related
        } else if self.lyrics_showing(cx) {
            Pane::Lyrics
        } else {
            Pane::UpNext
        }
    }

    /// The Lyrics button: shows lyrics (opening Now Playing if need be), or hides them.
    pub fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.lyrics_showing(cx) {
            self.lyrics_pane.update(cx, |p, cx| p.set_open(false, cx));
        } else {
            self.show_pane(Pane::Lyrics, cx);
        }
        cx.notify();
    }

    /// The pane control: shows the queue, the lyrics or the related songs on Now Playing's right
    /// side (opening Now Playing for the last two). The choice is remembered.
    pub fn show_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.lyrics_pane
            .update(cx, |p, cx| p.set_open(pane == Pane::Lyrics, cx));
        self.related_pane
            .update(cx, |p, cx| p.set_open(pane == Pane::Related, cx));
        match pane {
            Pane::UpNext => {
                if !self.queue.is_open() {
                    self.queue.set(true, Instant::now(), !cx.reduce_motion());
                    self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
                }
            }
            Pane::Lyrics | Pane::Related => self.set_now_playing(true, cx),
        }
        cx.notify();
    }

    /// The lookup is ready, and where the Lyrics choice is remembered: lyrics start loading and
    /// the saved choice is applied.
    pub fn install_lyrics(
        &mut self,
        service: Arc<apollo_lyrics::LyricsService>,
        store: Option<Arc<dyn LyricsOpenStore>>,
        cx: &mut Context<Self>,
    ) {
        self.lyrics.update(cx, |l, cx| l.install(service, cx));
        self.lyrics_pane
            .update(cx, |p, _| p.set_store(store.clone()));
        let Some(store) = store else { return };
        let pane = self.lyrics_pane.downgrade();
        cx.spawn(async move |_, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { store.load() })
                .await;
            let _ = pane.update(cx, |p, cx| p.restore_open(saved, cx));
        })
        .detach();
    }

    /// Where the Related choice is remembered, and the saved choice applied.
    pub fn install_related(
        &mut self,
        store: Option<Arc<dyn RelatedOpenStore>>,
        cx: &mut Context<Self>,
    ) {
        self.related_pane
            .update(cx, |p, _| p.set_store(store.clone()));
        let Some(store) = store else { return };
        let pane = self.related_pane.downgrade();
        let lyrics = self.lyrics_pane.downgrade();
        cx.spawn(async move |_, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { store.load() })
                .await;
            // Lyrics and Related are exclusive: if both were saved on, Related wins.
            if saved == Some(true) {
                let _ = lyrics.update(cx, |p, cx| p.restore_open(Some(false), cx));
            }
            let _ = pane.update(cx, |p, cx| p.restore_open(saved, cx));
        })
        .detach();
    }

    /// Where the Library tabs' sort choices are remembered.
    pub fn install_library_sort(
        &mut self,
        store: Option<Arc<dyn LibrarySortStore>>,
        cx: &mut Context<Self>,
    ) {
        let model = self.library.read(cx).model().clone();
        model.update(cx, |m, cx| m.install_sort_store(store, cx));
    }

    /// The L key: like the playing song, or take the like back (the heart's own toggle).
    fn like_current(&mut self, cx: &mut Context<Self>) {
        if let Some(video) = self.player.read(cx).current_video() {
            self.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
        }
    }

    pub fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        if self.lyrics_showing(cx) || self.related_showing(cx) {
            // The queue asked for the side the lyrics or related songs are on: they give way.
            self.lyrics_pane.update(cx, |p, cx| p.set_open(false, cx));
            self.related_pane.update(cx, |p, cx| p.set_open(false, cx));
            if !self.queue.is_open() {
                self.queue.set(true, Instant::now(), !cx.reduce_motion());
                self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
            }
            cx.notify();
            return;
        }
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

    /// Hooks that need the window: tooltips go away when it loses focus.
    /// Keeps `subscription` (the window's position recorder) alive as long as the shell.
    pub fn keep(&mut self, subscription: Option<gpui::Subscription>) {
        self._remembered = subscription;
    }

    pub fn install_window_hooks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._activation = Some(cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                crate::ui::tooltip::dismiss_all(cx);
            }
        }));
    }

    /// Follow the writes' notice: a new one shows at once, a cleared one stays for the length of
    /// its fade-out (not at all under reduced motion).
    fn sync_toast(&mut self, animate: bool, cx: &mut Context<Self>) {
        match self.writes.read(cx).notice().cloned() {
            Some(notice) => {
                self.toast = Some(ToastShown {
                    notice,
                    leaving: false,
                });
            }
            None => {
                let Some(shown) = self.toast.as_mut().filter(|t| !t.leaving) else {
                    return;
                };
                if !animate {
                    self.toast = None;
                    return;
                }
                shown.leaving = true;
                let id = shown.notice.id;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(toast::FADE).await;
                    this.update(cx, |this, cx| {
                        let gone = this
                            .toast
                            .as_ref()
                            .is_some_and(|t| t.leaving && t.notice.id == id);
                        if gone {
                            this.toast = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Put keyboard focus on the root so shortcuts work before anything is clicked.
    pub fn focus_root(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.root_focus, cx);
    }
}

/// Whether the account can serve library requests: a confirmed session with a chosen identity.
/// A re-check in progress or a network failure says nothing new, so `previous` carries over.
fn account_usable(view: &apollo_account::service::AccountView, previous: bool) -> bool {
    use apollo_account::service::{IdentityState, SessionState};
    match view.session {
        SessionState::Valid => matches!(view.identity, IdentityState::Selected(_)),
        SessionState::Absent | SessionState::Expired => false,
        SessionState::Validating | SessionState::Unreachable => previous,
    }
}

/// The key the page cache files an account's pages under: the chosen channel, the same
/// identity the saved queue is bound to. `None` while none is chosen.
fn cache_account(view: &apollo_account::service::AccountView) -> Option<String> {
    match &view.identity {
        apollo_account::service::IdentityState::Selected(identity) => Some(identity.key()),
        _ => None,
    }
}

/// What Home may do for the account. A confirmed session with a chosen channel, or one that
/// could not be checked (offline), asks for the feed and shows its own failure; a re-check in
/// progress changes nothing once the feed is open.
fn home_access(
    view: &apollo_account::service::AccountView,
    signed_out: bool,
    by_choice: bool,
    previous: Access,
) -> Access {
    use apollo_account::service::{IdentityState, SessionState};
    if by_choice {
        return Access::SignedOut;
    }
    if signed_out {
        // Nobody is signed in and the session did not simply run out: it did.
        return Access::Expired;
    }
    match view.session {
        SessionState::Valid => match view.identity {
            IdentityState::Selected(_) => Access::Open,
            _ => Access::Waiting,
        },
        SessionState::Expired => Access::Expired,
        SessionState::Unreachable => Access::Open,
        SessionState::Absent | SessionState::Validating => {
            if previous == Access::Open {
                Access::Open
            } else {
                Access::Waiting
            }
        }
    }
}

/// A zero-size element whose paint registers window-wide listeners (capture phase, so no
/// child can swallow the event): scrolling, and a press or release (a drag starts and ends
/// with those), send every tooltip away. See `tooltip::dismiss_all`.
fn dismiss_tooltips_on_input() -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|_: &gpui::ScrollWheelEvent, phase, window, cx| {
                if phase.capture() {
                    crate::ui::tooltip::dismiss_all(cx);
                    window.refresh();
                }
            });
            window.on_mouse_event(|_: &gpui::MouseDownEvent, phase, window, cx| {
                if phase.capture() {
                    crate::ui::tooltip::dismiss_all(cx);
                    window.refresh();
                }
            });
            window.on_mouse_event(|_: &gpui::MouseUpEvent, phase, window, cx| {
                if phase.capture() {
                    crate::ui::tooltip::dismiss_all(cx);
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .size_0()
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
            let title = style::text_style(div(), typography::PAGE_TITLE, cx)
                .text_color(color::text_primary())
                .child(self.route.title());
            let page = page
                .overflow_y_scroll()
                .px(px(spacing::S8))
                .pt(px(spacing::S6));
            if self.route == Route::Settings {
                // One column, title included, no wider than the settings rows read well.
                page.child(
                    div()
                        .w_full()
                        .max_w(px(settings_page::MAX_WIDTH))
                        .child(title)
                        .child(self.settings_page.clone()),
                )
            } else {
                page.child(title)
            }
        };

        // The queue panel slides in from the right edge over 200 ms, then stays mounted only
        // while it is open or moving.
        let (openness, moving) = self.queue.level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let lyrics_showing = self.lyrics_showing(cx);
        let related_showing = self.related_showing(cx);
        // Lyrics and Related take the queue's side of Now Playing; the pane control sits over the
        // queue only while Now Playing is open. Related loads only while it is on screen.
        let other_pane = lyrics_showing || related_showing;
        self.related
            .update(cx, |m, cx| m.set_active(related_showing, cx));
        let tabs_visible = self.now_playing_open;
        self.queue_panel
            .update(cx, |p, cx| p.set_tabs_visible(tabs_visible, cx));
        let queue = (openness > 0.0 && !other_pane).then(|| {
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
            Route::Home if !self.now_playing_open => self.home_page.clone().into_any_element(),
            Route::Explore if !self.now_playing_open => {
                self.explore_page.clone().into_any_element()
            }
            _ => page.into_any_element(),
        };

        let playing = self.player.read(cx).current_video();
        let liked = playing
            .as_deref()
            .is_some_and(|id| self.writes.read(cx).is_liked(id));
        let bar = PlayerBar {
            player: self.player.clone(),
            view,
            focus: self.bar.clone(),
            progress: self.progress.clone(),
            volume: self.volume.clone(),
            queue_open: self.queue.is_open() && !other_pane,
            lyrics_open: lyrics_showing,
            on_lyrics_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| shell.update(cx, |s, cx| s.toggle_lyrics(cx))
            })),
            on_mini_player: Rc::new(Box::new(|_, cx| crate::mini::open(cx))),
            on_queue_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| shell.update(cx, |s, cx| s.toggle_queue(cx))
            })),
            now_playing_open: self.now_playing_open,
            on_now_playing_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| {
                    shell.update(cx, |s, cx| {
                        let open = !s.now_playing_open;
                        s.set_now_playing(open, cx);
                    });
                }
            })),
            liked,
            on_like: Rc::new(Box::new({
                let writes = self.writes.clone();
                move |_, cx| {
                    if let Some(video) = &playing {
                        writes.update(cx, |w, cx| w.toggle_like(video, cx));
                    }
                }
            })),
        };

        // A failed library write leaves a short error above the player bar.
        let animate = !cx.reduce_motion();
        self.sync_toast(animate, cx);
        let toast = self.toast.clone().map(|shown| {
            let writes = self.writes.clone();
            toast(
                &shown.notice,
                if shown.leaving { Fade::Out } else { Fade::In },
                animate,
                move |cx| writes.update(cx, |w, cx| w.dismiss_notice(cx)),
                cx,
            )
        });
        let picker = self.picker.read(cx).is_open().then(|| self.picker.clone());
        let menu = self.menu.read(cx).is_open().then(|| self.menu.clone());
        let dialog = self.dialog.read(cx).is_open().then(|| self.dialog.clone());
        let choices = self.choices.read(cx).is_open().then(|| self.choices.clone());
        let sleep_menu = self
            .sleep_menu
            .read(cx)
            .is_open()
            .then(|| self.sleep_menu.clone());

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
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| {
                this.player.update(cx, |m, cx| m.toggle_mute(cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| {
                this.player.update(cx, |m, cx| m.nudge_volume(VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| {
                this.player.update(cx, |m, cx| m.nudge_volume(-VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &SeekBack, _, cx| {
                this.player
                    .update(cx, |m, cx| m.os_command(OsCommand::SeekBy(-SEEK_STEP_MS), cx));
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.player
                    .update(cx, |m, cx| m.os_command(OsCommand::SeekBy(SEEK_STEP_MS), cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_lyrics(cx)))
            .on_action(cx.listener(|this, _: &LikeCurrent, _, cx| this.like_current(cx)))
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| window.focus_prev(cx)))
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.open_search(window, cx)),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(fonts)
            .text_size(px(typography::BODY.size_px))
            .text_color(color::text_primary())
            .bg(color::bg_base())
            .child(dismiss_tooltips_on_input())
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
            .children(toast)
            .children(picker)
            .children(menu)
            .children(dialog)
            .children(choices)
            .children(sleep_menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_account::service::{AccountView, IdentityState, SessionState};

    fn view(session: SessionState, identity: IdentityState) -> AccountView {
        AccountView {
            session,
            identity,
            premium: apollo_account::service::PremiumEvidence::Undeclared,
            storage: apollo_account::service::StorageMode::Durable,
            problem: apollo_account::service::Problem::None,
            epoch: 0,
            jar_is_seed: false,
        }
    }

    #[test]
    fn only_a_confirmed_session_with_an_identity_can_serve_the_library() {
        let who = apollo_account::identity::Identity {
            name: "N".into(),
            handle: None,
            avatar_url: None,
            page_id: None,
            selected_by_service: true,
        };
        let selected = IdentityState::Selected(who);
        assert!(account_usable(
            &view(SessionState::Valid, selected.clone()),
            false
        ));
        assert!(!account_usable(
            &view(SessionState::Valid, IdentityState::None),
            true
        ));
        assert!(!account_usable(
            &view(SessionState::Expired, selected.clone()),
            true
        ));
        assert!(!account_usable(
            &view(SessionState::Absent, IdentityState::None),
            true
        ));
        // A re-check or a lost connection changes nothing either way.
        for previous in [true, false] {
            assert_eq!(
                account_usable(&view(SessionState::Validating, selected.clone()), previous),
                previous
            );
            assert_eq!(
                account_usable(&view(SessionState::Unreachable, selected.clone()), previous),
                previous
            );
        }
    }
}
