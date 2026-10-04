//! Search upgrades on the headless UI: the suggestions and recents popover under the field, the
//! pasted-link path, the two Settings > Search switches, and forgetting recents on sign-out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::account::AccountModel;
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
use apollo_app::search::{
    SearchSource, SearchState, DEBOUNCE, HIDE_MUSIC_VIDEOS, SAVE_RECENT_SEARCHES, SUGGEST_DEBOUNCE,
};
use apollo_app::settings::{SettingsBackend, SettingsModel};
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_app::art::{ArtSource, DecodedArt};
use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_domain::playback::Intent;
use apollo_provider::{LinkTarget, ListedTrack, SearchResults};
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

impl Memory {
    /// The stored recents once the writer thread has caught up with `expected` (it is a real
    /// thread, so the test waits for it, briefly).
    fn recents_become(&self, expected: &str) -> String {
        for _ in 0..200 {
            let now = self.get("recent_searches").unwrap_or_default();
            if now == expected {
                return now;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.get("recent_searches").unwrap_or_default()
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

/// Remembers what it was asked. Every query finds two songs, the second a music video;
/// suggestions are ten lines "<query> 0".."<query> 9" plus a copy of the query and a repeat.
#[derive(Default)]
struct Script {
    searched: Mutex<Vec<String>>,
    suggested: Mutex<Vec<String>>,
    links: Mutex<Vec<ExternalLink>>,
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

impl SearchSource for Script {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        self.searched.lock().unwrap().push(query.to_owned());
        let mut results = SearchResults {
            songs: vec![song("audio000001", "The Song"), song("video000001", "The Video")],
            ..SearchResults::default()
        };
        results.video_ids.insert("video000001".into());
        Ok(results)
    }

    fn suggest(&self, query: &str) -> Result<Vec<String>, LoadFailure> {
        self.suggested.lock().unwrap().push(query.to_owned());
        let mut list = vec![query.to_uppercase()];
        list.extend((0..10).map(|i| format!("{query} {i}")));
        list.push(format!("{query} 0"));
        Ok(list)
    }

    fn resolve_link(&self, link: &ExternalLink) -> Result<LinkTarget, LoadFailure> {
        self.links.lock().unwrap().push(link.clone());
        match link {
            ExternalLink::Track { video, .. } if video.as_str() == "failvideo01" => {
                Err(LoadFailure::Network)
            }
            ExternalLink::Track { video, .. } => Ok(LinkTarget::Track(song(
                video.as_str(),
                "Linked Song",
            ))),
            ExternalLink::Playlist(id) => Ok(LinkTarget::Collection(Collection {
                kind: CollectionKind::Playlist,
                id: BrowseId::new(format!("VL{}", id.as_str())),
                title: "Linked Playlist".into(),
                subtitle: None,
                thumbnail: None,
            })),
            ExternalLink::Album(id) => Ok(LinkTarget::Collection(Collection {
                kind: CollectionKind::Album,
                id: id.clone(),
                title: "Linked Album".into(),
                subtitle: None,
                thumbnail: None,
            })),
            ExternalLink::Artist(id) => Ok(LinkTarget::Artist {
                id: id.clone(),
                name: "Linked Artist".into(),
                thumbnail: None,
            }),
        }
    }
}

struct Page {
    shell: Entity<Shell>,
    player: Entity<PlayerModel>,
    account: Entity<AccountModel>,
    settings: Entity<SettingsModel>,
    memory: Arc<Memory>,
    script: Arc<Script>,
}

fn setup<'a>(
    cx: &'a mut TestAppContext,
    stored_recents: &str,
) -> (Page, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let memory = Arc::new(Memory::default());
    if !stored_recents.is_empty() {
        memory.set("recent_searches", stored_recents);
    }
    let script = Arc::new(Script::default());
    let ports = apollo_app::backend::account::ports();
    apollo_app::backend::account::start(&ports, true);
    let account = cx.new(|cx| AccountModel::new(ports, cx));
    account.update(cx, |m, cx| m.startup_finished(cx));
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(memory.clone())));
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
            account,
            settings,
            memory,
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

fn rows(vcx: &mut VisualTestContext) -> usize {
    const IDS: [&str; 12] = [
        "search-suggestion-0",
        "search-suggestion-1",
        "search-suggestion-2",
        "search-suggestion-3",
        "search-suggestion-4",
        "search-suggestion-5",
        "search-suggestion-6",
        "search-suggestion-7",
        "search-suggestion-8",
        "search-suggestion-9",
        "search-suggestion-10",
        "search-suggestion-11",
    ];
    IDS.iter().filter(|id| on_screen(vcx, id)).count()
}

fn field_text(page: &Page, vcx: &mut VisualTestContext) -> String {
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.read_with(vcx, |f, _| f.text().to_owned())
}

fn set_field(page: &Page, vcx: &mut VisualTestContext, text: &str) {
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text(text, cx));
    vcx.run_until_parked();
}

fn search_state(page: &Page, vcx: &mut VisualTestContext) -> SearchState {
    page.shell
        .read_with(vcx, |s, cx| s.search_model().read(cx).state())
}

fn recents(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.shell
        .read_with(vcx, |s, cx| s.search_model().read(cx).recents().to_vec())
}

fn song_titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.search_model()
            .read(cx)
            .results()
            .map(|r| r.songs.iter().map(|s| s.track.title.clone()).collect())
            .unwrap_or_default()
    })
}

// ---- recents --------------------------------------------------------------------------------------

#[gpui::test]
fn an_empty_focused_field_lists_the_recents_under_it_and_enter_searches_the_chosen_one(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, "alpha\nbeta\ngamma");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.run_until_parked();
    assert!(on_screen(vcx, "search-popover"));
    assert_eq!(rows(vcx), 3);
    let (field, popover) = (bounds(vcx, "search-field"), bounds(vcx, "search-popover"));
    assert!(
        popover.top() >= field.bottom(),
        "the popover hangs under the field"
    );
    assert_eq!(popover.left(), field.left());

    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    assert_eq!(field_text(&page, vcx), "beta");
    assert_eq!(*page.script.searched.lock().unwrap(), ["beta"]);
    assert_eq!(search_state(&page, vcx), SearchState::Ready);
    assert!(!on_screen(vcx, "search-popover"), "a pick closes it");
    assert_eq!(recents(&page, vcx), ["beta", "alpha", "gamma"]);
    assert_eq!(page.memory.recents_become("beta\nalpha\ngamma"), "beta\nalpha\ngamma");
}

#[gpui::test]
fn at_most_ten_recents_show(cx: &mut TestAppContext) {
    let stored: Vec<String> = (0..15).map(|i| format!("q{i}")).collect();
    let (_page, vcx) = setup(cx, &stored.join("\n"));
    vcx.simulate_keystrokes("ctrl-k");
    vcx.run_until_parked();
    assert_eq!(rows(vcx), 10);
}

#[gpui::test]
fn an_x_removes_one_recent_and_clear_removes_them_all(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "alpha\nbeta\ngamma");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.run_until_parked();
    click(vcx, "search-recent-remove-1");
    assert_eq!(recents(&page, vcx), ["alpha", "gamma"]);
    assert_eq!(page.memory.recents_become("alpha\ngamma"), "alpha\ngamma");
    assert!(on_screen(vcx, "search-popover"));

    click(vcx, "search-recents-clear");
    assert!(recents(&page, vcx).is_empty());
    assert_eq!(page.memory.recents_become(""), "");
    assert!(!on_screen(vcx, "search-popover"), "nothing left to list");
}

#[gpui::test]
fn a_search_made_on_purpose_is_remembered_once_and_moves_to_the_top(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    set_field(&page, vcx, "daft punk");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    set_field(&page, vcx, "boards of canada");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    set_field(&page, vcx, "Daft Punk");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(recents(&page, vcx), ["Daft Punk", "boards of canada"]);
}

#[gpui::test]
fn typing_alone_does_not_remember_a_search(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f t");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    assert_eq!(search_state(&page, vcx), SearchState::Ready);
    assert!(recents(&page, vcx).is_empty());
}

#[gpui::test]
fn turning_save_recent_searches_off_forgets_them_and_stops_saving(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "alpha\nbeta");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.run_until_parked();
    assert_eq!(rows(vcx), 2);

    page.settings
        .update(vcx, |m, cx| m.set(&SAVE_RECENT_SEARCHES, false, cx));
    vcx.run_until_parked();
    assert!(recents(&page, vcx).is_empty());
    assert_eq!(page.memory.recents_become(""), "");
    assert!(!on_screen(vcx, "search-popover"));

    set_field(&page, vcx, "gamma");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(recents(&page, vcx).is_empty(), "nothing is saved while off");
    assert_eq!(page.memory.recents_become(""), "");
}

#[gpui::test]
fn signing_out_forgets_the_recents(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "alpha\nbeta");
    assert_eq!(recents(&page, vcx), ["alpha", "beta"]);
    page.account.update(vcx, |m, cx| m.sign_out(cx));
    vcx.run_until_parked();
    assert!(recents(&page, vcx).is_empty());
    assert_eq!(page.memory.recents_become(""), "");
}

// ---- suggestions -----------------------------------------------------------------------------------

#[gpui::test]
fn suggestions_wait_for_a_pause_of_150_ms_and_show_at_most_eight_rows(cx: &mut TestAppContext) {
    assert_eq!(SUGGEST_DEBOUNCE.as_millis(), 150);
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE / 2);
    vcx.run_until_parked();
    assert!(page.script.suggested.lock().unwrap().is_empty(), "still typing");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    assert_eq!(*page.script.suggested.lock().unwrap(), ["daf"]);
    assert_eq!(rows(vcx), 8, "eight rows, no copy of the typed text, no repeats");
    assert!(
        page.script.searched.lock().unwrap().is_empty(),
        "suggesting is not searching"
    );
}

#[gpui::test]
fn arrows_choose_a_suggestion_and_enter_searches_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("down down down up");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(field_text(&page, vcx), "daf 1");
    assert_eq!(*page.script.searched.lock().unwrap(), ["daf 1"]);
    assert_eq!(recents(&page, vcx), ["daf 1"]);
    assert!(!on_screen(vcx, "search-popover"));
}

#[gpui::test]
fn enter_with_no_row_chosen_searches_what_was_typed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(field_text(&page, vcx), "daf");
    assert_eq!(*page.script.searched.lock().unwrap(), ["daf"]);
    assert!(!on_screen(vcx, "search-popover"));
}

#[gpui::test]
fn a_click_on_a_suggestion_searches_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    click(vcx, "search-suggestion-2");
    assert_eq!(field_text(&page, vcx), "daf 2");
    assert_eq!(*page.script.searched.lock().unwrap(), ["daf 2"]);
}

#[gpui::test]
fn escape_closes_the_popover_first_then_clears_then_leaves(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f");
    vcx.executor().advance_clock(SUGGEST_DEBOUNCE);
    vcx.run_until_parked();
    assert!(on_screen(vcx, "search-popover"));

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "search-popover"));
    assert_eq!(field_text(&page, vcx), "daf", "the first Escape only closes it");

    vcx.simulate_keystrokes("escape");
    assert_eq!(field_text(&page, vcx), "", "the next one clears the field");

    vcx.simulate_keystrokes("x");
    vcx.run_until_parked();
    assert_eq!(field_text(&page, vcx), "x");
    assert!(
        page.player
            .read_with(vcx, |m, _| m.coordinator().unwrap().intent())
            == Intent::Paused
    );
}

#[gpui::test]
fn down_goes_to_the_results_once_the_popover_is_closed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("d a f t");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter down");
    let selected = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).selected());
    assert_eq!(selected, Some(0));
}

// ---- hide music videos -------------------------------------------------------------------------------

#[gpui::test]
fn hide_music_videos_filters_the_results_in_place_without_asking_again(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    set_field(&page, vcx, "daft");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(song_titles(&page, vcx), ["The Song", "The Video"]);

    page.settings
        .update(vcx, |m, cx| m.set(&HIDE_MUSIC_VIDEOS, true, cx));
    vcx.run_until_parked();
    assert_eq!(song_titles(&page, vcx), ["The Song"]);

    page.settings
        .update(vcx, |m, cx| m.set(&HIDE_MUSIC_VIDEOS, false, cx));
    vcx.run_until_parked();
    assert_eq!(song_titles(&page, vcx), ["The Song", "The Video"]);
    assert_eq!(*page.script.searched.lock().unwrap(), ["daft"], "one request");
}

#[gpui::test]
fn the_search_section_of_settings_has_its_two_switches_with_the_stated_defaults(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, "");
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
    let save = bounds(vcx, "setting-save-recent-searches");
    let hide = bounds(vcx, "setting-hide-music-videos");
    let gapless = bounds(vcx, "setting-gapless-playback");
    let close = bounds(vcx, "setting-close-to-tray");
    assert!(save.top() > gapless.top(), "after Playback");
    assert!(hide.top() > save.top());
    assert!(close.top() > hide.top(), "before Window");
    assert!(page.settings.read_with(vcx, |m, _| m.get(&SAVE_RECENT_SEARCHES)));
    assert!(!page.settings.read_with(vcx, |m, _| m.get(&HIDE_MUSIC_VIDEOS)));

    click(vcx, "setting-hide-music-videos");
    assert!(page.settings.read_with(vcx, |m, _| m.get(&HIDE_MUSIC_VIDEOS)));
}

// ---- pasted links ------------------------------------------------------------------------------------

const TRACK_LINK: &str = "https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc";

#[gpui::test]
fn a_pasted_track_link_plays_on_enter_and_is_never_searched(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    set_field(&page, vcx, TRACK_LINK);
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    assert!(page.script.searched.lock().unwrap().is_empty(), "typing a link searches nothing");
    assert!(page.script.suggested.lock().unwrap().is_empty());

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(page.script.searched.lock().unwrap().is_empty());
    let (title, intent) = page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        (
            c.queue().current().map(|e| e.track.title.clone()),
            c.intent(),
        )
    });
    assert_eq!(title.as_deref(), Some("Linked Song"));
    assert_eq!(intent, Intent::Playing);
    assert!(recents(&page, vcx).is_empty(), "a link is not a recent search");
}

#[gpui::test]
fn a_link_without_its_scheme_opens_too(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    set_field(&page, vcx, "youtu.be/dQw4w9WgXcQ");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(page.script.links.lock().unwrap().len(), 1);
}

#[gpui::test]
fn pasted_playlist_album_and_artist_links_open_their_pages(cx: &mut TestAppContext) {
    for link in [
        "https://music.youtube.com/playlist?list=PL1234567890",
        "https://music.youtube.com/browse/MPREb_abc123",
        "https://music.youtube.com/channel/UCabc1234567",
    ] {
        let (page, vcx) = setup(cx, "");
        vcx.simulate_keystrokes("ctrl-k");
        set_field(&page, vcx, link);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(
            page.shell.read_with(vcx, |s, _| s.route()),
            Route::Library,
            "{link}"
        );
        assert!(page.script.searched.lock().unwrap().is_empty(), "{link}");
    }
}

#[gpui::test]
fn a_link_that_cannot_be_opened_says_so_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, "");
    vcx.simulate_keystrokes("ctrl-k");
    set_field(&page, vcx, "https://www.youtube.com/@someone");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(search_state(&page, vcx), SearchState::BadLink);
    assert!(page.script.links.lock().unwrap().is_empty());

    set_field(&page, vcx, "https://youtu.be/failvideo01");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        search_state(&page, vcx),
        SearchState::Failed(LoadFailure::Network)
    );
    page.shell.update(vcx, |s, cx| {
        s.search_model().update(cx, |m, cx| m.retry(cx))
    });
    vcx.run_until_parked();
    assert_eq!(page.script.links.lock().unwrap().len(), 2, "the link, not a search");
    assert!(page.script.searched.lock().unwrap().is_empty());
}
