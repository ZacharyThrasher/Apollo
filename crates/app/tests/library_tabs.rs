//! Headless tests for the Library's Artists and History tabs, the sort control and the filter
//! box of a playlist or album page: the real model and view on GPUI's test platform, with the
//! simulated library (no latency).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::browse::SortKey;
use apollo_app::library::model::{LibraryModel, Load, Tab};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::library::{LibraryEdits, LibraryWrites};
use apollo_app::player::{PlayerModel, QueueHeading};
use apollo_app::settings::LibrarySortStore;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::library::LIKED_MUSIC_BROWSE_ID;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

/// A sort store that keeps its choices in memory.
#[derive(Default)]
struct MemSorts(Mutex<HashMap<String, String>>);

impl LibrarySortStore for MemSorts {
    fn load(&self, tab: &str) -> Option<String> {
        self.0.lock().unwrap().get(tab).cloned()
    }
    fn save(&self, tab: &str, sort: &str) {
        self.0.lock().unwrap().insert(tab.into(), sort.into());
    }
}

fn prepared(library: Arc<SimLibrary>) -> Prepared {
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
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(apollo_app::url_thumbs::NoThumbs),
        library,
    }
}

struct Page {
    player: Entity<PlayerModel>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
    edits: Entity<LibraryEdits>,
    sim: Arc<SimLibrary>,
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
        s.show_route(Route::Library, cx);
    });
    let sim = Arc::new(SimLibrary::instant());
    player.update(vcx, |m, cx| m.install(prepared(sim.clone()), cx));
    vcx.run_until_parked();
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    let edits = shell.read_with(vcx, |s, _| s.edits().clone());
    (
        Page {
            player,
            view,
            model,
            edits,
            sim,
        },
        vcx,
    )
}

fn titles(page: &Page, vcx: &mut VisualTestContext, tab: Tab) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.list(tab).iter().map(|c| c.title.clone()).collect()
    })
}

fn artist_names(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.artist_list().iter().map(|h| h.title.clone()).collect()
    })
}

fn current_title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on the page"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn open(page: &Page, vcx: &mut VisualTestContext, id: &str) {
    let c = page.model.read_with(vcx, |m, _| {
        m.list(Tab::Playlists)
            .iter()
            .chain(m.list(Tab::Albums))
            .find(|c| c.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("no collection {id}"))
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

// ---- Artists and History ---------------------------------------------------------------------

#[gpui::test]
fn the_artists_tab_loads_when_it_is_opened_in_the_service_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.model.read_with(vcx, |m, _| {
        assert!(
            matches!(m.artists(), Load::Idle),
            "nothing is fetched until the tab opens"
        );
    });
    click(vcx, "library-tab-artists");
    assert_eq!(
        artist_names(&page, vcx),
        [
            "Verdant",
            "Unknown",
            "Monochrome",
            "An Artist With A Similarly Long Name, Featuring Several Other Collaborators",
            "Soft Lantern",
            "Neon Arcade"
        ]
    );
}

#[gpui::test]
fn the_history_tab_groups_rows_under_the_services_own_headings(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-history");
    page.model.read_with(vcx, |m, _| {
        let Load::Ready(groups) = m.history() else {
            panic!("history is loaded");
        };
        let shape: Vec<(&str, usize)> = groups
            .iter()
            .map(|g| (g.title.as_str(), g.rows.len()))
            .collect();
        assert_eq!(
            shape,
            [
                ("Today", 3),
                ("Yesterday", 2),
                ("This week", 2),
                ("Older", 1)
            ]
        );
        assert_eq!(m.history_count(), 8);
        assert_eq!(m.history_at(1).unwrap().track.title, "Glass Garden");
    });
    // The history is a log: no sort control.
    assert!(vcx.debug_bounds("library-sort").is_none());
    let drawn = page.view.read_with(vcx, |v, _| v.rendered_rows());
    assert_eq!(drawn, 0..8);
}

#[gpui::test]
fn clicking_a_history_row_plays_that_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-history");
    click(vcx, "library-row-1");
    assert_eq!(current_title(&page, vcx), "Glass Garden");
}

#[gpui::test]
fn enter_on_a_history_row_plays_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-history");
    page.view.update_in(vcx, |v, window, cx| {
        window.focus(&gpui::Focusable::focus_handle(v, cx), cx)
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
}

#[gpui::test]
fn a_failed_history_offers_retry_and_a_refresh_failure_keeps_the_rows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_tabs_failure(Some(LoadFailure::Network));
    click(vcx, "library-tab-history");
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(m.history(), Load::Failed(LoadFailure::Network)));
    });
    page.sim.set_tabs_failure(None);
    page.model.update(vcx, |m, cx| m.reload_history(cx));
    vcx.run_until_parked();
    page.model
        .read_with(vcx, |m, _| assert_eq!(m.history_count(), 8));
    // Coming back to the tab fetches again; a failure then leaves what was shown.
    click(vcx, "library-tab-playlists");
    page.sim.set_tabs_failure(Some(LoadFailure::Network));
    click(vcx, "library-tab-history");
    page.model
        .read_with(vcx, |m, _| assert_eq!(m.history_count(), 8));
}

#[gpui::test]
fn a_failed_artists_tab_can_be_retried(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_tabs_failure(Some(LoadFailure::Session));
    click(vcx, "library-tab-artists");
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(m.artists(), Load::Failed(LoadFailure::Session)));
    });
    page.sim.set_tabs_failure(None);
    page.model.update(vcx, |m, cx| m.reload_artists(cx));
    vcx.run_until_parked();
    assert_eq!(artist_names(&page, vcx).len(), 6);
}

// ---- sort ------------------------------------------------------------------------------------

#[gpui::test]
fn the_sort_button_opens_a_popover_and_choosing_an_order_sorts_the_tab(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-albums");
    assert_eq!(
        titles(&page, vcx, Tab::Albums),
        ["Night Shift", "Greenhouse", "Long Names", "Greys"],
        "recently added is the service's order"
    );
    click(vcx, "library-sort");
    assert!(page.view.read_with(vcx, |v, _| v.sort_open()));
    assert!(vcx.debug_bounds("library-sort-menu").is_some());
    // Recently added, A-Z and Artist.
    for ix in 0..3 {
        let id = [
            "library-sort-item-0",
            "library-sort-item-1",
            "library-sort-item-2",
        ][ix];
        assert!(vcx.debug_bounds(id).is_some(), "{id} is offered");
    }
    click(vcx, "library-sort-item-1");
    assert!(
        !page.view.read_with(vcx, |v, _| v.sort_open()),
        "choosing closes it"
    );
    assert_eq!(
        titles(&page, vcx, Tab::Albums),
        ["Greenhouse", "Greys", "Long Names", "Night Shift"]
    );
    click(vcx, "library-sort");
    click(vcx, "library-sort-item-2");
    // By artist: "An Artist With ...", Monochrome, Neon Arcade, Soft Lantern.
    assert_eq!(
        titles(&page, vcx, Tab::Albums),
        ["Long Names", "Greys", "Night Shift", "Greenhouse"]
    );
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.sort(Tab::Albums), SortKey::Artist);
        assert_eq!(
            m.sort(Tab::Playlists),
            SortKey::Recent,
            "each tab keeps its own"
        );
    });
}

#[gpui::test]
fn playlists_sort_a_to_z_with_liked_music_pinned_and_offer_no_artist_order(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-sort");
    assert!(vcx.debug_bounds("library-sort-item-1").is_some());
    assert!(
        vcx.debug_bounds("library-sort-item-2").is_none(),
        "only albums sort by artist"
    );
    click(vcx, "library-sort-item-1");
    let names = titles(&page, vcx, Tab::Playlists);
    assert_eq!(names[0], "Liked Music");
    let rest: Vec<String> = names[1..].iter().map(|n| n.to_lowercase()).collect();
    let mut sorted = rest.clone();
    sorted.sort();
    assert_eq!(rest, sorted);
}

#[gpui::test]
fn the_popover_works_from_the_keyboard_and_escape_closes_only_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-albums");
    click(vcx, "library-sort");
    vcx.simulate_keystrokes("down down");
    vcx.simulate_keystrokes("escape");
    assert!(!page.view.read_with(vcx, |v, _| v.sort_open()));
    assert_eq!(
        page.model.read_with(vcx, |m, _| m.sort(Tab::Albums)),
        SortKey::Recent,
        "Esc closes without choosing"
    );
    click(vcx, "library-sort");
    vcx.simulate_keystrokes("down enter");
    assert_eq!(
        page.model.read_with(vcx, |m, _| m.sort(Tab::Albums)),
        SortKey::Title
    );
}

#[gpui::test]
fn a_click_outside_closes_the_popover(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-sort");
    assert!(page.view.read_with(vcx, |v, _| v.sort_open()));
    vcx.simulate_click(
        gpui::point(gpui::px(500.0), gpui::px(600.0)),
        Modifiers::none(),
    );
    vcx.run_until_parked();
    assert!(!page.view.read_with(vcx, |v, _| v.sort_open()));
}

#[gpui::test]
fn the_artists_tab_sorts_a_to_z(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-artists");
    click(vcx, "library-sort");
    assert!(vcx.debug_bounds("library-sort-item-2").is_none());
    click(vcx, "library-sort-item-1");
    let names = artist_names(&page, vcx);
    assert_eq!(
        names[0],
        "An Artist With A Similarly Long Name, Featuring Several Other Collaborators"
    );
    assert_eq!(names[5], "Verdant");
}

#[gpui::test]
fn sort_choices_are_saved_and_come_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let store = Arc::new(MemSorts::default());
    page.model
        .update(vcx, |m, cx| m.install_sort_store(Some(store.clone()), cx));
    vcx.run_until_parked();
    page.model
        .update(vcx, |m, cx| m.set_sort(Tab::Albums, SortKey::Artist, cx));
    // A sort the tab does not offer is refused, and History keeps none.
    page.model
        .update(vcx, |m, cx| m.set_sort(Tab::Playlists, SortKey::Artist, cx));
    page.model
        .update(vcx, |m, cx| m.set_sort(Tab::History, SortKey::Title, cx));
    vcx.run_until_parked();
    {
        let saved = store.0.lock().unwrap();
        assert_eq!(saved.get("albums").map(String::as_str), Some("artist"));
        assert!(!saved.contains_key("playlists") && !saved.contains_key("history"));
    }
    // A fresh model reads it back, whatever was loaded before.
    let writes = vcx.new(|cx| LibraryWrites::new(page.player.clone(), cx));
    let edits = vcx.new(|_| LibraryEdits::new(page.player.clone(), writes.clone()));
    let fresh = vcx.new(|cx| LibraryModel::new(page.player.clone(), writes, edits, cx));
    fresh.update(vcx, |m, cx| m.install_sort_store(Some(store.clone()), cx));
    vcx.run_until_parked();
    fresh.read_with(vcx, |m, _| {
        assert_eq!(m.sort(Tab::Albums), SortKey::Artist);
        assert_eq!(m.sort(Tab::Playlists), SortKey::Recent);
    });
}

// ---- filter ----------------------------------------------------------------------------------

#[gpui::test]
fn the_filter_keeps_rows_by_title_artist_or_album_and_plays_the_right_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    assert!(
        vcx.debug_bounds("library-filter").is_some(),
        "the header has a filter"
    );
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field");
    let count = |page: &Page, vcx: &mut VisualTestContext| {
        page.view.read_with(vcx, |v, cx| v.visible_count(cx))
    };
    assert_eq!(count(&page, vcx), 6);
    for (needle, kept) in [
        ("ash", 1),
        ("monochrome", 1),
        ("greenhouse", 1),
        ("zzz", 0),
        ("", 6),
    ] {
        field.update(vcx, |f, cx| f.set_text(needle, cx));
        vcx.run_until_parked();
        assert_eq!(count(&page, vcx), kept, "filtering by {needle:?}");
    }
    // Row 0 of the filtered list is the fourth loaded row: it plays that one, with the whole
    // list queued.
    field.update(vcx, |f, cx| f.set_text("ash", cx));
    vcx.run_until_parked();
    click(vcx, "library-row-0");
    assert_eq!(current_title(&page, vcx), "Ash");
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        assert_eq!(
            q.history().len(),
            3,
            "the whole list is queued, from the real position"
        );
    });
}

#[gpui::test]
fn esc_clears_a_filter_before_it_closes_the_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field");
    field.update(vcx, |f, cx| f.set_text("ash", cx));
    vcx.run_until_parked();
    // From the list.
    page.view.update_in(vcx, |v, window, cx| {
        window.focus(&gpui::Focusable::focus_handle(v, cx), cx)
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "");
    page.model.read_with(vcx, |m, _| {
        assert!(m.detail().is_some(), "still on the page")
    });
    vcx.simulate_keystrokes("escape");
    page.model
        .read_with(vcx, |m, _| assert!(m.detail().is_none(), "then it closes"));
}

#[gpui::test]
fn esc_in_the_filter_field_clears_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field");
    page.view.update_in(vcx, |_, window, cx| {
        apollo_app::ui::text_field::focus(&field, window, cx)
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("a s h");
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "ash");
    assert_eq!(page.view.read_with(vcx, |v, cx| v.visible_count(cx)), 1);
    vcx.simulate_keystrokes("escape");
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "");
    assert_eq!(page.view.read_with(vcx, |v, cx| v.visible_count(cx)), 6);
    page.model
        .read_with(vcx, |m, _| assert!(m.detail().is_some()));
}

#[gpui::test]
fn a_filter_with_no_matches_says_so_and_clears(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, LIKED_MUSIC_BROWSE_ID);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field");
    field.update(vcx, |f, cx| f.set_text("zzz", cx));
    vcx.run_until_parked();
    click(vcx, "library-notice-action");
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "");
    assert_eq!(page.view.read_with(vcx, |v, cx| v.visible_count(cx)), 6);
}

#[gpui::test]
fn a_long_playlist_filters_without_loading_it_all_into_view(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open(&page, vcx, sim::LONG_ID);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field");
    field.update(vcx, |f, cx| f.set_text("track 2501", cx));
    vcx.run_until_parked();
    assert_eq!(page.view.read_with(vcx, |v, cx| v.visible_count(cx)), 1);
    click(vcx, "library-row-0");
    assert_eq!(current_title(&page, vcx), "Generated Track 2501");
}

#[gpui::test]
fn an_artist_card_opens_the_artist_and_back_returns_to_the_artists_tab(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-artists");
    // The first card is the newest artist ("Verdant"); open the one the simulation has a page for.
    let target = page.model.read_with(vcx, |m, _| {
        let hit = m
            .artist_list()
            .iter()
            .find(|h| h.title == "Neon Arcade")
            .cloned()
            .expect("Neon Arcade is in the library");
        apollo_app::library::model::ArtistTarget {
            id: hit.id,
            name: hit.title,
            thumbnail: hit.thumbnail,
        }
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open_artist(target, window, cx));
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert_eq!(
            m.artist().expect("the artist page is open").name(),
            "Neon Arcade"
        );
    });
    vcx.simulate_keystrokes("escape");
    page.model.read_with(vcx, |m, _| {
        assert!(m.artist().is_none());
        assert_eq!(m.tab(), Tab::Artists, "back lands on the tab it left");
    });
}

// ---- Songs -----------------------------------------------------------------------------------

fn song_titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.list_rows().iter().map(|r| r.track.title.clone()).collect()
    })
}

fn song_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.list_rows()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

#[test]
fn the_songs_tab_sits_after_the_others_and_before_stats() {
    let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(
        labels,
        ["Playlists", "Albums", "Artists", "History", "Songs", "Stats"]
    );
}

#[gpui::test]
fn the_songs_tab_loads_when_it_is_opened_and_lists_the_saved_songs_most_recent_first(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    assert!(song_ids(&page, vcx).is_empty(), "nothing is fetched until the tab opens");
    click(vcx, "library-tab-songs");
    assert_eq!(song_ids(&page, vcx), page.sim.library_song_ids());
    assert_eq!(song_ids(&page, vcx).len(), 5);
    assert_eq!(page.view.read_with(vcx, |v, _| v.rendered_rows()), 0..5);
    // The tab row carries the sort control and a filter, as a playlist page does.
    assert!(vcx.debug_bounds("library-sort").is_some());
    assert!(vcx.debug_bounds("library-filter").is_some());
}

// Reported from the PC check: the Songs tab's menu offered "Save to library" on one row and no
// way out on another. Every row the tab lists is in the library, and the source is told so, on
// the first page and on every continuation page.
#[gpui::test]
fn every_song_the_songs_tab_lists_is_reported_to_the_source_as_in_the_library(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.sim.preset_long_library_songs();
    click(vcx, "library-tab-songs");
    vcx.run_until_parked();
    let listed = page.sim.library_song_ids();
    assert!(listed.len() > 50, "more than one page");
    let mut want = listed.clone();
    want.sort();
    assert_eq!(page.sim.songs_noted_saved(), want);
}

#[gpui::test]
fn clicking_a_song_plays_it_with_the_rest_of_the_list_behind_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    let titles = song_titles(&page, vcx);
    click(vcx, "library-row-1");
    assert_eq!(current_title(&page, vcx), titles[1]);
    let upcoming = page.player.read_with(vcx, |m, _| m.queue_view().upcoming.len());
    assert_eq!(upcoming, titles.len() - 2);
}

#[gpui::test]
fn the_songs_tab_sorts_a_to_z_and_by_artist_and_remembers_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let store = Arc::new(MemSorts::default());
    page.model
        .update(vcx, |m, cx| m.install_sort_store(Some(store.clone()), cx));
    click(vcx, "library-tab-songs");
    let service_order = song_titles(&page, vcx);
    click(vcx, "library-sort");
    for ix in 0..3 {
        assert!(
            vcx.debug_bounds(Box::leak(format!("library-sort-item-{ix}").into_boxed_str()))
                .is_some(),
            "item {ix}"
        );
    }
    click(vcx, "library-sort-item-1");
    let sorted = song_titles(&page, vcx);
    let mut expected = service_order.clone();
    expected.sort_by_key(|t| t.to_lowercase());
    assert_eq!(sorted, expected);
    assert_eq!(
        store.0.lock().unwrap().get("songs").map(String::as_str),
        Some("title")
    );
    click(vcx, "library-sort");
    click(vcx, "library-sort-item-0");
    assert_eq!(song_titles(&page, vcx), service_order, "back to the service's order");
    // The row clicked is the row shown, whatever the order.
    click(vcx, "library-sort");
    click(vcx, "library-sort-item-1");
    click(vcx, "library-row-0");
    assert_eq!(current_title(&page, vcx), expected[0]);
}

#[gpui::test]
fn the_songs_filter_keeps_rows_and_plays_the_right_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    let titles = song_titles(&page, vcx);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field on the Songs tab");
    let count = |page: &Page, vcx: &mut VisualTestContext| {
        page.view.read_with(vcx, |v, cx| v.visible_count(cx))
    };
    assert_eq!(count(&page, vcx), titles.len());
    let needle = titles[1].to_lowercase();
    field.update(vcx, |f, cx| f.set_text(&needle, cx));
    vcx.run_until_parked();
    assert!(count(&page, vcx) >= 1);
    click(vcx, "library-row-0");
    assert_eq!(current_title(&page, vcx), titles[1]);
    field.update(vcx, |f, cx| f.set_text("no such song anywhere", cx));
    vcx.run_until_parked();
    assert_eq!(count(&page, vcx), 0);
    // "No matches" has a Clear button.
    click(vcx, "library-notice-action");
    assert_eq!(count(&page, vcx), titles.len());
}

/// The queue after a play: the heading's label, the titles it holds (played, current, upcoming).
fn queue_of(page: &Page, vcx: &mut VisualTestContext) -> (Option<String>, Vec<String>, bool) {
    page.player.read_with(vcx, |m, _| {
        let q = m.queue_view();
        let label = match q.heading {
            Some(QueueHeading::Source(source)) => Some(source.label().to_owned()),
            _ => None,
        };
        let titles = q
            .history
            .iter()
            .chain(q.current.iter())
            .chain(q.upcoming.iter())
            .map(|r| r.title.to_string())
            .collect();
        (label, titles, m.view().shuffle)
    })
}

#[gpui::test]
fn the_songs_tab_has_play_and_shuffle_that_queue_the_list_as_library_songs(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    assert!(vcx.debug_bounds("songs-play").is_some() && vcx.debug_bounds("songs-shuffle").is_some());
    let titles = song_titles(&page, vcx);

    click(vcx, "songs-play");
    assert_eq!(current_title(&page, vcx), titles[0], "plays from the first row");
    let (label, queued, shuffle) = queue_of(&page, vcx);
    assert_eq!(label.as_deref(), Some("Library songs"));
    assert_eq!(queued.len(), titles.len());
    assert!(!shuffle);

    click(vcx, "songs-shuffle");
    let (label, queued, shuffle) = queue_of(&page, vcx);
    assert_eq!(label.as_deref(), Some("Library songs"));
    assert_eq!(queued.len(), titles.len(), "the same list, shuffled");
    assert!(shuffle);
}

#[gpui::test]
fn play_on_the_songs_tab_follows_the_chosen_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    click(vcx, "library-sort");
    click(vcx, "library-sort-item-1");
    let shown = song_titles(&page, vcx);
    click(vcx, "songs-play");
    assert_eq!(current_title(&page, vcx), shown[0]);
    assert_eq!(queue_of(&page, vcx).1, shown, "queued in the order shown");
}

#[gpui::test]
fn play_and_shuffle_on_the_songs_tab_use_only_the_rows_the_filter_keeps(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    let titles = song_titles(&page, vcx);
    let field = page
        .view
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field on the Songs tab");
    field.update(vcx, |f, cx| f.set_text(&titles[2].to_lowercase(), cx));
    vcx.run_until_parked();
    let kept = page.view.read_with(vcx, |v, cx| v.visible_count(cx));
    assert!(kept >= 1 && kept < titles.len());

    // Autoplay would top a short queue up with a radio; this is about what the buttons queue.
    page.player.update(vcx, |m, cx| m.set_autoplay(false, cx));
    click(vcx, "songs-play");
    assert_eq!(current_title(&page, vcx), titles[2]);
    let (label, queued, _) = queue_of(&page, vcx);
    assert_eq!(label.as_deref(), Some("Library songs"));
    assert_eq!(queued.len(), kept, "only what the filter keeps is queued");
    assert!(queued.contains(&titles[2]));

    click(vcx, "songs-shuffle");
    let (_, queued, shuffle) = queue_of(&page, vcx);
    assert_eq!(queued.len(), kept);
    assert!(shuffle);

    // Clearing the filter brings every row back into the next play.
    field.update(vcx, |f, cx| f.set_text("", cx));
    vcx.run_until_parked();
    click(vcx, "songs-play");
    assert_eq!(queue_of(&page, vcx).1.len(), titles.len());
}

#[gpui::test]
fn an_empty_library_says_so_and_a_failed_read_can_be_retried(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.preset_library_songs(&[]);
    click(vcx, "library-tab-songs");
    assert!(page.model.read_with(vcx, |m, _| m.songs().listing.is_confirmed_empty()));
    assert!(vcx.debug_bounds("library-filter").is_none(), "nothing to filter");
    assert!(vcx.debug_bounds("songs-play").is_none(), "nothing to play");

    page.sim.preset_library_songs(&["simTrack001"]);
    page.sim.set_tabs_failure(Some(LoadFailure::Network));
    page.model.update(vcx, |m, cx| m.reload_songs(cx));
    vcx.run_until_parked();
    assert!(page.model.read_with(vcx, |m, _| m.songs().failure().is_some()));
    page.sim.set_tabs_failure(None);
    click(vcx, "library-notice-action");
    assert_eq!(song_ids(&page, vcx), ["simTrack001"]);
}

#[gpui::test]
fn a_long_songs_list_loads_page_after_page_and_draws_only_what_is_in_view(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.sim.preset_long_library_songs();
    click(vcx, "library-tab-songs");
    assert_eq!(song_ids(&page, vcx).len(), sim::LONG_SONGS);
    let drawn = page.view.read_with(vcx, |v, _| v.rendered_rows());
    assert!(drawn.len() < 60, "only a screenful is drawn: {drawn:?}");
}

#[gpui::test]
fn a_song_saved_elsewhere_joins_the_list_and_one_removed_leaves_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    let before = song_ids(&page, vcx);
    // Saved while the tab is open: it appears at the top once the list is read again.
    page.edits
        .update(vcx, |e, cx| e.set_song_saved("savedNewSong".into(), true, cx));
    vcx.run_until_parked();
    let after = song_ids(&page, vcx);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after[0], "savedNewSong");
    // Taken out: gone at once, with no read of the list needed.
    let gone = after[2].clone();
    page.edits
        .update(vcx, |e, cx| e.set_song_saved(gone.clone(), false, cx));
    vcx.run_until_parked();
    assert!(!song_ids(&page, vcx).contains(&gone));
    assert_eq!(song_ids(&page, vcx), page.sim.library_song_ids());
}

#[gpui::test]
fn a_song_saved_while_another_tab_shows_is_there_when_the_songs_tab_opens(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    click(vcx, "library-tab-playlists");
    page.edits
        .update(vcx, |e, cx| e.set_song_saved("savedNewSong".into(), true, cx));
    vcx.run_until_parked();
    click(vcx, "library-tab-songs");
    assert_eq!(song_ids(&page, vcx)[0], "savedNewSong");
}

#[gpui::test]
fn the_track_menu_of_a_songs_row_offers_remove_from_library(cx: &mut TestAppContext) {
    use apollo_app::ui::track_menu::Command;
    let (page, vcx) = setup(cx);
    click(vcx, "library-tab-songs");
    let menu = page.view.read_with(vcx, |_, cx| {
        cx.try_global::<apollo_app::ui::track_menu::MenuHandle>().map(|h| h.0.clone())
    });
    let Some(menu) = menu else {
        panic!("the shell hosts the menu");
    };
    let at = vcx.debug_bounds("library-row-0").unwrap().center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, gpui::MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
    let commands = menu.read_with(vcx, |m, cx| m.commands(cx));
    assert!(commands.contains(&Command::RemoveFromLibrary));
    assert!(!commands.contains(&Command::SaveToLibrary));
}
