//! App-wide keyboard shortcuts: M, Ctrl+Up/Down, Ctrl+Left/Right, Shift+Left/Right, Ctrl+L and L
//! bound on the shell, left to text fields and popups, plus what the global hotkeys do. The real shell on
//! GPUI's test platform with the deterministic mock adapters.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::keymap::GlobalCommand;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::playback::{EngineEvent, Event, Intent};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

/// The track that is playing at the start.
const FIRST: &str = "simTrack001";
/// The track after it, not liked.
const SECOND: &str = "plainTrack1";

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

struct NoSearch;
impl SearchSource for NoSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    sim: SimLibrary,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let sim = SimLibrary::instant();
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: [(FIRST, "First"), (SECOND, "Second")]
            .into_iter()
            .map(|(id, title)| LibraryTrack {
                track: apollo_domain::model::Track::new(id, title),
            })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (Page { player, shell, sim }, vcx)
}

/// Lets the current load finish opening: a 200 s track.
fn open_current_load(page: &Page, vcx: &mut VisualTestContext) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
        m.handle(
            Event::Resolved {
                load,
                result: Ok(apollo_domain::playback::LeaseId(1)),
            },
            cx,
        );
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Opened {
                    duration_ms: Some(200_000),
                },
            },
            cx,
        );
    });
}

fn volume(page: &Page, vcx: &mut VisualTestContext) -> (u8, bool) {
    page.player.read_with(vcx, |m, _| {
        let v = m.view();
        (v.volume, v.muted)
    })
}

fn position(page: &Page, vcx: &mut VisualTestContext) -> u64 {
    page.player.read_with(vcx, |m, _| m.view().position_ms)
}

fn title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

#[gpui::test]
fn m_mutes_and_unmutes_without_losing_the_level(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(volume(&page, vcx), (70, false));
    vcx.simulate_keystrokes("m");
    assert_eq!(volume(&page, vcx), (70, true));
    vcx.simulate_keystrokes("m");
    assert_eq!(volume(&page, vcx), (70, false));
}

#[gpui::test]
fn ctrl_up_and_ctrl_down_move_the_volume_by_five_and_stop_at_the_ends(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-up");
    assert_eq!(volume(&page, vcx), (75, false));
    vcx.simulate_keystrokes("ctrl-down ctrl-down");
    assert_eq!(volume(&page, vcx), (65, false));
    vcx.simulate_keystrokes("ctrl-up ctrl-up ctrl-up ctrl-up ctrl-up ctrl-up ctrl-up ctrl-up");
    assert_eq!(volume(&page, vcx), (100, false));
    page.player.update(vcx, |m, cx| m.set_volume(3, cx));
    vcx.simulate_keystrokes("ctrl-down");
    assert_eq!(volume(&page, vcx), (0, false));
}

#[gpui::test]
fn louder_unmutes_and_quieter_stays_muted(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("m ctrl-down");
    assert_eq!(
        volume(&page, vcx),
        (70, true),
        "quieter while muted is nothing"
    );
    vcx.simulate_keystrokes("ctrl-up");
    assert_eq!(volume(&page, vcx), (75, false));
}

#[gpui::test]
fn ctrl_left_and_ctrl_right_seek_five_seconds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    open_current_load(&page, vcx);
    assert_eq!(position(&page, vcx), 0);
    vcx.simulate_keystrokes("ctrl-left");
    assert_eq!(position(&page, vcx), 0, "no further back than the start");
    vcx.simulate_keystrokes("ctrl-right ctrl-right");
    assert_eq!(position(&page, vcx), 10_000);
    vcx.simulate_keystrokes("ctrl-left");
    assert_eq!(position(&page, vcx), 5_000);
}

#[gpui::test]
fn the_plain_arrows_never_touch_playback_or_volume(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    open_current_load(&page, vcx);
    // Nothing focused, then wherever the arrows take the focus: none of it plays with the song.
    vcx.simulate_keystrokes("up down left right up down left right left left");
    assert_eq!(volume(&page, vcx), (70, false), "no volume step");
    assert_eq!(position(&page, vcx), 0, "no seek");
    assert_eq!(title(&page, vcx), "First", "no track change");
}

#[gpui::test]
fn the_first_arrow_with_nothing_focused_lands_on_the_pages_list(cx: &mut TestAppContext) {
    use apollo_app::ui::nav::Route;
    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Library, window, cx);
        s.focus_root(window, cx);
    });
    vcx.run_until_parked();
    let list = page
        .shell
        .read_with(vcx, |s, cx| gpui::Focusable::focus_handle(s.library().read(cx), cx));
    vcx.update(|window, _| assert!(!list.is_focused(window), "starts with nothing focused"));
    vcx.simulate_keystrokes("down");
    vcx.update(|window, _| assert!(list.is_focused(window), "the first arrow lands on the list"));
    assert_eq!(volume(&page, vcx), (70, false), "and does not change the volume");
}

#[gpui::test]
fn arrows_walk_the_nav_rail_and_right_goes_into_the_page(cx: &mut TestAppContext) {
    use apollo_app::ui::nav::Route;
    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Library, window, cx);
    });
    vcx.run_until_parked();
    let stops = page.shell.read_with(vcx, |s, _| s.tab_stops());
    let focused = |vcx: &mut VisualTestContext, name: &str| {
        let handle = stops.iter().find(|(n, _)| *n == name).unwrap().1.clone();
        vcx.update(|window, _| handle.is_focused(window))
    };
    page.shell.update_in(vcx, |s, window, cx| {
        let home = s.tab_stops().into_iter().find(|(n, _)| *n == "nav-home").unwrap().1;
        window.focus(&home, cx);
    });
    assert!(focused(vcx, "nav-home"));
    vcx.simulate_keystrokes("down");
    assert!(focused(vcx, "nav-explore"), "Down walks the rail");
    vcx.simulate_keystrokes("down down");
    assert!(focused(vcx, "nav-library"));
    vcx.simulate_keystrokes("up");
    assert!(focused(vcx, "nav-search"), "Up walks back");
    vcx.simulate_keystrokes("left");
    assert!(focused(vcx, "nav-search"), "Left on the rail stays");
    vcx.simulate_keystrokes("right");
    let list = page
        .shell
        .read_with(vcx, |s, cx| gpui::Focusable::focus_handle(s.library().read(cx), cx));
    vcx.update(|window, _| assert!(list.is_focused(window), "Right goes into the page"));
    vcx.simulate_keystrokes("left");
    assert!(focused(vcx, "nav-library"), "Left from the list is the rail's button for the page");
    assert_eq!(volume(&page, vcx), (70, false));
}

#[gpui::test]
fn shift_right_and_shift_left_change_track(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(title(&page, vcx), "First");
    vcx.simulate_keystrokes("shift-right");
    assert_eq!(title(&page, vcx), "Second");
    vcx.simulate_keystrokes("shift-left");
    assert_eq!(title(&page, vcx), "First");
}

#[gpui::test]
fn ctrl_l_shows_and_hides_lyrics(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell
        .read_with(vcx, |s, cx| assert!(!s.lyrics_showing(cx)));
    vcx.simulate_keystrokes("ctrl-l");
    page.shell
        .read_with(vcx, |s, cx| assert!(s.lyrics_showing(cx)));
    vcx.simulate_keystrokes("ctrl-l");
    page.shell
        .read_with(vcx, |s, cx| assert!(!s.lyrics_showing(cx)));
}

#[gpui::test]
fn l_likes_the_playing_song_and_again_takes_it_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("shift-right");
    assert_eq!(title(&page, vcx), "Second");
    assert!(!page.sim.liked_ids().contains(&SECOND.to_owned()));
    vcx.simulate_keystrokes("l");
    assert!(page.sim.liked_ids().contains(&SECOND.to_owned()));
    vcx.simulate_keystrokes("l");
    assert!(!page.sim.liked_ids().contains(&SECOND.to_owned()));
}

#[gpui::test]
fn the_plain_keys_are_letters_and_cursor_moves_in_a_text_field(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    let liked_before = page.sim.liked_ids().len();
    vcx.simulate_keystrokes("m l up left right");
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "ml");
    assert_eq!(volume(&page, vcx), (70, false), "no mute, no volume step");
    assert_eq!(page.sim.liked_ids().len(), liked_before, "no like");
    assert_eq!(title(&page, vcx), "First", "no track change");
}

#[gpui::test]
fn global_hotkeys_drive_the_player(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let apply = |command: GlobalCommand, vcx: &mut VisualTestContext| {
        page.player.update(vcx, |m, cx| command.apply(m, cx));
    };
    apply(GlobalCommand::PlayPause, vcx);
    assert_eq!(
        page.player
            .read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
    apply(GlobalCommand::PlayPause, vcx);
    assert_eq!(
        page.player
            .read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Paused
    );
    apply(GlobalCommand::Next, vcx);
    assert_eq!(title(&page, vcx), "Second");
    apply(GlobalCommand::Previous, vcx);
    assert_eq!(title(&page, vcx), "First");
    apply(GlobalCommand::VolumeUp, vcx);
    assert_eq!(volume(&page, vcx), (75, false));
    apply(GlobalCommand::VolumeDown, vcx);
    apply(GlobalCommand::VolumeDown, vcx);
    assert_eq!(volume(&page, vcx), (65, false));
}

#[gpui::test]
fn the_plain_keys_leave_an_open_menu_alone(cx: &mut TestAppContext) {
    use apollo_app::ui::track_menu::{self, Anchor, Subject};
    use gpui::{point, px};

    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |_, window, cx| {
        track_menu::open(
            Subject::track(apollo_domain::model::Track::new("menuTrack1", "Menu"), None),
            Anchor::Point(point(px(300.), px(300.))),
            window,
            cx,
        );
    });
    vcx.run_until_parked();
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    assert!(menu.read_with(vcx, |m, _| m.is_open()), "the menu is open");
    let liked_before = page.sim.liked_ids().len();
    vcx.simulate_keystrokes("m l left right");
    assert_eq!(volume(&page, vcx), (70, false), "no mute");
    assert_eq!(page.sim.liked_ids().len(), liked_before, "no like");
    assert_eq!(position(&page, vcx), 0, "no seek");
}
