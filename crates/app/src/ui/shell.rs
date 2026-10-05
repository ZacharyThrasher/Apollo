//! The window's root view: title bar, nav rail, page and player bar.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_theme::tokens::{layout, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Pixels, Point, Render, ScrollHandle, Styled,
    Window,
};

use crate::account::AccountModel;
use crate::blocked_artists::{BlockHandle, BlockModel};
use crate::crate_list::{CrateHandle, CrateModel};
use crate::recent::{RecentHandle, RecentModel};
use crate::explore::ExploreModel;
use crate::home::{Access, HomeModel};
use crate::keymap::{
    AddCurrentToPlaylist, AddCurrentToTarget, CrateCurrent, GoBack, GoForward, LikeCurrent,
    NavigateDown, NavigateLeft, NavigateRight, NavigateUp, RedoQueue, SeekBack, SeekForward,
    ToggleCrateDock, ShowShortcuts, SkimCancel, SkimCrate, SkimKeep, ToggleFullscreen, ToggleLyrics,
    ToggleMute,
    UndoQueue, VolumeDown,
    VolumeUp, SEEK_STEP_MS, VOLUME_STEP,
};
use crate::library::model::Arrange;
use crate::library::{LibraryEdits, LibraryWrites, Notice, Target};
use crate::lyrics::LyricsModel;
use crate::motion::PanelSlide;
use crate::os_media::OsCommand;
use crate::page_cache::PageCache;
use crate::player::PlayerModel;
use crate::related::RelatedModel;
use crate::search::{SearchModel, SearchSource};
use crate::settings::{SettingsModel, MOOD_CHIPS, MORE_LIKE_THIS, PERSONAL_SHELVES};
use crate::settings::{LibrarySortStore, LyricsOpenStore, RelatedOpenStore, SettingsBackend};
use crate::style::{self, color};
use crate::assets::Icon;
use crate::ui::account_page::AccountPage;
use crate::ui::button::IconButton;
use crate::ui::crate_page::{self, CratePage};
use crate::ui::dock_tabs::{Dock, DockFocus};
use crate::ui::explore_page::{self, ExploreView};
use crate::ui::home_page::{self, HomeView};
use crate::ui::library::{self, LibraryView};
use crate::ui::lyrics_pane::LyricsPane;
use crate::ui::nav::{NavRail, Route};
use crate::ui::nav_history::{HistoryEntry, NavHistory, Place, PlaceState};
use crate::ui::now_playing::NowPlaying;
use crate::ui::audio_page::AudioPage;
use crate::ui::data_page::DataPage;
use crate::ui::choice_menu::{self, ChoiceHandle, ChoiceMenu};
use crate::ui::pane_tabs::{OnPick, Pane, PaneFocus};
use crate::ui::playback_page::PlaybackPage;
use crate::ui::player_bar::{BarFocus, PlayerBar};
use crate::ui::blocked_dialog::{self, BlockedDialog, BlockedDialogHandle};
use crate::ui::duplicates_dialog::{self, DuplicatesDialog, DuplicatesDialogHandle};
use crate::ui::cheat_sheet::{self, CheatSheet, CheatSheetHandle};
use crate::ui::playlist_dialog::{self, DialogHandle, PlaylistDialog};
use crate::target_playlist::{TargetHandle, TargetModel};
use crate::ui::playlist_picker::{self, PickerHandle, PickerSong, PlaylistPicker};
use crate::ui::queue_panel::QueuePanel;
use crate::ui::related_pane::RelatedPane;
use crate::ui::search_page::{self, SearchPage};
use crate::ui::settings_page::{self, SettingsPage};
use crate::ui::sleep_menu::{SleepMenu, SleepMenuHandle};
use crate::ui::slider::{SliderState, StepBack, StepForward};
use crate::ui::titlebar::TitleBar;
use crate::ui::toast::{self, toast, Fade};
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
        /// Left, from a list or grid that has nothing further left: onto the nav rail.
        FocusSidebar,
        Quit
    ]
);

/// How long Ctrl is held before the key badges show.
pub const BADGE_AFTER: std::time::Duration = std::time::Duration::from_millis(600);

/// Key context of the root view; global shortcuts bind here.
pub const KEY_CONTEXT: &str = "Shell";
/// Where the docked Crate's open state is kept.
const KEY_CRATE_DOCK: &str = "crate_dock_open";
/// Below this window width the docked Crate overlays the page instead of squeezing it.
pub const DOCK_OVERLAY_BELOW: f32 = 1100.0;
/// The root's context while Skim runs: the shell's, plus `Skimming`.
const SKIMMING_CONTEXT: &str = "Shell Skimming";

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some(KEY_CONTEXT)),
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
    crate_page::bind_keys(cx);
    crate::ui::recent_list::bind_keys(cx);
    playlist_picker::bind_keys(cx);
    track_menu::bind_keys(cx);
    playlist_dialog::bind_keys(cx);
    blocked_dialog::bind_keys(cx);
    duplicates_dialog::bind_keys(cx);
    cheat_sheet::bind_keys(cx);
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

/// The undo pill on screen, possibly on its way out.
#[derive(Clone)]
struct PillShown {
    id: u64,
    text: gpui::SharedString,
    leaving: bool,
    /// A one-time shortcut tip: an add's confirmation does not push it off the screen.
    tip: bool,
}

/// A scroll position waiting for its page's content to be there to scroll.
struct PendingRestore {
    place: Place,
    scroll: Option<Point<Pixels>>,
    /// Frames spent trying.
    tries: u8,
}

/// Side of the title bar's Back and Forward buttons.
const HISTORY_BUTTON: f32 = 28.0;

/// How many frames a returned-to page gets to grow tall enough for its old scroll position.
const RESTORE_TRIES: u8 = 24;

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
    /// The list of blocked artists, with Unblock.
    blocked_dialog: Entity<BlockedDialog>,
    /// The preview that Remove duplicates asks for.
    duplicates_dialog: Entity<DuplicatesDialog>,
    /// The `?` overlay listing every shortcut.
    cheat_sheet: Entity<CheatSheet>,
    /// Ctrl is held (and nothing else); the key badges show once it has been for a moment.
    ctrl_held: bool,
    /// Bumped whenever the hold starts or ends, so a stale timer does nothing.
    badge_gen: u64,
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
    /// The places the window has shown, for Back and Forward.
    history: NavHistory,
    /// A page just returned to by Back or Forward whose scroll position is still being put back
    /// (its content may not be there on the first frame).
    pending_restore: Option<PendingRestore>,
    /// Where Settings is scrolled (the only page the shell scrolls itself).
    settings_scroll: ScrollHandle,
    /// The Back and Forward buttons at the left of the title bar.
    history_back: FocusHandle,
    history_forward: FocusHandle,
    /// The account epoch the library was last loaded under.
    library_epoch: u64,
    /// Whether the account could serve the library when it was last looked at; the library is
    /// loaded again when it becomes able to (see [`account_usable`]).
    library_usable: bool,
    /// An account was signed in since the shell started: signing out then forgets its recents.
    search_account_live: bool,
    /// The Crate: songs set aside, and the page that lists them.
    crate_model: Entity<CrateModel>,
    /// The artists the owner said not to play from radio, autoplay and the personal shelves.
    block_model: Entity<BlockModel>,
    target_model: Entity<TargetModel>,
    crate_page: Entity<CratePage>,
    /// What was played lately, for the Crate page's section and the dock's Recent tab.
    recent_model: Entity<RecentModel>,
    /// "Queue restored", after Ctrl+Z / Ctrl+Shift+Z.
    pill: Option<PillShown>,
    pill_seq: u64,
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
    /// The Crate docked on the right of the main window. It shares that slot with the queue
    /// (never both open) and is remembered across restarts.
    crate_dock: PanelSlide,
    crate_dock_panel: Entity<CratePage>,
    crate_dock_store: Option<Arc<dyn SettingsBackend>>,
    /// The Queue · Crate control's stops (shared with the queue panel).
    dock_focus: DockFocus,
    /// Where the keyboard was when Ctrl+Shift+C jumped into the docked Crate; Esc goes back.
    dock_return: Option<FocusHandle>,
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
    nav_items: [FocusHandle; 6],
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
        // The title bar's Back and Forward are not tab stops (they have keys).
        let history_back = cx.focus_handle();
        let history_forward = cx.focus_handle();
        // Explore shares Search's index: the page's own stops start at 5 and Search's field is one
        // of them, so the rail cannot grow past 5; stops with one index keep the rail's order.
        let nav_items = [
            stop(cx, 2),
            stop(cx, 3),
            stop(cx, 3),
            stop(cx, 4),
            // The Crate sits under Library and shares its index.
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
            crate_dock: stop(cx, 29),
            // Under the artist, in the block of the Now Playing button.
            target: stop(cx, 19),
        };
        let progress = cx.new(|_| SliderState::default());
        let volume = cx.new(|_| SliderState::default());
        let root_focus = cx.focus_handle();
        let weak = cx.weak_entity();
        // Adds confirm themselves through this (the pill here, a pulse where the list shows).
        let confirm = cx.new(|_| crate::confirm::ConfirmModel::new(settings.clone()));
        cx.set_global(crate::confirm::ConfirmHandle(confirm.clone()));
        cx.subscribe(&confirm, |this, _, event: &crate::confirm::ConfirmEvent, cx| {
            this.confirmed(event, cx)
        })
        .detach();
        cx.observe(&confirm, |_, _, cx| cx.notify()).detach();
        let writes = cx.new(|cx| LibraryWrites::new(player.clone(), cx));
        let edits = cx.new(|_| LibraryEdits::new(player.clone(), writes.clone()));
        let picker = cx.new(|cx| {
            PlaylistPicker::new(player.clone(), writes.clone(), edits.clone(), settings.clone(), cx)
        });
        cx.set_global(PickerHandle(picker.clone()));
        let dialog = cx.new(|cx| PlaylistDialog::new(player.clone(), edits.clone(), settings.clone(), cx));
        cx.observe(&dialog, |_, _, cx| cx.notify()).detach();
        cx.set_global(DialogHandle(dialog.clone()));
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        let crate_model = cx.new(|_| CrateModel::new());
        cx.observe(&crate_model, |_, _, cx| cx.notify()).detach();
        cx.set_global(CrateHandle(crate_model.clone()));
        let recent_model = cx.new(|_| RecentModel::new());
        cx.set_global(RecentHandle(recent_model.clone()));
        crate_model.update(cx, |m, cx| {
            m.follow(player.clone(), &writes, settings.clone(), cx)
        });
        let target_model =
            cx.new(|cx| TargetModel::new(settings.clone(), writes.clone(), &edits, cx));
        cx.observe(&target_model, |_, _, cx| cx.notify()).detach();
        cx.set_global(TargetHandle(target_model.clone()));
        cx.set_global(crate::auto_advance::AutoAdvanceHandle(settings.clone()));
        let block_model = cx.new(|_| BlockModel::new());
        cx.set_global(BlockHandle(block_model.clone()));
        let blocked_dialog =
            cx.new(|cx| BlockedDialog::new(player.clone(), block_model.clone(), cx));
        cx.observe(&blocked_dialog, |_, _, cx| cx.notify()).detach();
        cx.set_global(BlockedDialogHandle(blocked_dialog.clone()));
        // A shortcut used while Ctrl is down (Ctrl+Z, say) puts the key badges away. Observed, not
        // listened for: a keystroke an action took never reaches a key listener.
        cx.observe_keystrokes(|this, _, _, cx| this.key_pressed(cx))
            .detach();
        let cheat_sheet = cx.new(CheatSheet::new);
        cx.observe(&cheat_sheet, |_, _, cx| cx.notify()).detach();
        cx.set_global(CheatSheetHandle(cheat_sheet.clone()));
        {
            // Tips ("Tip: press C") ride the pill; deferred, because they come from inside a click.
            let shell = cx.weak_entity();
            crate::shortcuts::install(
                settings.clone(),
                move |text, cx| {
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        let _ = shell.update(cx, |s, cx| s.show_tip(text, cx));
                    });
                },
                cx,
            );
        }
        let crate_page = cx.new(|cx| {
            CratePage::new(
                crate_model.clone(),
                player.clone(),
                writes.clone(),
                picker.clone(),
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_search(window, cx));
                    }
                },
                cx,
            )
        });
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
                    move |collection, occurrences, cx| {
                        let _ = weak.update(cx, |s, cx| {
                            let model = s.library.read(cx).model().clone();
                            model.update(cx, |m, cx| {
                                m.remove_rows(collection.library_key(), occurrences, cx)
                            });
                        });
                    }
                },
                {
                    let weak = weak.clone();
                    move |collection, occurrences, edge, cx| {
                        let _ = weak.update(cx, |s, cx| {
                            let model = s.library.read(cx).model().clone();
                            model.update(cx, |m, cx| {
                                m.arrange_rows(
                                    collection.library_key(),
                                    occurrences,
                                    Arrange::To(edge),
                                    cx,
                                )
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
        let thumbs = cx.new(|_| ThumbStore::new());
        let queue_panel = cx.new(|cx| {
            QueuePanel::new(
                player.clone(),
                writes.clone(),
                thumbs.clone(),
                {
                    let weak = weak.clone();
                    move |window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_search(window, cx));
                    }
                },
                Rc::new({
                    let weak = weak.clone();
                    move |target, window, cx| {
                        let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
                    }
                }),
                cx,
            )
        });
        let crate_dock_panel = cx.new(|cx| {
            CratePage::dock(
                crate_model.clone(),
                player.clone(),
                writes.clone(),
                picker.clone(),
                cx,
            )
        });
        // The Queue · Crate control, shared by the two panels that take the main window's right side.
        let dock_focus = DockFocus::new(cx);
        let on_dock_pick: crate::ui::dock_tabs::OnPick = Rc::new({
            let weak = weak.clone();
            move |dock, _, cx| {
                let _ = weak.update(cx, |s, cx| s.show_dock(dock, cx));
            }
        });
        queue_panel.update(cx, |p, _| p.set_dock_tabs(dock_focus.clone(), on_dock_pick.clone()));
        crate_dock_panel.update(cx, |p, _| p.set_dock_tabs(dock_focus.clone(), on_dock_pick.clone()));
        crate_dock_panel.update(cx, |p, _| {
            p.set_on_leave(Rc::new(Box::new({
                let weak = weak.clone();
                move |window, cx| {
                    let _ = weak.update(cx, |s, cx| s.leave_crate_dock(window, cx));
                }
            })))
        });
        let lyrics = cx.new(|cx| LyricsModel::new(player.clone(), cx));
        let lyrics_pane = cx.new(|cx| LyricsPane::new(player.clone(), lyrics.clone(), cx));
        cx.observe(&lyrics_pane, |_, _, cx| cx.notify()).detach();
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
        now_playing.update(cx, |n, _| {
            let weak = weak.clone();
            n.set_on_open(Rc::new(move |target, window, cx| {
                let _ = weak.update(cx, |s, cx| s.open_target(target, window, cx));
            }));
        });
        let account_page = cx.new(|cx| AccountPage::new(account.clone(), player.clone(), cx));
        let playback_page =
            cx.new(|cx| PlaybackPage::new(player.clone(), settings.clone(), cx));
        let audio_page = cx.new(|cx| AudioPage::new(player.clone(), settings.clone(), cx));
        let data_page =
            cx.new(|cx| DataPage::new(player.clone(), settings.clone(), account.clone(), cx));
        let settings_page = cx.new(|cx| {
            SettingsPage::new(
                account_page,
                playback_page,
                audio_page,
                data_page,
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
        library.update(cx, |l, _| {
            let weak = weak.clone();
            l.set_on_back(move |window, cx| {
                let _ = weak.update(cx, |s, cx| s.go_back(window, cx));
            });
        });
        let duplicates_dialog = {
            let model = library.read(cx).model().clone();
            cx.new(|cx| DuplicatesDialog::new(player.clone(), model, cx))
        };
        cx.observe(&duplicates_dialog, |_, _, cx| cx.notify()).detach();
        cx.set_global(DuplicatesDialogHandle(duplicates_dialog.clone()));
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
        // So does Home > Mood chips.
        let chips_on = settings.read(cx).get(&MOOD_CHIPS);
        home.update(cx, |h, cx| h.set_chips_enabled(chips_on, cx));
        // Playlists > Show More like this follows its switch.
        {
            let on = settings.read(cx).get(&MORE_LIKE_THIS);
            library.update(cx, |l, cx| l.set_more_like_enabled(on, cx));
            let library = library.clone();
            cx.observe(&settings, move |_, settings, cx| {
                let on = settings.read(cx).get(&MORE_LIKE_THIS);
                library.update(cx, |l, cx| l.set_more_like_enabled(on, cx));
            })
            .detach();
        }
        {
            let home = home.clone();
            cx.observe(&settings, move |_, settings, cx| {
                let on = settings.read(cx).get(&PERSONAL_SHELVES);
                home.update(cx, |h, cx| h.set_personal_enabled(on, cx));
                let chips = settings.read(cx).get(&MOOD_CHIPS);
                home.update(cx, |h, cx| h.set_chips_enabled(chips, cx));
            })
            .detach();
        }
        // Blocking or unblocking an artist changes what the personal shelves hold.
        {
            let home = home.clone();
            cx.subscribe(&block_model, move |_, _, _: &crate::blocked_artists::Changed, cx| {
                home.update(cx, |h, cx| h.refresh_personal(cx));
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
            blocked_dialog,
            duplicates_dialog,
            cheat_sheet,
            ctrl_held: false,
            badge_gen: 0,
            menu,
            choices,
            sleep_menu,
            toast: None,
            detail_origin: None,
            history: NavHistory::new(),
            pending_restore: None,
            settings_scroll: ScrollHandle::new(),
            history_back,
            history_forward,
            library_epoch: 0,
            library_usable: false,
            search_account_live: false,
            crate_model,
            block_model,
            target_model,
            crate_page,
            recent_model,
            pill: None,
            pill_seq: 0,
            routed_to_sign_in: false,
            _activation: None,
            _remembered: None,
            route: Route::Home,
            nav_expanded: false,
            nav_toggled: false,
            queue: PanelSlide::default(),
            queue_panel,
            crate_dock: PanelSlide::default(),
            crate_dock_panel,
            crate_dock_store: None,
            dock_focus: dock_focus.clone(),
            dock_return: None,
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
            // Where the old account had been is not a place to go back to.
            self.history.clear();
            self.pending_restore = None;
            // What the old account could undo does not carry over.
            self.player.update(cx, |p, _| p.clear_queue_history());
            self.crate_page.update(cx, |p, cx| p.reset(cx));
            self.library.update(cx, |l, cx| l.reset(cx));
            // Likes and the open picker belong to the old account.
            self.writes.update(cx, |w, cx| w.reset(cx));
            self.picker.update(cx, |p, cx| p.reset(cx));
            self.menu.update(cx, |m, cx| m.reset(cx));
            self.edits.update(cx, |e, cx| e.reset(cx));
            self.dialog.update(cx, |d, cx| d.reset(cx));
            self.blocked_dialog.update(cx, |d, cx| d.reset(cx));
            self.duplicates_dialog.update(cx, |d, cx| d.reset(cx));
            self.sleep_menu.update(cx, |m, cx| m.reset(cx));
        }
        // The Crate belongs to the chosen account (a moment without one keeps what shows).
        if let Some(key) = self.account_key(cx) {
            self.crate_model
                .update(cx, |m, cx| m.set_account(Some(key.clone()), cx));
            self.target_model
                .update(cx, |m, cx| m.set_account(Some(key.clone()), cx));
            self.block_model
                .update(cx, |m, cx| m.set_account(Some(key), cx));
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
            "nav-crate",
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
            ("bar-crate", bar.crate_dock.clone()),
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

    pub fn blocked_dialog(&self) -> &Entity<BlockedDialog> {
        &self.blocked_dialog
    }

    pub fn duplicates_dialog(&self) -> &Entity<DuplicatesDialog> {
        &self.duplicates_dialog
    }

    pub fn dialog(&self) -> &Entity<PlaylistDialog> {
        &self.dialog
    }

    pub fn choices(&self) -> &Entity<ChoiceMenu> {
        &self.choices
    }

    /// The Settings page, for tests that look inside it.
    pub fn settings_page(&self) -> &Entity<SettingsPage> {
        &self.settings_page
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

    // ---- Back and Forward ----------------------------------------------------------------

    /// The place on screen, as the history names it.
    fn place(&self, cx: &App) -> Place {
        if self.now_playing_open {
            return Place::NowPlaying;
        }
        match self.route {
            Route::Home => Place::Home,
            Route::Explore => Place::Explore,
            Route::Search => Place::Search,
            Route::Crate => Place::Crate,
            Route::Settings => Place::Settings,
            Route::Library => {
                let model = self.library.read(cx).model().read(cx);
                match model.top_target() {
                    Some(Target::Collection(c)) => Place::Collection(c.library_key().to_owned()),
                    Some(Target::Artist(a)) => Place::Artist(a.id.as_str().to_owned()),
                    None => Place::Library(model.tab()),
                }
            }
        }
    }

    /// How far the page of `place` is scrolled.
    fn page_scroll(&self, place: &Place, cx: &App) -> Option<Point<Pixels>> {
        match place {
            Place::Home => Some(self.home_page.read(cx).scroll_offset()),
            Place::Explore => self.explore_page.read(cx).scroll_offset(cx),
            Place::Search => Some(self.search.read(cx).scroll_offset()),
            Place::Library(_) | Place::Collection(_) | Place::Artist(_) => {
                Some(self.library.read(cx).scroll_offset(cx))
            }
            Place::Crate => Some(self.crate_page.read(cx).scroll_offset()),
            Place::Settings => Some(self.settings_scroll.offset()),
            Place::NowPlaying => None,
        }
    }

    fn set_page_scroll(&self, place: &Place, offset: Point<Pixels>, cx: &App) {
        match place {
            Place::Home => self.home_page.read(cx).set_scroll_offset(offset),
            Place::Explore => self.explore_page.read(cx).set_scroll_offset(offset),
            Place::Search => self.search.read(cx).set_scroll_offset(offset),
            Place::Library(_) | Place::Collection(_) | Place::Artist(_) => {
                self.library.read(cx).set_scroll_offset(offset, cx)
            }
            Place::Crate => self.crate_page.read(cx).set_scroll_offset(offset),
            Place::Settings => self.settings_scroll.set_offset(offset),
            Place::NowPlaying => {}
        }
    }

    /// What is worth keeping about the page of `place` as it is now.
    fn snapshot(&self, place: &Place, cx: &App) -> PlaceState {
        let library_page = matches!(
            place,
            Place::Library(_) | Place::Collection(_) | Place::Artist(_)
        );
        PlaceState {
            target: matches!(place, Place::Collection(_) | Place::Artist(_))
                .then(|| self.library.read(cx).model().read(cx).top_target())
                .flatten(),
            scroll: self.page_scroll(place, cx),
            query: (*place == Place::Search)
                .then(|| self.search.read(cx).field().read(cx).text().to_owned()),
            filter: library_page
                .then(|| self.library.read(cx).filter_snapshot(cx))
                .flatten(),
            origin: self.detail_origin,
        }
    }

    /// Record the place on screen: a new one is added to the history, and the one that is
    /// current keeps up with what is done on it (scroll, search text, filter). Called every
    /// frame, so every way of getting anywhere is recorded without saying so.
    fn sync_history(&mut self, cx: &mut Context<Self>) {
        let place = self.place(cx);
        if self.history.current().map(|e| &e.place) != Some(&place) {
            self.pending_restore = None;
            let state = self.snapshot(&place, cx);
            self.history.observe(place.clone(), state);
        }
        // While a returned-to page is still being scrolled into place its saved position is
        // what the history holds; the page's present one would overwrite it.
        if self.pending_restore.is_none() {
            let state = self.snapshot(&place, cx);
            if let Some(slot) = self.history.current_state_mut() {
                *slot = state;
            }
        }
    }

    /// Put a returned-to page's scroll position back, a frame at a time until it holds (the
    /// page may not be tall enough yet).
    fn apply_pending_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((place, target, tries)) = self
            .pending_restore
            .as_ref()
            .map(|p| (p.place.clone(), p.scroll, p.tries))
        else {
            return;
        };
        let Some(target) = target else {
            self.pending_restore = None;
            return;
        };
        if self.place(cx) != place {
            self.pending_restore = None;
            return;
        }
        let settled = self.page_scroll(&place, cx).is_some_and(|here| {
            (f32::from(here.x) - f32::from(target.x)).abs() < 1.0
                && (f32::from(here.y) - f32::from(target.y)).abs() < 1.0
        });
        if settled || tries >= RESTORE_TRIES {
            self.pending_restore = None;
            return;
        }
        self.set_page_scroll(&place, target, cx);
        if let Some(p) = self.pending_restore.as_mut() {
            p.tries += 1;
        }
        window.request_animation_frame();
    }

    pub fn can_go_back(&self, cx: &App) -> bool {
        self.history.can_back() || self.mood_open(cx)
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_forward()
    }

    /// The places visited and the current one's index (tests).
    pub fn history_places(&self) -> (Vec<Place>, usize) {
        self.history.places()
    }

    /// An Explore mood page is open over the tiles: Back closes it before it leaves Explore.
    fn mood_open(&self, cx: &App) -> bool {
        !self.now_playing_open
            && self.route == Route::Explore
            && self.explore.read(cx).mood().is_some()
    }

    /// A popup or dialog is open: the history keys and mouse buttons leave the screen alone.
    fn overlay_open(&self, cx: &App) -> bool {
        self.picker.read(cx).is_open()
            || self.menu.read(cx).is_open()
            || self.dialog.read(cx).is_open()
            || self.blocked_dialog.read(cx).is_open()
            || self.duplicates_dialog.read(cx).is_open()
            || self.cheat_sheet.read(cx).is_open()
            || self.choices.read(cx).is_open()
            || self.sleep_menu.read(cx).is_open()
    }

    /// Back: to the previous place, scrolled and filled in as it was left. An open mood page
    /// closes first. With nowhere to go back to, a page the history never saw just closes.
    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Catch up with anything done since the last frame.
        self.sync_history(cx);
        if self.mood_open(cx) {
            self.explore_page.update(cx, |e, cx| e.back(window, cx));
            return;
        }
        match self.history.back() {
            Some(entry) => self.show_place(entry, window, cx),
            None => {
                if self.route == Route::Library && !self.now_playing_open {
                    self.library.update(cx, |l, cx| l.back_one(window, cx));
                }
            }
        }
    }

    /// Forward: undoes a Back.
    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_history(cx);
        if let Some(entry) = self.history.forward() {
            self.show_place(entry, window, cx);
        }
    }

    /// Show a place the history stepped to, and put back what was done on it.
    fn show_place(&mut self, entry: HistoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        let HistoryEntry { place, state } = entry;
        match &place {
            Place::NowPlaying => self.set_now_playing(true, cx),
            Place::Home => self.show_route(Route::Home, cx),
            Place::Explore => self.show_route(Route::Explore, cx),
            Place::Crate => self.show_route(Route::Crate, cx),
            Place::Settings => self.show_route(Route::Settings, cx),
            Place::Search => {
                self.show_route(Route::Search, cx);
                if let Some(query) = &state.query {
                    self.search.update(cx, |s, cx| s.restore_query(query, cx));
                }
                self.search.update(cx, |s, cx| s.restore_focus(window, cx));
            }
            Place::Library(tab) => {
                self.show_route(Route::Library, cx);
                self.library.update(cx, |l, cx| {
                    l.go_home(window, cx);
                    l.set_tab(*tab, cx);
                });
            }
            Place::Collection(_) | Place::Artist(_) => {
                let Some(target) = state.target.clone() else {
                    return;
                };
                self.leave_opened_pages(cx);
                self.route = Route::Library;
                self.now_playing_open = false;
                self.library.update(cx, |l, cx| {
                    l.go_home(window, cx);
                    l.open_target(target, window, cx);
                });
                self.detail_origin = state.origin;
            }
        }
        if let Some(filter) = &state.filter {
            self.library
                .update(cx, |l, cx| l.restore_filter(filter, window, cx));
        }
        if !matches!(
            place,
            Place::Search | Place::Library(_) | Place::Collection(_) | Place::Artist(_)
        ) {
            self.focus_root(window, cx);
        }
        self.pending_restore = (place != Place::NowPlaying).then(|| PendingRestore {
            place,
            scroll: state.scroll,
            tries: 0,
        });
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
                    self.queue_takes_the_slot(cx);
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

    /// The C key: set the playing song aside in the Crate (a song already there stays).
    fn crate_current(&mut self, cx: &mut Context<Self>) {
        let track = self
            .player
            .read(cx)
            .coordinator()
            .and_then(|c| c.queue().current().map(|e| e.track.clone()));
        if let Some(track) = track {
            crate::crate_list::add_track(&self.player, track, None, cx);
        }
    }

    /// The P key: the playlist picker for the playing song (next to its Now Playing button when
    /// that is on screen). The keys on a list's own cursor row are the list's.
    fn add_current_to_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.picker.read(cx).key_enabled(cx) {
            return;
        }
        let track = self
            .player
            .read(cx)
            .coordinator()
            .and_then(|c| c.queue().current().map(|e| e.track.clone()));
        let Some(track) = track else {
            return;
        };
        let anchor = self
            .now_playing_open
            .then_some(crate::ui::now_playing::PLAYLIST_ANCHOR);
        playlist_picker::open_songs(
            vec![PickerSong::new(track.video_id.as_str(), track.title.clone())],
            anchor,
            window,
            cx,
        );
    }

    /// The A key: add the playing song to the target playlist (the picker opens to choose one
    /// when there is none). The keys on a list's own cursor row are the list's.
    fn add_current_to_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let track = self
            .player
            .read(cx)
            .coordinator()
            .and_then(|c| c.queue().current().map(|e| e.track.clone()));
        let Some(track) = track else {
            return;
        };
        let anchor = self
            .now_playing_open
            .then_some(crate::ui::now_playing::PLAYLIST_ANCHOR);
        crate::target_playlist::add_songs(
            vec![PickerSong::new(track.video_id.as_str(), track.title.clone())],
            anchor,
            window,
            cx,
        );
    }

    pub fn target_model(&self) -> &Entity<TargetModel> {
        &self.target_model
    }

    pub fn crate_model(&self) -> &Entity<CrateModel> {
        &self.crate_model
    }

    pub fn crate_page(&self) -> &Entity<CratePage> {
        &self.crate_page
    }

    /// Where the Crate is kept between runs.
    pub fn install_crate_store(
        &mut self,
        store: Option<Arc<dyn crate::crate_list::CrateStore>>,
        cx: &mut Context<Self>,
    ) {
        self.crate_model.update(cx, |m, cx| m.install(store, cx));
    }

    /// Where the blocked artists are kept between runs.
    pub fn install_block_store(
        &mut self,
        store: Option<Arc<dyn crate::blocked_artists::BlockStore>>,
        cx: &mut Context<Self>,
    ) {
        self.block_model.update(cx, |m, cx| m.install(store, cx));
    }

    pub fn block_model(&self) -> &Entity<BlockModel> {
        &self.block_model
    }

    /// The opaque id of the signed-in account, once it is chosen.
    fn account_key(&self, cx: &App) -> Option<String> {
        match self.account.read(cx).service().view().identity {
            apollo_account::service::IdentityState::Selected(identity) => Some(identity.key()),
            _ => None,
        }
    }

    /// Ctrl+Z: take back the last thing done to the queue or to a playlist, whichever came last.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let (depth, queue_undoable) = {
            let player = self.player.read(cx);
            (player.queue_undo_depth(), player.queue_undoable())
        };
        let playlist = self.writes.read(cx).playlist_undo_side();
        let crate_ = self.crate_model.read(cx).undo_side();
        match apollo_domain::undo::newest(depth, queue_undoable, playlist, crate_) {
            apollo_domain::undo::Newest::Queue => self.undo_queue(cx),
            apollo_domain::undo::Newest::Playlist => self.undo_playlist(cx),
            apollo_domain::undo::Newest::Crate => self.undo_crate(cx),
            apollo_domain::undo::Newest::Nothing => {}
        }
    }

    /// Take back the last change to the Crate; the pill says what was undone.
    fn undo_crate(&mut self, cx: &mut Context<Self>) {
        if let Some(words) = self.crate_model.update(cx, |m, cx| m.undo(cx)) {
            self.show_tip(format!("Undid: {words}").into(), cx);
        }
    }

    /// Take back the last queue change.
    pub fn undo_queue(&mut self, cx: &mut Context<Self>) {
        if self.player.update(cx, |p, cx| p.undo_queue(cx)) {
            self.show_pill("Queue restored", toast::PILL_FOR, false, cx);
        }
    }

    /// Take back the last playlist edit; the pill says what was undone.
    fn undo_playlist(&mut self, cx: &mut Context<Self>) {
        let model = self.library.read(cx).model().clone();
        if let Some(words) = model.update(cx, |m, cx| m.undo_playlist_edit(cx)) {
            self.show_tip(format!("Undid: {words}").into(), cx);
        }
    }

    /// Ctrl+Shift+Z: do again what Ctrl+Z took back.
    pub fn redo_queue(&mut self, cx: &mut Context<Self>) {
        if self.player.update(cx, |p, cx| p.redo_queue(cx)) {
            self.show_pill("Queue redone", toast::PILL_FOR, false, cx);
        }
    }

    /// The text of the undo pill while it shows.
    pub fn pill_text(&self) -> Option<String> {
        self.pill.as_ref().map(|p| p.text.to_string())
    }

    /// Show `text` in the pill for [`toast::PILL_FOR`], fading out over the last bit (not at
    /// all under reduced motion).
    fn show_pill(
        &mut self,
        text: impl Into<gpui::SharedString>,
        lasts: std::time::Duration,
        tip: bool,
        cx: &mut Context<Self>,
    ) {
        self.pill_seq += 1;
        let id = self.pill_seq;
        self.pill = Some(PillShown {
            id,
            text: text.into(),
            leaving: false,
            tip,
        });
        let animate = !cx.reduce_motion();
        cx.spawn(async move |this, cx| {
            if animate {
                cx.background_executor()
                    .timer(lasts.saturating_sub(toast::FADE))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(p) = this.pill.as_mut().filter(|p| p.id == id) {
                        p.leaving = true;
                        cx.notify();
                    }
                });
                cx.background_executor().timer(toast::FADE).await;
            } else {
                cx.background_executor().timer(lasts).await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.pill.as_ref().is_some_and(|p| p.id == id) {
                    this.pill = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Something was added to a list: say so in the pill, for [`crate::confirm::CONFIRM_FOR`].
    fn confirmed(&mut self, event: &crate::confirm::ConfirmEvent, cx: &mut Context<Self>) {
        let crate::confirm::ConfirmEvent::Show(c) = event;
        // A tip is shown once ever; it is not worth losing to an add's confirmation.
        if self.pill.as_ref().is_some_and(|p| p.tip && !p.leaving) {
            return;
        }
        let text = crate::confirm::with_undo_hint(&c.text, c.undoable, crate::shortcuts::hints_on(cx));
        self.show_pill(text, crate::confirm::CONFIRM_FOR, false, cx);
    }

    /// A one-line shortcut tip ("Tip: press C"), for [`toast::TIP_FOR`].
    pub fn show_tip(&mut self, text: gpui::SharedString, cx: &mut Context<Self>) {
        self.show_pill(text, toast::TIP_FOR, true, cx);
    }

    /// Ctrl went down or up. Held alone for [`BADGE_AFTER`], the key badges show on the buttons
    /// that have a single-key shortcut; letting go (or any other key) hides them.
    fn modifiers_changed(&mut self, m: gpui::Modifiers, cx: &mut Context<Self>) {
        let ctrl_alone = m.control && !m.alt && !m.shift && !m.platform;
        if ctrl_alone == self.ctrl_held {
            return;
        }
        self.ctrl_held = ctrl_alone;
        self.badge_gen += 1;
        if !ctrl_alone {
            self.hide_badges(cx);
            return;
        }
        if !crate::shortcuts::badges_enabled(cx) {
            return;
        }
        let generation = self.badge_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BADGE_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.ctrl_held && this.badge_gen == generation && crate::shortcuts::set_badges(true, cx) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Any key pressed while Ctrl is down (Ctrl+Z, say) is a shortcut being used, not looked up.
    fn key_pressed(&mut self, cx: &mut Context<Self>) {
        if self.ctrl_held {
            self.badge_gen += 1;
            self.hide_badges(cx);
        }
    }

    fn hide_badges(&mut self, cx: &mut Context<Self>) {
        if crate::shortcuts::set_badges(false, cx) {
            cx.notify();
        }
    }

    pub fn cheat_sheet(&self) -> &Entity<CheatSheet> {
        &self.cheat_sheet
    }

    pub fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        if self.lyrics_showing(cx) || self.related_showing(cx) {
            // The queue asked for the side the lyrics or related songs are on: they give way.
            self.lyrics_pane.update(cx, |p, cx| p.set_open(false, cx));
            self.related_pane.update(cx, |p, cx| p.set_open(false, cx));
            if !self.queue.is_open() {
                self.queue_takes_the_slot(cx);
                self.queue.set(true, Instant::now(), !cx.reduce_motion());
                self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
            }
            cx.notify();
            return;
        }
        let open = !self.queue.is_open();
        if open {
            self.queue_takes_the_slot(cx);
        }
        self.queue.set(open, Instant::now(), !cx.reduce_motion());
        if open {
            self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
        }
        cx.notify();
    }

    // ---- the docked Crate ------------------------------------------------------------------

    /// Whether the docked Crate is open. It shows while no page takes the whole window (Now
    /// Playing) and the Crate page itself is not on screen.
    pub fn crate_dock_open(&self) -> bool {
        self.crate_dock.is_open()
    }

    pub fn crate_dock_panel(&self) -> &Entity<CratePage> {
        &self.crate_dock_panel
    }

    /// The Crate button and Ctrl+Shift+C.
    pub fn toggle_crate_dock(&mut self, cx: &mut Context<Self>) {
        let open = !self.crate_dock.is_open();
        self.set_crate_dock(open, cx);
    }

    /// Whether the docked Crate is on screen: open, and no page takes the window (Now Playing) or
    /// shows the same list (the Crate page).
    fn crate_dock_visible(&self) -> bool {
        self.crate_dock.is_open() && !self.now_playing_open && self.route != Route::Crate
    }

    /// Whether the keyboard is somewhere in the docked Crate.
    pub fn crate_dock_focused(&self, window: &Window, cx: &App) -> bool {
        self.crate_dock_visible()
            && (self.crate_dock_panel.read(cx).has_focus(window)
                || self.dock_focus.any_focused(window))
    }

    /// Ctrl+Shift+C: open the docked Crate and put the cursor in it, on the row it was last on
    /// (else the first); Esc goes back to where the keyboard was. With the dock already open and
    /// focused it closes instead, and focus goes back too. On the Crate page, which is the same
    /// list, or with an empty Crate, the key only opens or closes the dock as before.
    pub fn jump_crate_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // With no list to land on (the Crate page is the same list; an empty Crate shows only its
        // hint) there is nothing to jump into, so the key just opens or closes the dock.
        if self.route == Route::Crate || self.crate_model.read(cx).is_empty() {
            self.toggle_crate_dock(cx);
            return;
        }
        if self.crate_dock_focused(window, cx) {
            self.set_crate_dock(false, cx);
            self.return_from_crate_dock(window, cx);
            return;
        }
        let before = window.focused(cx);
        self.set_crate_dock(true, cx);
        let entered = self
            .crate_dock_panel
            .update(cx, |p, cx| p.enter(window, cx));
        // Only a jump that landed has somewhere to come back from; a second press while the dock
        // is already open but unfocused keeps the first place.
        if entered {
            self.dock_return = before.or(self.dock_return.take());
        }
    }

    /// Esc in the docked list: the keyboard goes back to where the jump into it came from.
    pub fn leave_crate_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.return_from_crate_dock(window, cx);
    }

    fn return_from_crate_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let queue_list = self.queue_panel.read(cx).focus_handle(cx);
        match self.dock_return.take() {
            // The queue went away when the Crate took its slot: there is nothing to land on.
            Some(handle) if handle != queue_list || self.queue.is_open() => {
                window.focus(&handle, cx)
            }
            _ => self.focus_main(window, cx),
        }
    }

    /// Open or close the docked Crate. Opening it takes the slot from the queue panel (and
    /// leaves Now Playing, which fills the window); the choice is remembered.
    pub fn set_crate_dock(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.crate_dock.is_open() == open {
            return;
        }
        let animate = !cx.reduce_motion();
        self.crate_dock.set(open, Instant::now(), animate);
        if open {
            self.queue.set(false, Instant::now(), animate);
            if self.now_playing_open {
                self.set_now_playing(false, cx);
            }
        }
        self.save_crate_dock(open, cx);
        cx.notify();
    }

    /// The Queue · Crate · Recent control: the other panel (or list) takes the slot.
    pub fn show_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        match dock {
            Dock::Crate | Dock::Recent => {
                // The Crate panel shows the Crate or the recently played list.
                self.crate_dock_panel.update(cx, |p, cx| p.show_view(dock, cx));
                self.set_crate_dock(true, cx);
            }
            Dock::Queue => {
                self.set_crate_dock(false, cx);
                if !self.queue.is_open() {
                    self.queue.set(true, Instant::now(), !cx.reduce_motion());
                    self.queue_panel.update(cx, |p, cx| p.reveal_current(cx));
                }
                cx.notify();
            }
        }
    }

    /// The queue was opened: it has the slot, so the Crate gives it up.
    fn queue_takes_the_slot(&mut self, cx: &mut Context<Self>) {
        self.set_crate_dock(false, cx);
    }

    fn save_crate_dock(&self, open: bool, cx: &mut Context<Self>) {
        let Some(store) = self.crate_dock_store.clone() else {
            return;
        };
        cx.background_executor()
            .spawn(async move { store.set(KEY_CRATE_DOCK, if open { "on" } else { "off" }) })
            .detach();
    }

    /// Where the docked Crate's open/closed state is remembered, and the saved state applied.
    pub fn install_crate_dock(
        &mut self,
        store: Option<Arc<dyn SettingsBackend>>,
        cx: &mut Context<Self>,
    ) {
        self.crate_dock_store = store.clone();
        self.recent_model
            .update(cx, |m, cx| m.install(store.clone(), cx));
        let Some(store) = store else { return };
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { store.get(KEY_CRATE_DOCK) })
                .await;
            let _ = this.update(cx, |s, cx| {
                // A choice made while this was loading wins over the saved one.
                if saved.as_deref() == Some("on") && !s.crate_dock.is_open() && !s.queue.is_open() {
                    s.crate_dock.set(true, Instant::now(), false);
                    cx.notify();
                }
            });
        })
        .detach();
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
        self._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                crate::ui::tooltip::dismiss_all(cx);
                // Ctrl released in another window is never heard here.
                this.ctrl_held = false;
                this.badge_gen += 1;
                this.hide_badges(cx);
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

    /// The nav rail's stops from top to bottom: the toggle, then one per destination.
    fn rail_stops(&self) -> Vec<FocusHandle> {
        std::iter::once(self.nav_toggle.clone())
            .chain(self.nav_items.iter().cloned())
            .collect()
    }

    /// The plain arrows, where no list, grid, slider or menu with focus binds them itself.
    ///
    /// * Nothing focused: the first press lands on the page's main list (see
    ///   [`Self::focus_main`]).
    /// * On the nav rail: Up / Down walk the destinations, Right goes into the page, Left stays.
    /// * Anywhere else: Up / Left go to the previous Tab stop and Down / Right to the next, the
    ///   same order Tab and Shift+Tab use.
    fn navigate(&mut self, step: ArrowStep, window: &mut Window, cx: &mut Context<Self>) {
        if self.root_focus.is_focused(window) {
            self.focus_main(window, cx);
            return;
        }
        let rail = self.rail_stops();
        if let Some(at) = rail.iter().position(|h| h.is_focused(window)) {
            match step {
                ArrowStep::Up if at > 0 => window.focus(&rail[at - 1], cx),
                ArrowStep::Down if at + 1 < rail.len() => window.focus(&rail[at + 1], cx),
                ArrowStep::Right => self.focus_main(window, cx),
                _ => {}
            }
            return;
        }
        match step {
            ArrowStep::Up | ArrowStep::Left => window.focus_prev(cx),
            ArrowStep::Down | ArrowStep::Right => window.focus_next(cx),
        }
    }

    /// Left, from a list or grid: onto the rail's button for the page on screen.
    fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = if self.now_playing_open { None } else { Some(self.route) };
        let at = route.and_then(|r| Route::ALL.iter().position(|x| *x == r));
        let stop = match at {
            Some(at) => self.nav_items[at].clone(),
            None => self.nav_toggle.clone(),
        };
        window.focus(&stop, cx);
    }

    /// Put the cursor on the main list of what is on screen: the page's list, grid or first
    /// shelf, else the queue when it is open, else the first Tab stop past the rail.
    pub fn focus_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (queue_open, queue_list) = (
            self.queue.is_open() && !self.lyrics_showing(cx) && !self.related_showing(cx),
            self.queue_panel.read(cx).focus_handle(cx),
        );
        let page = if self.now_playing_open {
            None
        } else {
            match self.route {
                Route::Library => Some(self.library.read(cx).focus_handle(cx)),
                Route::Crate => Some(self.crate_page.read(cx).focus_handle(cx)),
                Route::Home => self.home_page.read(cx).main_focus(),
                Route::Explore => self.explore_page.read(cx).main_focus(cx),
                Route::Search => {
                    self.search.update(cx, |p, cx| p.focus_main(window, cx));
                    return;
                }
                Route::Settings => None,
            }
        };
        // The docked Crate steps aside for Now Playing and for the Crate page (the same list).
        let dock_open =
            self.crate_dock.is_open() && !self.now_playing_open && self.route != Route::Crate;
        let dock_list = self.crate_dock_panel.read(cx).focus_handle(cx);
        let side = queue_open
            .then_some(queue_list)
            .or(dock_open.then_some(dock_list));
        if let Some(handle) = page.or(side) {
            window.focus(&handle, cx);
            return;
        }
        // No list to land on: the first Tab stop that is not the rail's.
        let rail = self.rail_stops();
        for _ in 0..=rail.len() {
            window.focus_next(cx);
            if !rail.iter().any(|h| h.is_focused(window)) {
                break;
            }
        }
    }
}

/// An arrow key, for [`Shell::navigate`].
#[derive(Clone, Copy)]
enum ArrowStep {
    Up,
    Down,
    Left,
    Right,
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
        // Full screen belongs to Now Playing: leaving it (Esc, the chevron, another page) leaves
        // full screen too.
        if window.is_fullscreen() && !self.now_playing_open {
            window.toggle_fullscreen();
        }
        self.sync_history(cx);
        self.apply_pending_restore(window, cx);
        let view = self.player.read(cx).view();
        if view.accent_moving {
            window.request_animation_frame();
        }
        let accent = view.accent;
        let shell = cx.entity();

        // Back and Forward, at the left of the title bar.
        let history_buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .px(px(spacing::S1))
            .child(
                div().debug_selector(|| "history-back-slot".into()).child(
                    IconButton::new(
                        "history-back",
                        &self.history_back,
                        Icon::ChevronLeft,
                        "Back",
                        accent,
                    )
                    .size(HISTORY_BUTTON)
                    .skip_tab()
                    .enabled(self.can_go_back(cx))
                    .on_click({
                        let shell = shell.clone();
                        move |window, cx| shell.update(cx, |s, cx| s.go_back(window, cx))
                    }),
                ),
            )
            .child(
                div().debug_selector(|| "history-forward-slot".into()).child(
                    IconButton::new(
                        "history-forward",
                        &self.history_forward,
                        Icon::ChevronRight,
                        "Forward",
                        accent,
                    )
                    .size(HISTORY_BUTTON)
                    .skip_tab()
                    .enabled(self.can_go_forward())
                    .on_click({
                        let shell = shell.clone();
                        move |window, cx| shell.update(cx, |s, cx| s.go_forward(window, cx))
                    }),
                ),
            );

        // An instant playlist was just made: open its page with the title ready to rename.
        if let Some(made) = self.dialog.update(cx, |d, _| d.take_instant()) {
            let shell = shell.clone();
            window.defer(cx, move |window, cx| {
                shell.update(cx, |s, cx| {
                    s.open_target(Target::Collection(made.clone()), window, cx);
                    s.library
                        .update(cx, |l, cx| l.begin_rename(&made, window, cx));
                });
            });
        }

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
        .crate_count(self.crate_model.read(cx).len())
        .attention(self.account.read(cx).attention())
        .on_route({
            let shell = shell.clone();
            move |route, window, cx| {
                shell.update(cx, |s, cx| s.choose_route(route, window, cx));
            }
        });

        let page = div()
            .id("page")
            .debug_selector(|| "page".into())
            .flex_1()
            .min_w_0()
            .h_full();
        let page = if self.now_playing_open {
            page.child(self.now_playing.clone())
        } else if self.route == Route::Crate {
            page.px(px(spacing::S8))
                .pt(px(spacing::S6))
                .child(self.crate_page.clone())
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
                .track_scroll(&self.settings_scroll)
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

        // The docked Crate slides in the same way, in the queue's place. It stays out of the way
        // of Now Playing (which fills the window) and of the Crate page (which is the same list).
        let (dock_level, dock_moving) = self.crate_dock.level(Instant::now());
        if dock_moving {
            window.request_animation_frame();
        }
        let dock_shown =
            dock_level > 0.0 && !self.now_playing_open && self.route != Route::Crate;
        // Narrow windows give the page all the width: the panel overlays its right edge instead.
        let dock_overlays = f32::from(window.viewport_size().width) < DOCK_OVERLAY_BELOW;
        let crate_dock = dock_shown.then(|| {
            let slot = div()
                .id("crate-dock-slot")
                .h_full()
                .w(px(layout::QUEUE_PANEL_WIDTH * dock_level))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .child(self.crate_dock_panel.clone()),
                );
            if dock_overlays {
                slot.absolute().top_0().bottom_0().right_0()
            } else {
                slot.relative().flex_none()
            }
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
            crate_open: self.crate_dock.is_open(),
            crate_count: self.crate_model.read(cx).len(),
            on_crate_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| shell.update(cx, |s, cx| s.toggle_crate_dock(cx))
            })),
            lyrics_open: lyrics_showing,
            on_lyrics_toggle: Rc::new(Box::new({
                let shell = shell.clone();
                move |_, cx| shell.update(cx, |s, cx| s.toggle_lyrics(cx))
            })),
            // Deferred: this runs inside the main window's own event, and opening the mini player
            // reads and then hides that same window.
            on_mini_player: Rc::new(Box::new(|_, cx| cx.defer(crate::mini::open))),
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
            target: crate::target_playlist::pill_label(cx),
            on_open: Rc::new({
                let shell = shell.clone();
                move |target, window, cx| {
                    shell.update(cx, |s, cx| s.open_target(target, window, cx))
                }
            }),
            on_target: Rc::new(Box::new(|window, cx| {
                playlist_picker::open_target(
                    Vec::new(),
                    Some(crate::ui::player_bar::TARGET_ANCHOR),
                    window,
                    cx,
                );
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
        let pill = self.pill.clone().map(|p| {
            toast::pill(
                p.id,
                p.text,
                if p.leaving { Fade::Out } else { Fade::In },
                animate,
                cx,
            )
        });
        let picker = self.picker.read(cx).is_open().then(|| self.picker.clone());
        let menu = self.menu.read(cx).is_open().then(|| self.menu.clone());
        let dialog = self.dialog.read(cx).is_open().then(|| self.dialog.clone());
        let blocked_dialog = self
            .blocked_dialog
            .read(cx)
            .is_open()
            .then(|| self.blocked_dialog.clone());
        let duplicates_dialog = self
            .duplicates_dialog
            .read(cx)
            .is_open()
            .then(|| self.duplicates_dialog.clone());
        let cheat_sheet = self
            .cheat_sheet
            .read(cx)
            .is_open()
            .then(|| self.cheat_sheet.clone());
        let choices = self.choices.read(cx).is_open().then(|| self.choices.clone());
        let sleep_menu = self
            .sleep_menu
            .read(cx)
            .is_open()
            .then(|| self.sleep_menu.clone());

        let fonts = cx.global::<style::Fonts>().text.clone();
        let skimming = self.player.read(cx).skimming();
        div()
            .id("shell")
            .key_context(if skimming {
                SKIMMING_CONTEXT
            } else {
                KEY_CONTEXT
            })
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
            // F11 takes Now Playing full screen and back; elsewhere it does nothing.
            .on_action(cx.listener(|this, _: &ToggleFullscreen, window, _| {
                if this.now_playing_open || window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
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
                this.player
                    .update(cx, |m, cx| m.nudge_volume(VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| {
                this.player
                    .update(cx, |m, cx| m.nudge_volume(-VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &SeekBack, _, cx| {
                this.player.update(cx, |m, cx| {
                    m.os_command(OsCommand::SeekBy(-SEEK_STEP_MS), cx)
                });
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.player.update(cx, |m, cx| {
                    m.os_command(OsCommand::SeekBy(SEEK_STEP_MS), cx)
                });
            }))
            .on_action(cx.listener(|_, _: &ShowShortcuts, window, cx| cheat_sheet::toggle(window, cx)))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                if !this.overlay_open(cx) {
                    this.go_back(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| {
                if !this.overlay_open(cx) {
                    this.go_forward(window, cx);
                }
            }))
            // The mouse's side buttons.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| {
                    if !this.overlay_open(cx) {
                        this.go_back(window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| {
                    if !this.overlay_open(cx) {
                        this.go_forward(window, cx);
                    }
                }),
            )
            .on_modifiers_changed(cx.listener(|this, event: &gpui::ModifiersChangedEvent, _, cx| {
                this.modifiers_changed(event.modifiers, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_lyrics(cx)))
            .on_action(cx.listener(|this, _: &LikeCurrent, _, cx| this.like_current(cx)))
            .on_action(cx.listener(|this, _: &CrateCurrent, _, cx| this.crate_current(cx)))
            .on_action(cx.listener(|this, _: &ToggleCrateDock, window, cx| this.jump_crate_dock(window, cx)))
            .on_action(cx.listener(|this, _: &AddCurrentToPlaylist, window, cx| {
                this.add_current_to_playlist(window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddCurrentToTarget, window, cx| {
                this.add_current_to_target(window, cx)
            }))
            // Skim: K sets the song aside and the skim goes on, Enter keeps the song, Esc goes
            // back to what was playing. The keys only exist while the shell is "Skimming".
            .on_action(cx.listener(|this, _: &SkimCrate, _, cx| {
                this.crate_current(cx);
                // With auto-advance on, K also moves on to the next song.
                if crate::auto_advance::enabled(cx) {
                    this.player.update(cx, |m, cx| {
                        m.skim_advance(cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &SkimKeep, _, cx| {
                this.player.update(cx, |m, cx| m.skim_keep(cx));
            }))
            .on_action(cx.listener(|this, _: &SkimCancel, _, cx| {
                this.player.update(cx, |m, cx| m.skim_cancel(cx));
            }))
            .on_action(cx.listener(|this, _: &UndoQueue, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &RedoQueue, _, cx| this.redo_queue(cx)))
            .on_action(cx.listener(|this, _: &NavigateUp, window, cx| {
                this.navigate(ArrowStep::Up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NavigateDown, window, cx| {
                this.navigate(ArrowStep::Down, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NavigateLeft, window, cx| {
                this.navigate(ArrowStep::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NavigateRight, window, cx| {
                this.navigate(ArrowStep::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusSidebar, window, cx| {
                this.focus_sidebar(window, cx)
            }))
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
            .child(TitleBar::new().left(history_buttons))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(nav)
                    .child(page)
                    .children(queue)
                    .children(crate_dock),
            )
            .child(bar)
            .children(toast)
            .children(pill)
            .children(picker)
            .children(menu)
            .children(dialog)
            .children(blocked_dialog)
            .children(duplicates_dialog)
            .children(cheat_sheet)
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
