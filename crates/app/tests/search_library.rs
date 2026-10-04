//! Searching the owner's own library on the headless UI: the "In your library" group under the
//! Top result row, the Library chip, and that neither asks the service for anything.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::account::AccountModel;
use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
use apollo_app::search::{SearchChip, SearchSource, SearchState, DEBOUNCE, SUGGEST_DEBOUNCE};
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


#[gpui::test]
fn the_library_group_sits_under_the_top_result_row_and_shows_what_matched(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // The simulated library holds the album "Night Shift" and the liked song "Midnight Drive".
    search(&page, vcx, "NIGHT");
    assert!(on_screen(vcx, "library-group"), "the group shows");
    let (row, group) = (bounds(vcx, "top-card"), bounds(vcx, "library-group"));
    assert!(group.top() >= row.bottom(), "the group is under the Top result row");
    assert!(on_screen(vcx, "library-row-0"), "the album is a row");
    // The song uses the Songs group's own row, numbered after the four songs listed above it.
    assert!(on_screen(vcx, "song-like-4"), "the liked song is a row");
    assert!(on_screen(vcx, "show-all-library"));
    let m = model(&page, vcx);
    let found = m.read_with(vcx, |m, _| m.local().clone());
    assert_eq!(found.songs.len(), 1);
    assert_eq!(found.songs[0].track.title, "Midnight Drive");
    assert_eq!(found.albums.len(), 1);
}

#[gpui::test]
fn the_group_is_hidden_when_nothing_in_the_library_matches(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "daft");
    assert!(!on_screen(vcx, "library-group"));
    assert!(!on_screen(vcx, "show-all-library"));
    assert!(on_screen(vcx, "top-card"), "the service's answer is untouched");
}

#[gpui::test]
fn the_library_chip_is_last_and_lists_every_match_in_groups(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "night");
    let (last, one_before) = (
        bounds(vcx, "search-chip-Library"),
        bounds(vcx, "search-chip-Community playlists"),
    );
    assert!(last.left() >= one_before.right(), "Library ends the row");
    click(vcx, "search-chip-Library");
    let m = model(&page, vcx);
    assert_eq!(m.read_with(vcx, |m, _| m.chip()), SearchChip::Library);
    assert_eq!(m.read_with(vcx, |m, _| m.state()), SearchState::Ready);
    assert!(!on_screen(vcx, "top-card"));
    assert!(!on_screen(vcx, "library-group"));
    assert!(on_screen(vcx, "song-like-0"), "the Songs group");
    assert!(on_screen(vcx, "search-album-0"), "the Albums group");
    assert!(
        page.script.filtered.lock().unwrap().is_empty(),
        "a chip of local matches asks the service for no list"
    );
}

#[gpui::test]
fn show_all_opens_the_library_chip_without_a_request(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "night");
    click(vcx, "show-all-library");
    assert_eq!(
        model(&page, vcx).read_with(vcx, |m, _| m.chip()),
        SearchChip::Library
    );
    assert!(page.script.filtered.lock().unwrap().is_empty());
    assert_eq!(*page.script.mixed.lock().unwrap(), ["night"]);
}

#[gpui::test]
fn typing_on_the_library_chip_asks_the_service_nothing_at_all(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "night");
    click(vcx, "search-chip-Library");
    search(&page, vcx, "shift");
    assert!(on_screen(vcx, "search-album-0"));
    assert_eq!(*page.script.mixed.lock().unwrap(), ["night"], "nothing new was asked");
    assert!(page.script.filtered.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_library_matches_while_the_service_is_still_answering(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    // Load the library's lists first (the first search from an empty field does it).
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("ni", cx));
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    page.script.mixed.lock().unwrap().clear();

    field.update(vcx, |f, cx| f.set_text("night shift", cx));
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    assert!(page.script.mixed.lock().unwrap().is_empty(), "the service has not been asked");
    assert!(on_screen(vcx, "library-group"), "the library's rows are already there");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    assert_eq!(*page.script.mixed.lock().unwrap(), ["night shift"]);
    assert!(on_screen(vcx, "library-group"));
}

#[gpui::test]
fn a_library_row_opens_its_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "night shift");
    click(vcx, "library-row-0");
    let depth = page.shell.read_with(vcx, |s, cx| {
        s.library().read(cx).model().read(cx).depth()
    });
    assert_eq!(depth, 1, "the album's page is open");
}

#[gpui::test]
fn an_unlike_takes_the_song_out_of_the_library_matches(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx, "midnight");
    let m = model(&page, vcx);
    assert_eq!(m.read_with(vcx, |m, _| m.local().songs.len()), 1);
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    writes.update(vcx, |w, cx| w.toggle_like("simTrack001", cx));
    vcx.run_until_parked();
    search(&page, vcx, "midnights");
    search(&page, vcx, "midnight");
    assert_eq!(m.read_with(vcx, |m, _| m.local().songs.len()), 0);
}
