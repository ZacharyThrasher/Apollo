use std::sync::Mutex;

use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{Hit, ListedTrack, SearchFilter, SearchPage};
use gpui::{AppContext, Entity, TestAppContext};

use super::*;

/// Answers from a script and remembers every query it was asked.
#[derive(Default)]
struct Script {
    asked: Mutex<Vec<String>>,
    fail: Mutex<Option<LoadFailure>>,
}

impl Script {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl SearchSource for Script {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        self.asked.lock().unwrap().push(query.to_owned());
        if let Some(f) = *self.fail.lock().unwrap() {
            return Err(f);
        }
        // "nothing" finds nothing; anything else finds one song titled after the query.
        if query.starts_with("nothing") {
            return Ok(SearchResults::default());
        }
        Ok(SearchResults {
            songs: vec![ListedTrack {
                track: Track::new("aaaaaaaaaaa", query),
                occurrence: None,
                thumbnail: None,
                explicit: false,
                liked: None,
            }],
            albums: vec![Hit {
                id: BrowseId::new("MPREb_x"),
                title: query.to_owned(),
                subtitle: None,
                thumbnail: None,
                explicit: false,
            }],
            ..SearchResults::default()
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<SearchModel>, Arc<Script>) {
    let script = Arc::new(Script::default());
    let model = cx.new(|_| SearchModel::new());
    model.update(cx, |m, cx| m.install(script.clone(), cx));
    (model, script)
}

fn state(model: &Entity<SearchModel>, cx: &mut TestAppContext) -> SearchState {
    model.read_with(cx, |m, _| m.state())
}

fn song_titles(model: &Entity<SearchModel>, cx: &mut TestAppContext) -> Vec<String> {
    model.read_with(cx, |m, _| {
        m.results()
            .map(|r| r.songs.iter().map(|s| s.track.title.clone()).collect())
            .unwrap_or_default()
    })
}

#[test]
fn whitespace_is_collapsed_into_one_query() {
    assert_eq!(normalise("  daft   punk "), "daft punk");
    assert_eq!(normalise("   "), "");
}

#[test]
fn failure_text_is_six_words_or_fewer() {
    for f in [
        LoadFailure::Session,
        LoadFailure::Throttled,
        LoadFailure::Network,
        LoadFailure::Parse,
        LoadFailure::Other,
    ] {
        assert!(failure_text(f).split_whitespace().count() <= 6, "{f:?}");
    }
}

#[gpui::test]
fn typing_waits_for_a_pause_and_asks_once(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    for text in ["da", "daf", "daft"] {
        model.update(cx, |m, cx| m.set_query(text, cx));
        cx.executor().advance_clock(DEBOUNCE / 4);
        cx.run_until_parked();
    }
    assert!(script.asked().is_empty(), "still typing");
    assert_eq!(state(&model, cx), SearchState::Loading);
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(script.asked(), ["daft"]);
    assert_eq!(state(&model, cx), SearchState::Ready);
    assert_eq!(song_titles(&model, cx), ["daft"]);
}

#[gpui::test]
fn a_short_or_blank_query_clears_the_page_and_asks_nothing(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(state(&model, cx), SearchState::Ready);

    model.update(cx, |m, cx| m.set_query("d", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(state(&model, cx), SearchState::Idle);
    assert!(model.read_with(cx, |m, _| m.results().is_none()));
    assert_eq!(script.asked(), ["daft"]);
}

#[gpui::test]
fn enter_searches_at_once_even_for_one_character(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| {
        m.set_query("d", cx);
        m.submit(cx);
    });
    cx.run_until_parked();
    assert_eq!(script.asked(), ["d"]);
    assert_eq!(state(&model, cx), SearchState::Ready);
}

#[gpui::test]
fn enter_after_the_preview_does_not_ask_again(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| m.set_query("daft punk", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.submit(cx));
    cx.run_until_parked();
    assert_eq!(script.asked(), ["daft punk"]);
}

#[gpui::test]
fn typing_back_to_the_answered_query_does_not_ask_again(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.set_query("daftx", cx));
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(script.asked(), ["daft"]);
    assert_eq!(state(&model, cx), SearchState::Ready);
}

#[gpui::test]
fn an_abandoned_query_never_updates_the_view(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| {
        m.set_query("first", cx);
        m.submit(cx);
    });
    // The newer query replaces the first before it is answered.
    model.update(cx, |m, cx| {
        m.set_query("second", cx);
        m.submit(cx);
    });
    cx.run_until_parked();
    assert_eq!(song_titles(&model, cx), ["second"]);
    assert_eq!(model.read_with(cx, |m, _| m.query().to_owned()), "second");
    assert!(!script.asked().contains(&"first".to_owned()));
}

#[gpui::test]
fn the_previous_answer_stays_visible_while_the_next_loads(cx: &mut TestAppContext) {
    let (model, _script) = setup(cx);
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.set_query("daft punk", cx));
    assert_eq!(state(&model, cx), SearchState::Loading);
    assert_eq!(song_titles(&model, cx), ["daft"]);
}

#[gpui::test]
fn nothing_found_is_confirmed_empty_and_a_failure_is_not(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    model.update(cx, |m, cx| m.set_query("nothing here", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert!(model.read_with(cx, |m, _| m.is_confirmed_empty()));

    *script.fail.lock().unwrap() = Some(LoadFailure::Network);
    model.update(cx, |m, cx| m.set_query("something else", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(state(&model, cx), SearchState::Failed(LoadFailure::Network));
    assert!(!model.read_with(cx, |m, _| m.is_confirmed_empty()));
    assert!(model.read_with(cx, |m, _| m.results().is_none()));
}

#[gpui::test]
fn retry_asks_again_after_a_failure(cx: &mut TestAppContext) {
    let (model, script) = setup(cx);
    *script.fail.lock().unwrap() = Some(LoadFailure::Throttled);
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(
        state(&model, cx),
        SearchState::Failed(LoadFailure::Throttled)
    );
    *script.fail.lock().unwrap() = None;
    model.update(cx, |m, cx| m.retry(cx));
    cx.run_until_parked();
    assert_eq!(state(&model, cx), SearchState::Ready);
    assert_eq!(script.asked(), ["daft", "daft"]);
}

#[gpui::test]
fn a_query_typed_before_the_backend_is_ready_goes_out_when_it_is(cx: &mut TestAppContext) {
    let model = cx.new(|_| SearchModel::new());
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(
        state(&model, cx),
        SearchState::Loading,
        "nothing to ask yet"
    );
    let script = Arc::new(Script::default());
    model.update(cx, |m, cx| m.install(script.clone(), cx));
    cx.run_until_parked();
    assert_eq!(script.asked(), ["daft"]);
    assert_eq!(state(&model, cx), SearchState::Ready);
}

#[test]
fn only_youtube_links_count_as_links_and_a_missing_scheme_is_added() {
    assert!(as_link("daft punk").is_none());
    assert!(as_link("youtube is great").is_none());
    assert!(matches!(
        as_link("https://youtu.be/dQw4w9WgXcQ"),
        Some(Ok(ExternalLink::Track { .. }))
    ));
    assert!(matches!(
        as_link("music.youtube.com/playlist?list=PL1234567890"),
        Some(Ok(ExternalLink::Playlist(_)))
    ));
    assert!(matches!(
        as_link("https://example.test/watch?v=dQw4w9WgXcQ"),
        Some(Err(_))
    ));
    assert!(matches!(
        as_link("https://www.youtube.com/@someone"),
        Some(Err(_))
    ));
}

#[test]
fn suggestions_drop_blanks_repeats_and_the_typed_text_and_stop_at_eight() {
    let raw = ["Daft", "daft punk", "DAFT PUNK", "  ", "daft punk live"]
        .into_iter()
        .chain((0..10).map(|_| "more"))
        .map(str::to_owned)
        .collect();
    let cleaned = clean_suggestions(raw, "daft");
    assert_eq!(cleaned[..3], ["daft punk", "daft punk live", "more"]);
    let many = (0..20).map(|i| format!("daft {i}")).collect();
    assert_eq!(clean_suggestions(many, "daft").len(), MAX_SUGGESTIONS);
}

#[test]
fn recents_keep_one_copy_newest_first_and_round_trip_through_text() {
    let mut list = Vec::new();
    for q in ["a", "b", "c", "B"] {
        push_recent(&mut list, q, false);
    }
    assert_eq!(list, ["B", "c", "a"]);
    // Older stored entries merge in behind, without repeating what is there.
    push_recent(&mut list, "b", true);
    push_recent(&mut list, "z", true);
    assert_eq!(list, ["B", "c", "a", "z"]);
    assert_eq!(decode_recents(&encode_recents(&list)), list);
    assert_eq!(decode_recents("\n  \nx  y\n"), ["x y"]);
}

#[gpui::test]
fn only_twenty_recents_are_kept(cx: &mut TestAppContext) {
    let (model, _script) = setup(cx);
    for i in 0..30 {
        model.update(cx, |m, cx| m.search_for(&format!("query {i}"), cx));
    }
    cx.run_until_parked();
    model.update(cx, |m, cx| m.set_options(true, false, cx));
    let all = model.read_with(cx, |m, _| m.recents().to_vec());
    assert_eq!(all.len(), SHOWN_RECENTS, "the popover shows ten");
    assert_eq!(all[0], "query 29");
    let kept = model.read_with(cx, |m, _| m.recents.len());
    assert_eq!(kept, KEPT_RECENTS);
}

#[gpui::test]
fn suggestions_are_cached_per_query_ignoring_case(cx: &mut TestAppContext) {
    #[derive(Default)]
    struct Suggesting(Mutex<Vec<String>>);
    impl SearchSource for Suggesting {
        fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
            Ok(SearchResults::default())
        }
        fn suggest(&self, query: &str) -> Result<Vec<String>, LoadFailure> {
            self.0.lock().unwrap().push(query.to_owned());
            Ok(vec![format!("{query} live")])
        }
    }
    let source = Arc::new(Suggesting::default());
    let model = cx.new(|_| SearchModel::new());
    model.update(cx, |m, cx| m.install(source.clone(), cx));
    for text in ["Daft", "Daf", "daft"] {
        model.update(cx, |m, cx| m.set_query(text, cx));
        cx.executor().advance_clock(SUGGEST_DEBOUNCE);
        cx.run_until_parked();
    }
    assert_eq!(*source.0.lock().unwrap(), ["Daft", "Daf"], "'daft' came from the cache");
    assert_eq!(
        model.read_with(cx, |m, _| m.suggestions().to_vec()),
        ["Daft live"]
    );
    model.update(cx, |m, cx| m.set_query("", cx));
    assert!(model.read_with(cx, |m, _| m.suggestions().is_empty()));
}

#[gpui::test]
fn a_failing_suggestion_request_just_leaves_the_popover_empty(cx: &mut TestAppContext) {
    struct Failing;
    impl SearchSource for Failing {
        fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
            Ok(SearchResults::default())
        }
        fn suggest(&self, _: &str) -> Result<Vec<String>, LoadFailure> {
            Err(LoadFailure::Network)
        }
    }
    let model = cx.new(|_| SearchModel::new());
    model.update(cx, |m, cx| m.install(Arc::new(Failing), cx));
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert!(model.read_with(cx, |m, _| m.suggestions().is_empty()));
    assert_ne!(
        model.read_with(cx, |m, _| m.state()),
        SearchState::Failed(LoadFailure::Network),
        "suggestions never put the page in an error state"
    );
}

// ---- chips and paging -------------------------------------------------------------------------

/// Filtered lists: "a" answers a page of two songs (the second a video) with a token, the token
/// answers two more, and a token whose page repeats the first is "stuck".
#[derive(Default)]
struct Paged {
    filtered: Mutex<Vec<(String, SearchFilter)>>,
    more: Mutex<Vec<String>>,
}

fn paged_song(i: usize) -> ListedTrack {
    ListedTrack {
        track: Track::new(format!("song{i:07}"), format!("Song {i}")),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

impl SearchSource for Paged {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }

    fn search_filtered(&self, query: &str, filter: SearchFilter) -> Result<SearchPage, LoadFailure> {
        self.filtered.lock().unwrap().push((query.to_owned(), filter));
        let mut results = SearchResults {
            songs: vec![paged_song(0), paged_song(1)],
            ..SearchResults::default()
        };
        results.video_ids.insert("song0000001".into());
        Ok(SearchPage {
            results,
            continuation: Some(if query == "stuck" { "again" } else { "next" }.into()),
        })
    }

    fn search_more(&self, token: &str) -> Result<SearchPage, LoadFailure> {
        self.more.lock().unwrap().push(token.to_owned());
        let songs = if token == "again" {
            // The same rows as the first page, with a token that leads nowhere new.
            vec![paged_song(0), paged_song(1)]
        } else {
            vec![paged_song(2), paged_song(3)]
        };
        Ok(SearchPage {
            results: SearchResults {
                songs,
                ..SearchResults::default()
            },
            continuation: Some(token.to_owned()),
        })
    }
}

fn paged_model(cx: &mut TestAppContext) -> (Entity<SearchModel>, Arc<Paged>) {
    let source = Arc::new(Paged::default());
    let model = cx.new(|_| SearchModel::new());
    model.update(cx, |m, cx| m.install(source.clone(), cx));
    (model, source)
}

#[gpui::test]
fn a_query_typed_on_a_chip_asks_only_for_that_chips_list(cx: &mut TestAppContext) {
    let (model, source) = paged_model(cx);
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(
        *source.filtered.lock().unwrap(),
        [("daft".to_owned(), SearchFilter::Songs)]
    );
    assert_eq!(state(&model, cx), SearchState::Ready);
    assert_eq!(song_titles(&model, cx), ["Song 0", "Song 1"]);
}

#[gpui::test]
fn the_hide_music_videos_choice_applies_to_a_filtered_songs_list_and_its_next_pages(
    cx: &mut TestAppContext,
) {
    let (model, _source) = paged_model(cx);
    model.update(cx, |m, cx| m.set_options(true, true, cx));
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(song_titles(&model, cx), ["Song 0"]);
    model.update(cx, |m, cx| m.load_more(cx));
    cx.run_until_parked();
    assert_eq!(song_titles(&model, cx), ["Song 0", "Song 2", "Song 3"]);
    model.update(cx, |m, cx| m.set_options(true, false, cx));
    assert_eq!(
        song_titles(&model, cx),
        ["Song 0", "Song 1", "Song 2", "Song 3"],
        "turning it off shows them again with no new request"
    );
}

#[gpui::test]
fn a_page_with_nothing_new_ends_the_list(cx: &mut TestAppContext) {
    let (model, source) = paged_model(cx);
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    model.update(cx, |m, cx| m.set_query("stuck", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert!(model.read_with(cx, |m, _| m.has_more()));
    model.update(cx, |m, cx| m.load_more(cx));
    cx.run_until_parked();
    assert!(
        !model.read_with(cx, |m, _| m.has_more()),
        "a page of repeats is the end, whatever token it carries"
    );
    model.update(cx, |m, cx| m.load_more(cx));
    cx.run_until_parked();
    assert_eq!(source.more.lock().unwrap().len(), 1);
}

#[gpui::test]
fn a_page_for_a_query_that_was_replaced_is_dropped(cx: &mut TestAppContext) {
    let (model, _source) = paged_model(cx);
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.load_more(cx));
    // The query changes before the page lands.
    model.update(cx, |m, cx| m.set_query("punk", cx));
    cx.run_until_parked();
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(
        song_titles(&model, cx),
        ["Song 0", "Song 1"],
        "only punk's first page is shown"
    );
}

#[gpui::test]
fn a_chip_that_was_left_loading_is_asked_again_when_it_comes_back(cx: &mut TestAppContext) {
    let (model, source) = paged_model(cx);
    model.update(cx, |m, cx| m.set_query("daft", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    // Away before the answer, onto a new query, then back.
    model.update(cx, |m, cx| m.set_chip(SearchChip::Albums, cx));
    model.update(cx, |m, cx| m.set_query("punk", cx));
    cx.executor().advance_clock(DEBOUNCE);
    cx.run_until_parked();
    model.update(cx, |m, cx| m.set_chip(SearchChip::Songs, cx));
    cx.run_until_parked();
    assert_eq!(state(&model, cx), SearchState::Ready);
    assert!(source
        .filtered
        .lock()
        .unwrap()
        .contains(&("punk".to_owned(), SearchFilter::Songs)));
}
