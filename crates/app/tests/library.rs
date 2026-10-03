//! Headless tests for the Library page: the real model and view on GPUI's test platform, with
//! the simulated library (no latency) and the deterministic mock playback adapters.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{ArtSize, LibraryModel, Load, PlayRequest, Tab};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::library::{Collection, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::Intent;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Focusable, TestAppContext, VisualTestContext};

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

fn prepared() -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("startTrack01", "Start"),
        }],
        art: Arc::new(TestArt),
        library: Arc::new(SimLibrary::instant()),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| {
        s.focus_root(window, cx);
        s.go_to(Route::Library, cx);
    });
    player.update(vcx, |m, cx| m.install(prepared(), cx));
    vcx.run_until_parked();
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            player,
            shell,
            view,
            model,
        },
        vcx,
    )
}

fn collection(page: &Page, vcx: &mut VisualTestContext, id: &str) -> Collection {
    page.model.read_with(vcx, |m, _| {
        [Tab::Playlists, Tab::Albums]
            .into_iter()
            .flat_map(|t| m.list(t).iter())
            .find(|c| c.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("no collection {id}"))
    })
}

fn open(page: &Page, vcx: &mut VisualTestContext, id: &str) {
    let c = collection(page, vcx, id);
    page.view
        .update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

fn current_title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

fn queue_len(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        q.history().len() + usize::from(q.current().is_some()) + q.upcoming().count()
    })
}

#[gpui::test]
fn the_library_loads_with_liked_music_first_and_albums_separate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(m.collections(), Load::Ready(_)));
        assert!(m.list(Tab::Playlists)[0].is_liked_music());
        let liked = m
            .list(Tab::Playlists)
            .iter()
            .filter(|c| c.is_liked_music())
            .count();
        assert_eq!(liked, 1);
        assert_eq!(m.list(Tab::Albums).len(), 4);
    });
}

#[gpui::test]
fn play_all_replaces_the_queue_and_starts_the_first_track_in_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.player.update(vcx, |m, cx| m.toggle_shuffle(cx)); // shuffle was on
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
    assert_eq!(queue_len(&page, vcx), 6);
    page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        assert_eq!(c.intent(), Intent::Playing);
        assert!(!c.queue().shuffle(), "Play turns shuffle off");
    });
}

#[gpui::test]
fn shuffle_turns_shuffle_on_and_queues_every_track(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx));
    vcx.run_until_parked();
    page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        assert!(c.queue().shuffle());
        assert_eq!(c.intent(), Intent::Playing);
    });
    assert_eq!(queue_len(&page, vcx), 6);
}

#[gpui::test]
fn playing_from_a_row_starts_there_and_keeps_the_whole_list_queued(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::row(3), cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Ash");
    assert_eq!(queue_len(&page, vcx), 6);
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        assert_eq!(
            q.history().len(),
            3,
            "earlier rows stay reachable with Previous"
        );
        assert!(!q.shuffle());
    });
}

#[gpui::test]
fn a_long_playlist_pages_in_and_play_waits_for_every_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let c = collection(&page, vcx, sim::LONG_ID);
    // Ask to play row 2500 the moment the collection opens, before any page has arrived.
    page.view
        .update_in(vcx, |v, window, cx| v.open(c, window, cx));
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::row(2500), cx));
    assert_eq!(
        current_title(&page, vcx),
        "Start",
        "nothing plays from a half-loaded list"
    );
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert_eq!(d.listing.items().len(), sim::LONG_LEN);
        assert!(d.pending.is_none());
    });
    assert_eq!(current_title(&page, vcx), "Generated Track 2501");
    assert_eq!(queue_len(&page, vcx), sim::LONG_LEN);
}

#[gpui::test]
fn only_a_screenful_of_rows_is_drawn_for_three_thousand(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, sim::LONG_ID);
    vcx.run_until_parked();
    let rows = page.view.read_with(vcx, |v, _| v.rendered_rows());
    assert!(
        rows.len() > 3 && rows.len() < 60,
        "drew {rows:?} of {} rows",
        sim::LONG_LEN
    );
    assert_eq!(rows.start, 0);
}

#[gpui::test]
fn a_late_answer_for_a_closed_collection_never_lands_in_the_next_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let long = collection(&page, vcx, sim::LONG_ID);
    let liked = collection(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.model.update(vcx, |m, cx| m.open(long, cx));
    page.model.update(vcx, |m, cx| m.close(cx));
    page.model.update(vcx, |m, cx| m.open(liked, cx));
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert!(d.collection.is_liked_music());
        assert_eq!(d.listing.items().len(), 6);
    });
}

#[gpui::test]
fn a_failed_collection_shows_failure_and_retry_reloads(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, sim::BROKEN_ID);
    page.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert_eq!(d.failure(), Some(LoadFailure::Network));
        assert!(d.listing.items().is_empty());
        assert!(!d.is_loading());
    });
    page.model.update(vcx, |m, cx| m.reload_detail(cx));
    // Back in flight right after Retry, with the failure cleared.
    page.model.read_with(vcx, |m, _| {
        assert!(m.detail().unwrap().is_loading());
    });
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.detail().unwrap().failure(), Some(LoadFailure::Network));
    });
    // A failed list never starts playback.
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    assert_eq!(current_title(&page, vcx), "Start");
}

#[gpui::test]
fn an_empty_collection_is_confirmed_empty_and_play_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, sim::EMPTY_ID);
    page.model.read_with(vcx, |m, _| {
        assert!(m.detail().unwrap().listing.is_confirmed_empty());
    });
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Start");
}

#[gpui::test]
fn covers_load_for_the_rows_in_view_only_and_are_cached(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.run_until_parked();
    let rendered = page.view.read_with(vcx, |v, _| v.rendered_rows());
    assert!(!rendered.is_empty());
    let (visible, far): (Vec<String>, Vec<String>) = page.model.read_with(vcx, |m, _| {
        let list = m.list(Tab::Playlists);
        let url = |c: &Collection| c.thumbnail.clone();
        let visible: Vec<String> = list[rendered.clone()].iter().filter_map(url).collect();
        let far = list[rendered.end..]
            .iter()
            .filter_map(url)
            .filter(|u| !visible.contains(u))
            .collect();
        (visible, far)
    });
    assert!(!visible.is_empty() && !far.is_empty());
    page.model.read_with(vcx, |m, _| {
        for url in &visible {
            assert!(m.art(url, ArtSize::List).is_some(), "{url} is on screen");
        }
        for url in &far {
            assert!(!m.art_requested(url, ArtSize::List), "{url} is off screen");
        }
    });
}

#[gpui::test]
fn the_keyboard_selects_opens_and_goes_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.view
        .update_in(vcx, |v, window, cx| window.focus(&v.focus_handle(cx), cx));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("down down");
    assert_eq!(page.view.read_with(vcx, |v, _| v.selected()), 2);
    vcx.simulate_keystrokes("up");
    assert_eq!(page.view.read_with(vcx, |v, _| v.selected()), 1);
    vcx.simulate_keystrokes("end");
    let last = page
        .model
        .read_with(vcx, |m, _| m.list(Tab::Playlists).len() - 1);
    assert_eq!(page.view.read_with(vcx, |v, _| v.selected()), last);
    vcx.simulate_keystrokes("home enter");
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert!(
            m.detail().unwrap().collection.is_liked_music(),
            "Home then Enter opens Liked Music"
        );
    });
    // Enter on a track plays from it.
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Glass Garden");
    vcx.simulate_keystrokes("escape");
    page.model
        .read_with(vcx, |m, _| assert!(m.detail().is_none()));
    // Back lands on the row that was open.
    assert_eq!(page.view.read_with(vcx, |v, _| v.selected()), 0);
}

#[gpui::test]
fn choosing_library_again_returns_to_the_lists(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.view
        .update_in(vcx, |v, window, cx| v.go_home(window, cx));
    page.model
        .read_with(vcx, |m, _| assert!(m.detail().is_none()));
    page.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Library));
}

#[gpui::test]
fn the_empty_states_action_opens_search(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, sim::EMPTY_ID);
    vcx.run_until_parked();
    // Drive the same path the empty state's button takes.
    page.view.update(vcx, |_, cx| {
        cx.emit(apollo_app::ui::library::LibraryEvent::OpenSearch)
    });
    page.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Search));
}
