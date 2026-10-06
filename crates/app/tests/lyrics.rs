//! Lyrics in Now Playing: looked up for the playing and the next track, shown where the queue
//! would be, following the song, seeking on a click, pausing when scrolled by hand. The real
//! shell and views on GPUI's test platform, a scripted lyrics source, and the mock playback
//! adapters.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::lyrics::LyricsState;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::LyricsOpenStore;
use apollo_app::style::Fonts;
use apollo_app::ui::lyrics_pane::LyricsPane;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{ArtistRef, Track};
use apollo_domain::playback::{EngineEvent, Event, LeaseId, Phase};
use apollo_lyrics::{FetchError, Line, Lyrics, LyricsQuery, LyricsService, LyricsSource};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, px, AppContext, Bounds, Entity, Modifiers, Pixels, ScrollDelta, ScrollWheelEvent,
    TestAppContext, TouchPhase, VisualTestContext,
};

/// Synced, plain, none, then a track whose lookup fails.
const IDS: [&str; 4] = ["track000000", "track000001", "track000002", "track000003"];
const LINES: usize = 24;

fn synced() -> Lyrics {
    Lyrics::Synced(
        (0..LINES)
            .map(|i| Line {
                start_ms: 5_000 * (i as u64 + 1),
                text: format!("Line number {i} of the song"),
            })
            .collect(),
    )
}

/// Answers by video id and remembers what it was asked.
#[derive(Default)]
struct Script {
    asked: Mutex<Vec<String>>,
    offline: Mutex<bool>,
}

impl Script {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl LyricsSource for Script {
    fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        self.asked.lock().unwrap().push(q.video_id.clone());
        if *self.offline.lock().unwrap() {
            return Err(FetchError("offline".into()));
        }
        Ok(match q.video_id.as_str() {
            "track000000" => Some(synced()),
            "track000001" => Some(Lyrics::Plain("First line\n\nSecond line".into())),
            _ => None,
        })
    }
}

#[derive(Default)]
struct MemFlag(Mutex<Option<bool>>);
impl LyricsOpenStore for MemFlag {
    fn load(&self) -> Option<bool> {
        *self.0.lock().unwrap()
    }
    fn save(&self, on: bool) {
        *self.0.lock().unwrap() = Some(on);
    }
}

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }
}

fn prepared() -> Prepared {
    let queue = IDS
        .iter()
        .map(|id| {
            let mut track = Track::new(*id, format!("Title {id}"));
            track.artists = vec![ArtistRef {
                name: "Artist".into(),
                id: None,
            }];
            track.duration = Some(Duration::from_secs(200));
            LibraryTrack { track }
        })
        .collect();
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue,
        art: Arc::new(TestArt),
        search: Arc::new(TestSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(apollo_app::library::sim::SimLibrary::instant()),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    pane: Entity<LyricsPane>,
    script: Arc<Script>,
    flag: Arc<MemFlag>,
}

fn setup(cx: &mut TestAppContext, saved: Option<bool>) -> (Page, &mut VisualTestContext) {
    setup_with(cx, saved, true)
}

fn setup_with(
    cx: &mut TestAppContext,
    saved: Option<bool>,
    install: bool,
) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = prepared();
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    let script = Arc::new(Script::default());
    let flag = Arc::new(MemFlag(Mutex::new(saved)));
    let service = Arc::new(LyricsService::new(vec![script.clone()], None));
    if install {
        shell.update(vcx, |s, cx| {
            s.install_lyrics(service, Some(flag.clone() as Arc<dyn LyricsOpenStore>), cx)
        });
    }
    vcx.run_until_parked();
    let pane = shell.read_with(vcx, |s, _| s.lyrics_pane().clone());
    (
        Page {
            player,
            shell,
            pane,
            script,
            flag,
        },
        vcx,
    )
}

/// Several parks: the follow logic waits a frame for the column's layout.
fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..4 {
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
    }
    vcx.run_until_parked();
}

fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
}

fn on_screen(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

/// Load the current track and report a playback position.
fn play_at(page: &Page, vcx: &mut VisualTestContext, position_ms: u64) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load());
    let load = match load {
        Some(load) => load,
        None => {
            page.player.update(vcx, |m, cx| m.handle(Event::Play, cx));
            page.player
                .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap())
        }
    };
    let loaded = page.player.read_with(vcx, |m, _| {
        matches!(
            m.coordinator().unwrap().phase(),
            Phase::Playing | Phase::Paused | Phase::Seeking | Phase::Buffering
        )
    });
    if !loaded {
        page.player.update(vcx, |m, cx| {
            m.handle(
                Event::Resolved {
                    load,
                    result: Ok(LeaseId(1)),
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
    page.player.update(vcx, |m, cx| {
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Position(position_ms),
            },
            cx,
        )
    });
    settle(vcx);
}

/// What was looked up so far, sorted (lookups run concurrently).
fn asked(page: &Page) -> Vec<String> {
    let mut asked = page.script.asked();
    asked.sort();
    asked
}

fn show_lyrics(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.toggle_lyrics(cx));
    settle(vcx);
}

/// Where a line's top sits below the top of the lyrics column.
fn line_top(vcx: &mut VisualTestContext, line: &'static str) -> f32 {
    let column = bounds(vcx, "lyrics-scroll");
    let line = bounds(vcx, line);
    f32::from(line.top() - column.top())
}

fn scroll_by_hand(vcx: &mut VisualTestContext, dy: f32) {
    let at = bounds(vcx, "lyrics-scroll").center();
    vcx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    settle(vcx);
}

/// One skip, then enough time for the lookups it started to be let out (they are spaced).
fn skip(page: &Page, vcx: &mut VisualTestContext) {
    page.player.update(vcx, |m, cx| m.next(cx));
    vcx.executor().advance_clock(apollo_app::lyrics::SETTLE * 2);
    vcx.run_until_parked();
}

#[gpui::test]
fn lyrics_are_looked_up_for_the_playing_and_the_next_track_without_the_pane_open(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    // Nothing was opened: the pane is closed and Now Playing is not showing.
    assert!(!page.shell.read_with(vcx, |s, cx| s.lyrics_showing(cx)));
    assert_eq!(asked(&page), IDS[..2].to_vec());
    // Next: the track after the new current one is looked up; the others are not asked twice.
    skip(&page, vcx);
    assert_eq!(asked(&page), IDS[..3].to_vec());
    skip(&page, vcx);
    assert_eq!(asked(&page), IDS.to_vec());
}

#[gpui::test]
fn skipping_quickly_looks_up_only_the_song_landed_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    assert_eq!(asked(&page), IDS[..2].to_vec());
    // Two skips in a row, faster than the spacing: the song in between is never asked for.
    page.player.update(vcx, |m, cx| m.next(cx));
    page.player.update(vcx, |m, cx| m.next(cx));
    vcx.run_until_parked();
    // (The first skip may go out at once, being the first in a while; the second is held back.)
    assert!(!asked(&page).contains(&IDS[3].to_owned()), "held back, not asked per skip");
    vcx.executor().advance_clock(apollo_app::lyrics::SETTLE * 2);
    vcx.run_until_parked();
    assert_eq!(asked(&page), IDS.to_vec(), "the landed-on song and the one after it");
}

#[gpui::test]
fn the_state_follows_the_lookup_for_each_track(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    let lyrics = page.shell.read_with(vcx, |s, _| s.lyrics().clone());
    let state =
        |vcx: &mut VisualTestContext, id: &str| lyrics.read_with(vcx, |l, _| l.state(Some(id)));
    assert!(matches!(state(vcx, IDS[0]), LyricsState::Ready(l) if l.is_synced()));
    assert!(matches!(state(vcx, IDS[1]), LyricsState::Ready(l) if !l.is_synced()));
    // Not looked up yet: only the playing and the next track are.
    assert_eq!(state(vcx, IDS[2]), LyricsState::Loading);
    page.player.update(vcx, |m, cx| m.next(cx));
    page.player.update(vcx, |m, cx| m.next(cx));
    vcx.run_until_parked();
    assert_eq!(state(vcx, IDS[2]), LyricsState::Missing);
    assert_eq!(
        lyrics.read_with(vcx, |l, _| l.state(None)),
        LyricsState::Idle
    );
}

#[gpui::test]
fn the_lyrics_button_puts_lyrics_where_the_queue_would_be_and_the_queue_button_takes_it_back(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    assert!(!on_screen(vcx, "np-lyrics"));
    // Pressing Lyrics opens Now Playing with lyrics on its right half.
    show_lyrics(&page, vcx);
    assert!(page.shell.read_with(vcx, |s, _| s.now_playing_open()));
    assert!(page.shell.read_with(vcx, |s, cx| s.lyrics_showing(cx)));
    let art = bounds(vcx, "now-playing-art");
    let lyrics = bounds(vcx, "np-lyrics");
    assert!(art.right() <= lyrics.left(), "the art stays on the left");
    assert!(lyrics.size.width >= px(apollo_theme::tokens::lyrics::MIN_PANE_WIDTH));
    assert!(on_screen(vcx, "lyrics-scroll"));
    // The setting is remembered (saved off the UI thread).
    for _ in 0..200 {
        if page.flag.load() == Some(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(page.flag.load(), Some(true));

    // The queue button asks for the same side: the lyrics give way, the queue opens.
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    settle(vcx);
    assert!(!page.shell.read_with(vcx, |s, cx| s.lyrics_showing(cx)));
    assert!(page.shell.read_with(vcx, |s, _| s.queue_open()));
    assert!(!on_screen(vcx, "np-lyrics"));

    // Lyrics again: they replace the queue panel while they show; the queue stays open under.
    show_lyrics(&page, vcx);
    assert!(on_screen(vcx, "np-lyrics"));
    assert!(page.shell.read_with(vcx, |s, _| s.queue_open()));
    // Off: Now Playing stays, the queue is back.
    show_lyrics(&page, vcx);
    assert!(page.shell.read_with(vcx, |s, _| s.now_playing_open()));
    assert!(!on_screen(vcx, "np-lyrics"));
}

#[gpui::test]
fn the_saved_choice_is_applied_at_startup(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, Some(true));
    assert!(page.pane.read_with(vcx, |p, _| p.is_open()));
    // Open lyrics wait for Now Playing; nothing shows on other pages.
    assert!(!page.shell.read_with(vcx, |s, cx| s.lyrics_showing(cx)));
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    settle(vcx);
    assert!(on_screen(vcx, "np-lyrics"));
}

#[gpui::test]
fn the_current_line_sits_a_third_of_the_way_down_and_the_column_follows_the_song(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    show_lyrics(&page, vcx);
    play_at(&page, vcx, 12_000); // line 1 started at 10 s
    let column = bounds(vcx, "lyrics-scroll");
    let want = f32::from(column.size.height) / 3.0;
    let line = bounds(vcx, "lyric-1");
    let centre = f32::from(line.center().y - column.top());
    assert!(
        (centre - want).abs() < 2.0,
        "line 1 is centred {centre}px down, wanted {want}"
    );
    // Later in the song the column has moved on; the new line is at the same place.
    play_at(&page, vcx, 62_000); // line 11
    let line = bounds(vcx, "lyric-11");
    let centre = f32::from(line.center().y - column.top());
    assert!(
        (centre - want).abs() < 2.0,
        "line 11 is centred {centre}px down, wanted {want}, column {column:?}"
    );
    // Near the start the first lines cannot reach a third: the column rests at the top.
    play_at(&page, vcx, 0);
    assert!(line_top(vcx, "lyric-0") > 0.0);
}

#[gpui::test]
fn clicking_a_line_seeks_to_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show_lyrics(&page, vcx);
    play_at(&page, vcx, 12_000);
    let at = bounds(vcx, "lyric-3").center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    let (phase, intent_seek) = page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        (c.phase(), c.phase() == Phase::Seeking)
    });
    assert!(intent_seek, "a seek began, phase {phase:?}");
}

#[gpui::test]
fn scrolling_by_hand_pauses_following_until_four_seconds_pass_or_a_line_is_clicked(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    show_lyrics(&page, vcx);
    play_at(&page, vcx, 12_000);
    let column = bounds(vcx, "lyrics-scroll");
    let want = f32::from(column.size.height) / 3.0;
    let centred = |vcx: &mut VisualTestContext, line: &'static str| {
        let line = bounds(vcx, line);
        (f32::from(line.center().y - column.top()) - want).abs() < 2.0
    };

    scroll_by_hand(vcx, -120.0);
    assert!(!centred(vcx, "lyric-1"), "the wheel moved the column");
    // The song moves on while the user reads elsewhere: the column stays where they put it.
    let held = line_top(vcx, "lyric-1");
    play_at(&page, vcx, 22_000);
    assert_eq!(line_top(vcx, "lyric-1"), held);
    // 4 seconds after the last scroll, following resumes on the current line.
    vcx.executor().advance_clock(Duration::from_millis(3_000));
    settle(vcx);
    assert_eq!(line_top(vcx, "lyric-1"), held, "still held at 3 s");
    vcx.executor().advance_clock(Duration::from_millis(1_600));
    settle(vcx);
    assert!(centred(vcx, "lyric-3"), "line 3 is current at 22 s");

    // A click on a line ends the pause at once.
    scroll_by_hand(vcx, -120.0);
    assert!(!centred(vcx, "lyric-3"));
    let at = bounds(vcx, "lyric-5").center();
    vcx.simulate_click(at, Modifiers::none());
    play_at(&page, vcx, 30_000); // line 5 is current
    assert!(centred(vcx, "lyric-5"), "following resumed after the click");
}

#[gpui::test]
fn plain_lyrics_are_static_text_and_no_lyrics_is_one_line(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show_lyrics(&page, vcx);
    assert!(
        on_screen(vcx, "lyrics-scroll"),
        "synced for the first track"
    );
    assert!(!on_screen(vcx, "lyrics-plain"));
    page.player.update(vcx, |m, cx| m.next(cx));
    settle(vcx);
    assert!(on_screen(vcx, "lyrics-plain"));
    assert!(!on_screen(vcx, "lyrics-scroll"));
    assert!(
        !on_screen(vcx, "lyric-0"),
        "plain lines are not clickable rows"
    );
    page.player.update(vcx, |m, cx| m.next(cx));
    settle(vcx);
    assert!(on_screen(vcx, "lyrics-none"));
    assert!(!on_screen(vcx, "lyrics-plain"));
}

#[gpui::test]
fn a_failed_lookup_is_tried_again_and_shows_no_lyrics_meanwhile(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    *page.script.offline.lock().unwrap() = true;
    skip(&page, vcx);
    skip(&page, vcx);
    skip(&page, vcx);
    let before = page.script.asked().len();
    let lyrics = page.shell.read_with(vcx, |s, _| s.lyrics().clone());
    assert_eq!(
        lyrics.read_with(vcx, |l, _| l.state(Some(IDS[3]))),
        LyricsState::Missing
    );
    // Back online: the retry (15 s later) finds nothing for this id, but it did ask again.
    *page.script.offline.lock().unwrap() = false;
    vcx.executor().advance_clock(Duration::from_secs(16));
    vcx.run_until_parked();
    assert!(page.script.asked().len() > before);
}

#[gpui::test]
fn until_the_lookup_is_installed_the_pane_shows_the_skeleton_and_then_the_lyrics(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup_with(cx, None, false);
    show_lyrics(&page, vcx);
    assert!(on_screen(vcx, "lyrics-skeleton"));
    assert!(!on_screen(vcx, "lyrics-none"));
    assert!(!on_screen(vcx, "lyrics-scroll"));
    let service = Arc::new(LyricsService::new(vec![page.script.clone()], None));
    page.shell
        .update(vcx, |s, cx| s.install_lyrics(service, None, cx));
    settle(vcx);
    assert!(!on_screen(vcx, "lyrics-skeleton"));
    assert!(on_screen(vcx, "lyrics-scroll"));
}

#[gpui::test]
fn with_animation_on_the_column_glides_to_the_new_line(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    vcx.update(|_, cx| cx.set_reduce_motion(false));
    show_lyrics(&page, vcx);
    play_at(&page, vcx, 12_000); // the first placement jumps
    let column = bounds(vcx, "lyrics-scroll");
    let want = f32::from(column.size.height) / 3.0;
    let before = line_top(vcx, "lyric-9");
    play_at(&page, vcx, 52_000); // line 9 is current: a glide starts
    std::thread::sleep(Duration::from_millis(260));
    settle(vcx);
    let line = bounds(vcx, "lyric-9");
    let centre = f32::from(line.center().y - column.top());
    assert!(
        (centre - want).abs() < 2.0,
        "glided to the third: {centre} vs {want}"
    );
    assert!(
        line_top(vcx, "lyric-9") < before,
        "and it moved up the page"
    );
}

#[gpui::test]
fn at_the_minimum_window_lyrics_and_the_cover_column_fit_and_the_bar_holds_every_button(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    for (w, h) in [(864.0_f32, 480.0_f32), (1100.0, 720.0), (1400.0, 900.0)] {
        vcx.simulate_resize(gpui::size(px(w), px(h)));
        show_lyrics(&page, vcx);
        let (art, lyrics, controls) = (
            bounds(vcx, "now-playing-art"),
            bounds(vcx, "np-lyrics"),
            bounds(vcx, "np-controls"),
        );
        assert!(art.left() >= px(64.0), "{w}x{h}: clear of the nav rail");
        assert!(
            art.right() <= lyrics.left(),
            "{w}x{h}: art {art:?} lyrics {lyrics:?}"
        );
        assert!(
            lyrics.size.width >= px(320.0),
            "{w}x{h}: lyrics are {:?} wide",
            lyrics.size.width
        );
        assert!(
            lyrics.right() <= px(w),
            "{w}x{h}: lyrics end inside the window"
        );
        assert!(
            controls.right() <= lyrics.left(),
            "{w}x{h}: controls stay in their half"
        );
        assert!(
            controls.bottom() <= px(h - 80.0),
            "{w}x{h}: transport stays on the page"
        );
        // The bar's right block holds heart, mute, volume, lyrics and queue without spilling.
        let (right, like) = (bounds(vcx, "player-bar-right"), bounds(vcx, "bar-like"));
        assert!(
            like.left() >= right.left(),
            "{w}x{h}: bar buttons fit their block: like {like:?} right {right:?}"
        );
        show_lyrics(&page, vcx);
    }
}
