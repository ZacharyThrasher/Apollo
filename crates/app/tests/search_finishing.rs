//! Search finishing on the headless UI: the chip row, the Top result card beside the top songs,
//! "Show all" jumping to a chip, and the filtered lists paging as they run out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::account::AccountModel;
use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
use apollo_app::search::{More, SearchChip, SearchSource, SearchState, DEBOUNCE};
use apollo_app::settings::{SettingsBackend, SettingsModel};
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{
    Hit, ListedTrack, SearchFilter, SearchPage, SearchResults, TopResult,
};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl SettingsBackend for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
}

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}
struct NoThumbs;
impl ThumbSource for NoThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

fn song(id: &str, title: &str) -> ListedTrack {
    ListedTrack {
        track: Track::new(id, title),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

fn hit(id: &str, title: &str) -> Hit {
    Hit {
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: None,
        thumbnail: None,
        explicit: false,
    }
}

/// The mixed answer: the artist on top, six songs, two albums, an artist and a playlist. A
/// filtered songs list is three pages of three; every other filtered list is one page of two.
#[derive(Default)]
struct Script {
    mixed: Mutex<Vec<String>>,
    filtered: Mutex<Vec<(String, SearchFilter)>>,
    more: Mutex<Vec<String>>,
    /// The second page of the songs list fails once.
    fail_page_two: Mutex<bool>,
    /// What the top result is; `None` leaves it out.
    top: Mutex<Option<TopResult>>,
}

impl Script {
    fn new() -> Self {
        let script = Script::default();
        *script.top.lock().unwrap() = Some(TopResult::Artist(hit("UCtop", "The Artist")));
        script
    }
}

fn songs_page(from: usize, token: Option<&str>) -> SearchPage {
    SearchPage {
        results: SearchResults {
            songs: (from..from + 3)
                .map(|i| song(&format!("song{i:07}"), &format!("Song {i}")))
                .collect(),
            ..SearchResults::default()
        },
        continuation: token.map(str::to_owned),
    }
}

impl SearchSource for Script {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        self.mixed.lock().unwrap().push(query.to_owned());
        Ok(SearchResults {
            top: self.top.lock().unwrap().clone(),
            songs: (0..6)
                .map(|i| song(&format!("song{i:07}"), &format!("Song {i}")))
                .collect(),
            albums: vec![hit("MPREb_a", "Album A"), hit("MPREb_b", "Album B")],
            artists: vec![hit("UCtop", "The Artist")],
            playlists: vec![hit("VLPLx", "A Playlist")],
            ..SearchResults::default()
        })
    }

    fn search_filtered(
        &self,
        query: &str,
        filter: SearchFilter,
    ) -> Result<SearchPage, LoadFailure> {
        self.filtered
            .lock()
            .unwrap()
            .push((query.to_owned(), filter));
        Ok(match filter {
            SearchFilter::Songs => songs_page(0, Some("page-2")),
            SearchFilter::Albums => SearchPage {
                results: SearchResults {
                    albums: vec![hit("MPREb_a", "Album A"), hit("MPREb_b", "Album B")],
                    ..SearchResults::default()
                },
                continuation: None,
            },
            _ => SearchPage::default(),
        })
    }

    fn search_more(&self, token: &str) -> Result<SearchPage, LoadFailure> {
        self.more.lock().unwrap().push(token.to_owned());
        match token {
            "page-2" if std::mem::take(&mut *self.fail_page_two.lock().unwrap()) => {
                Err(LoadFailure::Network)
            }
            "page-2" => Ok(songs_page(3, Some("page-3"))),
            _ => Ok(songs_page(6, None)),
        }
    }
}

struct Page {
    shell: Entity<Shell>,
    player: Entity<PlayerModel>,
    script: Arc<Script>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let script = Arc::new(Script::new());
    let ports = apollo_app::backend::account::ports();
    apollo_app::backend::account::start(&ports, true);
    let account = cx.new(|cx| AccountModel::new(ports, cx));
    account.update(cx, |m, cx| m.startup_finished(cx));
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(Arc::new(Memory::default()))));
    let (shell, vcx) = cx.add_window_view({
        let (player, account, settings) = (player.clone(), account.clone(), settings.clone());
        move |_, cx| Shell::with_settings(player, account, settings, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("track000001", "Track 1"),
        }],
        art: Arc::new(NoArt),
        search: script.clone(),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(apollo_app::library::sim::SimLibrary::instant()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (
        Page {
            shell,
            player,
            script,
        },
        vcx,
    )
}

fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn on_screen(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = bounds(vcx, selector).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn model(page: &Page, vcx: &mut VisualTestContext) -> Entity<apollo_app::search::SearchModel> {
    page.shell.read_with(vcx, |s, _| s.search_model().clone())
}

/// Open Search, type `query` and let the debounce and the answer through.
fn search(page: &Page, vcx: &mut VisualTestContext, query: &str) {
    vcx.simulate_keystrokes("ctrl-k");
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text(query, cx));
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
}

fn titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let m = model(page, vcx);
    m.read_with(vcx, |m, _| {
        m.results()
            .map(|r| r.songs.iter().map(|s| s.track.title.clone()).collect())
            .unwrap_or_default()
    })
}

const CHIPS: [&str; 6] = [
    "search-chip-All",
    "search-chip-Songs",
    "search-chip-Albums",
    "search-chip-Artists",
    "search-chip-Playlists",
    "search-chip-Community playlists",
];

#[gpui::test]
fn the_chip_row_appears_with_a_query_and_not_before(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    vcx.run_until_parked();
    assert!(!on_screen(vcx, CHIPS[0]), "nothing to filter yet");

    search(&page, vcx, "daft");
    let row: Vec<_> = CHIPS.iter().map(|c| bounds(vcx, c)).collect();
    // One row: every chip on the same line, left to right.
    for pair in row.windows(2) {
        assert_eq!(pair[0].top(), pair[1].top());
        assert!(pair[0].right() <= pair[1].left());
    }
    let field = bounds(vcx, "search-field");
    assert!(row[0].top() >= field.bottom(), "the row sits under the field");
    assert_eq!(
        model(&page, vcx).read_with(vcx, |m, _| m.chip()),
        SearchChip::All
    );
}

#[gpui::test]
fn all_shows_the_top_result_beside_the_top_four_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    let (card, list) = (bounds(vcx, "top-card"), bounds(vcx, "song-like-0"));
    assert!(card.right() <= list.left(), "the card is on the left");
    for i in 0..4 {
        let id: &'static str = ["song-like-0", "song-like-1", "song-like-2", "song-like-3"][i];
        assert!(on_screen(vcx, id), "song {i} is listed");
    }
    assert!(
        !on_screen(vcx, "song-like-4"),
        "the rest are behind Show all"
    );
    // The artist on top is not listed a second time under Artists.
    assert!(
        !on_screen(vcx, "show-all-artists"),
        "an artist group holding only the top result is left out"
    );
    for id in ["show-all-songs", "show-all-albums", "show-all-playlists"] {
        assert!(on_screen(vcx, id), "{id}");
    }
}

#[gpui::test]
fn without_a_top_result_the_songs_stand_alone(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    *page.script.top.lock().unwrap() = None;
    search(&page, vcx, "daft");
    assert!(!on_screen(vcx, "top-card"));
    assert!(on_screen(vcx, "song-like-3"));
    assert!(!on_screen(vcx, "song-like-4"));
    assert!(on_screen(vcx, "show-all-artists"), "the artist group is whole");
}

#[gpui::test]
fn play_on_an_artists_top_card_plays_the_songs_beside_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "top-card-play");
    let current = page.player.read_with(vcx, |m, _| {
        m.coordinator()
            .unwrap()
            .queue()
            .current()
            .map(|e| e.track.title.clone())
    });
    assert_eq!(current.as_deref(), Some("Song 0"));
}

#[gpui::test]
fn a_song_on_top_plays_when_its_card_is_clicked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    *page.script.top.lock().unwrap() = Some(TopResult::Song(song("topsong0001", "The Best Match")));
    search(&page, vcx, "daft");
    click(vcx, "top-card");
    let current = page.player.read_with(vcx, |m, _| {
        m.coordinator()
            .unwrap()
            .queue()
            .current()
            .map(|e| e.track.title.clone())
    });
    assert_eq!(current.as_deref(), Some("The Best Match"));
}

#[gpui::test]
fn show_all_jumps_to_that_chip_and_asks_for_the_filtered_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "show-all-albums");
    let m = model(&page, vcx);
    assert_eq!(m.read_with(vcx, |m, _| m.chip()), SearchChip::Albums);
    assert_eq!(
        *page.script.filtered.lock().unwrap(),
        [("daft".to_owned(), SearchFilter::Albums)]
    );
    assert_eq!(m.read_with(vcx, |m, _| m.state()), SearchState::Ready);
    assert!(on_screen(vcx, "search-album-1"));
    assert!(!on_screen(vcx, "top-card"), "a filtered list has no top result");
}

#[gpui::test]
fn clicking_a_chip_switches_and_all_is_not_asked_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "search-chip-Songs");
    assert_eq!(page.script.filtered.lock().unwrap().len(), 1);
    click(vcx, "search-chip-All");
    assert!(on_screen(vcx, "top-card"));
    assert_eq!(
        *page.script.mixed.lock().unwrap(),
        ["daft"],
        "the mixed answer was kept"
    );
    // Back on Songs: the list is already there.
    click(vcx, "search-chip-Songs");
    assert_eq!(page.script.filtered.lock().unwrap().len(), 1);
}

#[gpui::test]
fn a_filtered_songs_list_pages_in_as_it_runs_out_and_stops_at_the_end(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "search-chip-Songs");
    // Three short pages do not fill the window, so the next is asked for until the last.
    vcx.run_until_parked();
    assert_eq!(titles(&page, vcx).len(), 9);
    assert_eq!(*page.script.more.lock().unwrap(), ["page-2", "page-3"]);
    let m = model(&page, vcx);
    assert!(!m.read_with(vcx, |m, _| m.has_more()));
    assert_eq!(m.read_with(vcx, |m, _| m.more()), More::Idle);
    for _ in 0..3 {
        vcx.run_until_parked();
    }
    assert_eq!(
        page.script.more.lock().unwrap().len(),
        2,
        "nothing is asked for past the end"
    );
}

#[gpui::test]
fn a_failed_next_page_says_so_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    *page.script.fail_page_two.lock().unwrap() = true;
    search(&page, vcx, "daft");
    click(vcx, "search-chip-Songs");
    let m = model(&page, vcx);
    assert_eq!(
        m.read_with(vcx, |m, _| m.more()),
        More::Failed(LoadFailure::Network)
    );
    assert_eq!(titles(&page, vcx).len(), 3, "the first page stays");
    assert_eq!(
        page.script.more.lock().unwrap().len(),
        1,
        "a failure is not asked again by itself"
    );
    m.update(vcx, |m, cx| m.retry_more(cx));
    vcx.run_until_parked();
    assert_eq!(titles(&page, vcx).len(), 9);
}

#[gpui::test]
fn keyboard_down_from_the_field_enters_a_filtered_songs_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "search-chip-Songs");
    vcx.run_until_parked();
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    vcx.update(|window, cx| apollo_app::ui::text_field::focus(&field, window, cx));
    vcx.simulate_keystrokes("down down down down down");
    let selected = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).selected());
    assert_eq!(selected, Some(4), "every song of the list is reachable");
}

#[gpui::test]
fn a_new_query_keeps_the_chip_and_asks_only_for_that_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    click(vcx, "search-chip-Albums");
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("punk", cx));
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    assert_eq!(
        model(&page, vcx).read_with(vcx, |m, _| m.chip()),
        SearchChip::Albums
    );
    assert_eq!(page.script.mixed.lock().unwrap().len(), 1, "no mixed search for punk");
    assert_eq!(
        page.script.filtered.lock().unwrap().last(),
        Some(&("punk".to_owned(), SearchFilter::Albums))
    );
    // The mixed answer for the old query is not stood in for the new one.
    click(vcx, "search-chip-All");
    assert_eq!(*page.script.mixed.lock().unwrap(), ["daft", "punk"]);
}

#[gpui::test]
fn the_chip_row_is_one_tab_stop_and_the_arrows_walk_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    vcx.update(|window, cx| apollo_app::ui::text_field::focus(&field, window, cx));
    // The field's clear button, then the active chip.
    vcx.simulate_keystrokes("tab tab");
    let focused = |page: &Page, vcx: &mut VisualTestContext| {
        let search_page = page.shell.read_with(vcx, |s, _| s.search_page().clone());
        vcx.update(|window, cx| search_page.read(cx).focused_chip(window))
    };
    assert_eq!(focused(&page, vcx), Some(SearchChip::All));
    vcx.simulate_keystrokes("right right");
    assert_eq!(focused(&page, vcx), Some(SearchChip::Albums));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        model(&page, vcx).read_with(vcx, |m, _| m.chip()),
        SearchChip::Albums,
        "Right twice from All, Enter picks Albums"
    );
    // The next Tab leaves the row for the results list, not onto the next chip.
    vcx.simulate_keystrokes("tab");
    vcx.simulate_keystrokes("down");
    let selected = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).selected());
    assert_eq!(selected, None, "no songs on Albums: Down finds nothing to select");
    vcx.simulate_keystrokes("shift-tab");
    vcx.simulate_keystrokes("left enter");
    vcx.run_until_parked();
    assert_eq!(
        model(&page, vcx).read_with(vcx, |m, _| m.chip()),
        SearchChip::Songs
    );
}
