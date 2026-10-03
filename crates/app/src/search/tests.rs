use std::sync::Mutex;

use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{Hit, ListedTrack};
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
