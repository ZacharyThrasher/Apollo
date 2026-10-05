//! The Crate's add box: typing searches songs after a pause and Enter adds the one the keyboard
//! is on; Ctrl+V reads links and "Artist - Title" lines, with a review list for several lines;
//! every lookup honours the shared cooldown and trickles. The real shell and views on GPUI's test
//! platform, with a scripted search and library.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_add::{CrateAddModel, LineState, Status};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::source::TrackRow;
use apollo_app::library::{ArtistContent, LibrarySource};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::crate_page::CratePage;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::Collection;
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{ArtistRef, BrowseId, Track, VideoId};
use apollo_provider::{LinkTarget, ListedTrack, SearchFilter, SearchPage, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_services_common::Throttle;
use gpui::{AppContext, ClipboardItem, Entity, TestAppContext, VisualTestContext};

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

fn track(id: &str, title: &str, artist: &str) -> Track {
    Track {
        video_id: VideoId::new(id),
        title: title.into(),
        artists: vec![ArtistRef { name: artist.into(), id: None }],
        album: None,
        duration: Some(Duration::from_secs(200)),
    }
}

fn listed(id: &str, title: &str, artist: &str) -> ListedTrack {
    ListedTrack {
        track: track(id, title, artist),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

/// The songs the scripted service knows.
const SONGS: &[(&str, &str, &str)] = &[
    ("getlucky001", "Get Lucky", "Daft Punk"),
    ("getlucky002", "Get Lucky (Radio Edit)", "Daft Punk"),
    ("digital0001", "Digital Love", "Daft Punk"),
    ("bohemian001", "Bohemian Rhapsody", "Queen"),
    ("standbyme01", "Stand By Me", "Ben E. King"),
    ("wonderwall1", "Wonderwall", "Oasis"),
    ("creep000001", "Creep", "Radiohead"),
];

/// Answers searches from `SONGS` (a song matches when it holds every word of the query), writes
/// down what was asked, and can be made to say "too many requests" after some searches.
#[derive(Default)]
struct Script {
    asked: Mutex<Vec<String>>,
    links: Mutex<Vec<String>>,
    /// Searches answered before the service says "too many requests".
    busy_after: Mutex<Option<usize>>,
}

impl Script {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl SearchSource for Script {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }

    fn search_filtered(&self, query: &str, _: SearchFilter) -> Result<SearchPage, LoadFailure> {
        let mut asked = self.asked.lock().unwrap();
        if self.busy_after.lock().unwrap().is_some_and(|n| asked.len() >= n) {
            return Err(LoadFailure::Throttled);
        }
        asked.push(query.to_owned());
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let songs = SONGS
            .iter()
            .filter(|(_, title, artist)| {
                let hay = format!("{title} {artist}").to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(id, title, artist)| listed(id, title, artist))
            .collect();
        Ok(SearchPage {
            results: SearchResults { songs, ..SearchResults::default() },
            continuation: None,
        })
    }

    fn resolve_link(&self, link: &ExternalLink) -> Result<LinkTarget, LoadFailure> {
        self.links.lock().unwrap().push(format!("{link:?}"));
        match link {
            ExternalLink::Track { video, .. } => SONGS
                .iter()
                .find(|(id, _, _)| *id == video.as_str())
                .map(|(id, title, artist)| LinkTarget::Track(listed(id, title, artist)))
                .ok_or(LoadFailure::Parse),
            _ => Err(LoadFailure::Other),
        }
    }
}

/// A library with two pages of songs for any playlist or album.
#[derive(Default)]
struct Pages {
    asked: Mutex<Vec<String>>,
}

fn row(id: &str, title: &str) -> TrackRow {
    TrackRow {
        track: track(id, title, "Various"),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

impl LibrarySource for Pages {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        Ok(Vec::new())
    }

    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        self.asked.lock().unwrap().push(collection.id.as_str().to_owned());
        Ok(Page {
            items: vec![row("pl-a-000001", "Page one A"), row("pl-b-000001", "Page one B")],
            continuation: Some("next".into()),
            skipped: 0,
        })
    }

    fn more_tracks(&self, _: &str) -> Result<Page<TrackRow>, LoadFailure> {
        Ok(Page {
            items: vec![row("pl-c-000001", "Page two C")],
            continuation: None,
            skipped: 0,
        })
    }

    fn artist(&self, _: &BrowseId) -> Result<ArtistContent, LoadFailure> {
        Err(LoadFailure::Other)
    }

    fn artwork(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

struct Page_ {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
    crate_page: Entity<CratePage>,
    add: Entity<CrateAddModel>,
    script: Arc<Script>,
    library: Arc<Pages>,
}

fn setup(cx: &mut TestAppContext) -> (Page_, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let script = Arc::new(Script::default());
    let library = Arc::new(Pages::default());
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: ["queueAAAA01", "queueBBBB02"]
            .iter()
            .map(|id| LibraryTrack { track: track(id, id, "Queue") })
            .collect(),
        art: Arc::new(NoArt),
        search: script.clone(),
        thumbs: Arc::new(NoThumbs),
        library: library.clone(),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let crate_model = shell.read_with(vcx, |s, _| s.crate_model().clone());
    // This test's own record of YouTube being busy, not the process's.
    crate_model.update(vcx, |m, _| m.set_throttle(Throttle::new()));
    shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let crate_page = shell.read_with(vcx, |s, _| s.crate_page().clone());
    let add = crate_page.read_with(vcx, |p, _| p.add_model().clone());
    // No waits in a test: the executor's clock does not run by itself.
    add.update(vcx, |m, _| {
        m.set_pacing(Duration::ZERO, Duration::ZERO, Duration::from_secs(60))
    });
    (Page_ { player, shell, crate_model, crate_page, add, script, library }, vcx)
}

fn ids(page: &Page_, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    })
}

fn pill(page: &Page_, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn focus_add(page: &Page_, vcx: &mut VisualTestContext) {
    page.crate_page.update_in(vcx, |p, window, cx| p.focus_add(window, cx));
    vcx.run_until_parked();
}

fn add_has_focus(page: &Page_, vcx: &mut VisualTestContext) -> bool {
    let add_box = page.crate_page.read_with(vcx, |p, _| p.add_box().clone());
    vcx.update(|window, cx| add_box.read(cx).has_focus(window, cx))
}

fn clipboard(vcx: &mut VisualTestContext, text: &str) {
    vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned())));
}

fn undo(page: &Page_, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
}

fn visible(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn titles(page: &Page_, vcx: &mut VisualTestContext) -> Vec<String> {
    page.add.read_with(vcx, |m, _| {
        m.review()
            .map(|r| r.lines.iter().map(|l| l.title()).collect())
            .unwrap_or_default()
    })
}

// ---- typing ----------------------------------------------------------------------------------

#[gpui::test]
fn typing_lists_songs_and_enter_adds_the_one_on_and_the_field_stays_ready(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    vcx.simulate_input("daft");
    vcx.run_until_parked();
    let titles: Vec<String> = page
        .add
        .read_with(vcx, |m, _| m.results().iter().map(|r| r.track.title.clone()).collect());
    assert_eq!(titles, ["Get Lucky", "Get Lucky (Radio Edit)", "Digital Love"]);
    assert!(visible(vcx, "crate-add-popover"), "the popover lists them");
    assert!(visible(vcx, "crate-add-result-2"));

    // The first is under the keyboard; Down moves, Enter adds.
    vcx.simulate_keystrokes("down");
    assert_eq!(page.add.read_with(vcx, |m, _| m.active()), Some(1));
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["digital0001"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate \u{b7} Ctrl+Z to undo"));
    // The field is empty and still has the keyboard, the popover is gone.
    assert!(add_has_focus(&page, vcx));
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "");
    assert!(!visible(vcx, "crate-add-popover"));
    assert!(visible(vcx, "crate-add"));

    // The next one goes straight in behind it.
    vcx.simulate_input("queen");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["digital0001", "bohemian001"]);
}

#[gpui::test]
fn a_click_on_a_result_adds_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    vcx.simulate_input("creep");
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-add-result-0").unwrap().center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["creep000001"]);
}

#[gpui::test]
fn a_song_the_crate_holds_is_marked_and_adding_it_again_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    focus_add(&page, vcx);
    vcx.simulate_input("creep");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["creep000001"], "not twice");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Already in Crate"));
}

#[gpui::test]
fn search_waits_for_a_pause_in_typing_and_asks_once_for_what_was_typed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.add.update(vcx, |m, _| {
        m.set_pacing(Duration::from_millis(300), Duration::ZERO, Duration::from_secs(60))
    });
    focus_add(&page, vcx);
    vcx.simulate_input("da");
    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    assert!(page.script.asked().is_empty(), "still typing");
    vcx.simulate_input("ft");
    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    assert!(page.script.asked().is_empty(), "the pause starts over with each key");
    vcx.executor().advance_clock(Duration::from_millis(150));
    vcx.run_until_parked();
    assert_eq!(page.script.asked(), ["daft"], "one search, for the whole word");

    // Backspacing over a word already asked costs no request.
    vcx.simulate_keystrokes("backspace");
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    vcx.simulate_input("t");
    vcx.run_until_parked();
    assert_eq!(page.script.asked(), ["daft", "daf"], "only the new word is asked");
}

#[gpui::test]
fn one_letter_does_not_search(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    vcx.simulate_input("d");
    vcx.run_until_parked();
    assert!(page.script.asked().is_empty());
    assert_eq!(page.add.read_with(vcx, |m, _| m.status()), Status::Idle);
}

#[gpui::test]
fn no_songs_says_so_and_enter_adds_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    vcx.simulate_input("zzzz");
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, _| m.status()), Status::NoResults);
    assert!(visible(vcx, "crate-add-message"));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn too_many_requests_pauses_typing_without_asking_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    *page.script.busy_after.lock().unwrap() = Some(0);
    focus_add(&page, vcx);
    vcx.simulate_input("daft");
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, _| m.status()), Status::Paused);
    assert!(visible(vcx, "crate-add-message"), "the popover says YouTube is busy");
    // Typing on while it is busy asks nothing: the script would have said busy again, and the
    // model never gets that far.
    *page.script.busy_after.lock().unwrap() = None;
    vcx.simulate_input("p");
    vcx.run_until_parked();
    assert!(page.script.asked().is_empty(), "nothing asked during the cooldown");
    assert_eq!(page.add.read_with(vcx, |m, _| m.status()), Status::Paused);
}

#[gpui::test]
fn the_shared_cooldown_stops_lookups_too(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let throttle = Throttle::new();
    throttle.report_throttled(None);
    page.crate_model.update(vcx, |m, _| m.set_throttle(throttle));
    focus_add(&page, vcx);
    vcx.simulate_input("daft");
    vcx.run_until_parked();
    assert!(page.script.asked().is_empty());
    assert_eq!(page.add.read_with(vcx, |m, _| m.status()), Status::Paused);
}

#[gpui::test]
fn escape_closes_the_popover_then_gives_the_keyboard_back_to_the_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    focus_add(&page, vcx);
    vcx.simulate_input("daft");
    vcx.run_until_parked();
    assert!(visible(vcx, "crate-add-popover"));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!visible(vcx, "crate-add-popover"), "first Esc closes it");
    assert!(add_has_focus(&page, vcx));
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "daft", "the text stays");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "", "then the text goes");
    assert!(add_has_focus(&page, vcx));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!add_has_focus(&page, vcx), "and the list has the keyboard");
    assert!(page.crate_page.read_with(vcx, |p, _| p.cursor().is_some()));
}

#[gpui::test]
fn the_slash_key_puts_the_keyboard_in_the_box(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    page.crate_page.update_in(vcx, |p, window, cx| {
        p.enter(window, cx);
    });
    vcx.run_until_parked();
    assert!(!add_has_focus(&page, vcx));
    vcx.simulate_keystrokes("/");
    vcx.run_until_parked();
    assert!(add_has_focus(&page, vcx));
    // Typing a slash in the box is just a character.
    vcx.simulate_input("a/b");
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "a/b");
}

#[gpui::test]
fn down_with_nothing_open_goes_on_to_the_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    focus_add(&page, vcx);
    vcx.simulate_keystrokes("down");
    vcx.run_until_parked();
    assert!(!add_has_focus(&page, vcx));
}

#[gpui::test]
fn an_empty_crate_still_has_the_box(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(ids(&page, vcx).is_empty());
    assert!(visible(vcx, "crate-add"), "the box is on the empty page");
    focus_add(&page, vcx);
    vcx.simulate_input("queen");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["bohemian001"]);
}

// ---- pasting ---------------------------------------------------------------------------------

#[gpui::test]
fn pasting_a_song_link_onto_the_crate_adds_that_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    page.crate_page.update_in(vcx, |p, window, cx| {
        p.enter(window, cx);
    });
    clipboard(vcx, "https://music.youtube.com/watch?v=wonderwall1&si=abc123");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["creep000001", "wonderwall1"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate \u{b7} Ctrl+Z to undo"));
    assert_eq!(page.script.links.lock().unwrap().len(), 1, "one lookup for one link");
    assert!(page.add.read_with(vcx, |m, _| m.review().is_none()), "no review for one link");
}

#[gpui::test]
fn pasting_a_link_into_the_field_adds_it_too_and_leaves_the_field_empty(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, "https://youtu.be/standbyme01");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["standbyme01"]);
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "");
    assert!(add_has_focus(&page, vcx));
}

#[gpui::test]
fn a_playlist_link_adds_all_its_songs_across_pages_as_one_undo_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    page.crate_page.update_in(vcx, |p, window, cx| {
        p.enter(window, cx);
    });
    clipboard(vcx, "https://music.youtube.com/playlist?list=PLabcdefghij");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["creep000001", "pl-a-000001", "pl-b-000001", "pl-c-000001"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added 3 songs to Crate \u{b7} Ctrl+Z to undo"));
    assert_eq!(page.library.asked.lock().unwrap().as_slice(), ["VLPLabcdefghij"]);
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), ["creep000001"], "one Ctrl+Z takes the whole paste back");
}

#[gpui::test]
fn one_plain_line_pasted_into_the_field_is_typed_so_you_can_pick(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, "Daft Punk - Digital Love");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert!(ids(&page, vcx).is_empty(), "nothing added yet");
    assert_eq!(page.add.read_with(vcx, |m, _| m.query().to_owned()), "Daft Punk - Digital Love");
}

#[gpui::test]
fn one_plain_line_pasted_onto_the_crate_adds_its_best_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    page.crate_page.update_in(vcx, |p, window, cx| {
        p.enter(window, cx);
    });
    clipboard(vcx, "Digital Love by Daft Punk");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["creep000001", "digital0001"]);
    assert_eq!(page.script.asked(), ["Daft Punk Digital Love"], "one search for the line");
}

const LIST: &str = "1. Daft Punk - Get Lucky\n\
                    2. Queen - Bohemian Rhapsody\n\
                    3. Nobody - A Song Nobody Wrote\n\
                    4. https://music.youtube.com/watch?v=wonderwall1\n\
                    5. Stand By Me by Ben E. King";

#[gpui::test]
fn several_lines_open_a_review_with_each_match_and_nothing_is_added_yet(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_page.update_in(vcx, |p, window, cx| {
        p.enter(window, cx);
    });
    focus_add(&page, vcx);
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert!(ids(&page, vcx).is_empty(), "the review comes first");
    assert_eq!(
        titles(&page, vcx),
        ["Get Lucky", "Bohemian Rhapsody", "No match", "Wonderwall", "Stand By Me"]
    );
    let states: Vec<LineState> = page
        .add
        .read_with(vcx, |m, _| m.review().unwrap().lines.iter().map(|l| l.state).collect());
    assert_eq!(
        states,
        [
            LineState::Found,
            LineState::Found,
            LineState::NoMatch,
            LineState::Found,
            LineState::Found
        ]
    );
    assert!(visible(vcx, "crate-add-popover"));
    assert!(visible(vcx, "crate-add-line-4"));
    assert!(visible(vcx, "crate-add-confirm"));
    assert_eq!(page.add.read_with(vcx, |m, cx| m.addable(cx)), 4);
    // The first result of each search is the match: "Get Lucky" beat its radio edit.
    let first = page
        .add
        .read_with(vcx, |m, _| m.review().unwrap().lines[0].songs[0].track.video_id.as_str().to_owned());
    assert_eq!(first, "getlucky001");
}

#[gpui::test]
fn space_unticks_a_line_and_enter_adds_the_rest_as_one_undo_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.add(track("creep000001", "Creep", "Radiohead"), None, cx));
    vcx.run_until_parked();
    focus_add(&page, vcx);
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    // Down to Bohemian Rhapsody and take it off.
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, cx| m.addable(cx)), 3);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        ids(&page, vcx),
        ["creep000001", "getlucky001", "wonderwall1", "standbyme01"],
        "in the order pasted, without the unticked and the unmatched"
    );
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added 3 songs to Crate \u{b7} Ctrl+Z to undo"));
    assert!(page.add.read_with(vcx, |m, _| m.review().is_none()));
    assert!(add_has_focus(&page, vcx));
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), ["creep000001"], "the whole paste in one Ctrl+Z");
}

#[gpui::test]
fn a_click_on_a_line_ticks_it_and_add_confirms(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, "Daft Punk - Get Lucky\nQueen - Bohemian Rhapsody");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-add-line-0").unwrap().center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(page.add.read_with(vcx, |m, cx| m.addable(cx)), 1);
    let at = vcx.debug_bounds("crate-add-confirm").unwrap().center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["bohemian001"]);
}

#[gpui::test]
fn escape_cancels_the_review_and_adds_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(page.add.read_with(vcx, |m, _| m.review().is_none()));
    assert!(!visible(vcx, "crate-add-popover"));
    assert!(ids(&page, vcx).is_empty());
    assert!(add_has_focus(&page, vcx));
}

#[gpui::test]
fn lookups_run_two_at_a_time_with_a_pause_and_stop_when_youtube_is_busy(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.add.update(vcx, |m, _| {
        m.set_pacing(Duration::ZERO, Duration::from_millis(400), Duration::from_secs(60))
    });
    // The service answers four searches, then says "too many requests".
    *page.script.busy_after.lock().unwrap() = Some(4);
    focus_add(&page, vcx);
    let lines = ["Daft Punk - Get Lucky", "Queen - Bohemian Rhapsody", "Oasis - Wonderwall", "Radiohead - Creep", "Ben E. King - Stand By Me", "Daft Punk - Digital Love"];
    clipboard(vcx, &lines.join("\n"));
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(page.script.asked().len(), 2, "a pair, then a pause");
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    assert_eq!(page.script.asked().len(), 4, "the next pair");
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    // The fifth search was refused: everything after it waits out the cooldown.
    assert_eq!(page.script.asked().len(), 4);
    let (paused, states) = page.add.read_with(vcx, |m, _| {
        let r = m.review().unwrap();
        (r.paused, r.lines.iter().map(|l| l.state).collect::<Vec<_>>())
    });
    assert!(paused);
    assert_eq!(
        states,
        [
            LineState::Found,
            LineState::Found,
            LineState::Found,
            LineState::Found,
            LineState::Paused,
            LineState::Paused
        ]
    );
    // What was found can still be added.
    assert_eq!(page.add.read_with(vcx, |m, cx| m.addable(cx)), 4);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx).len(), 4);
    // And nothing more is asked while it is busy.
    clipboard(vcx, "Ben E. King - Stand By Me\nDaft Punk - Digital Love");
    page.crate_page.update_in(vcx, |p, window, cx| p.paste_clipboard(window, cx));
    vcx.run_until_parked();
    assert_eq!(page.script.asked().len(), 4);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Paused: YouTube is busy"));
    assert!(page.add.read_with(vcx, |m, _| m.review().is_none()));
}

#[gpui::test]
fn a_paste_reads_fifty_lines_at_most_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    let text: String = (0..60).map(|i| format!("Nobody {i} - Nothing {i}\n")).collect();
    clipboard(vcx, &text);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    let (total, left_out) = page
        .add
        .read_with(vcx, |m, _| (m.review().unwrap().total(), m.review().unwrap().left_out));
    assert_eq!((total, left_out), (50, 10));
    assert_eq!(page.script.asked().len(), 50);
    // Nothing matched, so there is nothing to add.
    assert_eq!(page.add.read_with(vcx, |m, cx| m.addable(cx)), 0);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn a_link_that_is_not_ours_and_blank_lines_are_skipped(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, "https://open.spotify.com/track/abc\n\n   \nQueen - Bohemian Rhapsody");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(titles(&page, vcx), ["No match", "Bohemian Rhapsody"]);
    assert_eq!(page.script.asked().len(), 1, "the other service's link made no request");
}

#[gpui::test]
fn the_lookups_never_touch_playback(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    vcx.simulate_input("daft");
    vcx.run_until_parked();
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    // Songs went into the Crate; the player was neither asked to play nor to prefetch them.
    let playing = page.player.read_with(vcx, |p, _| p.view().title.clone());
    assert!(!playing.contains("Get Lucky"), "{playing}");
}

#[gpui::test]
fn a_new_paste_replaces_an_unfinished_review(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    clipboard(vcx, "Queen - Bohemian Rhapsody\nOasis - Wonderwall");
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    assert_eq!(titles(&page, vcx), ["Bohemian Rhapsody", "Wonderwall"]);
}

#[gpui::test]
fn leaving_the_page_clears_the_box(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_add(&page, vcx);
    clipboard(vcx, LIST);
    vcx.simulate_keystrokes("ctrl-v");
    vcx.run_until_parked();
    page.crate_page.update(vcx, |p, cx| p.reset(cx));
    vcx.run_until_parked();
    assert!(page.add.read_with(vcx, |m, _| m.review().is_none()));
}
