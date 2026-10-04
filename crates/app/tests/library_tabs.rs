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
use apollo_app::library::LibraryWrites;
use apollo_app::player::PlayerModel;
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
    (
        Page {
            player,
            view,
            model,
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
    let fresh = vcx.new(|cx| LibraryModel::new(page.player.clone(), writes, cx));
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
