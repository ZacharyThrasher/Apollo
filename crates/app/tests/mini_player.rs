//! The mini player window and the remembered window place, on GPUI's test platform: opening it
//! hides nothing it should not, going back works every way it should, the keyboard and the
//! always-on-top choice reach it, and sizes and positions are written down once they settle.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::hud::{DIM, HOLD, IDLE_DIM};
use apollo_app::mini;
use apollo_app::player::PlayerModel;
use apollo_app::search::Unavailable;
use apollo_app::settings::{
    SettingsBackend, SettingsModel, HUD_AUTO_HIDE, MINI_COLLAPSE_ON_BLUR, MINI_DIM,
    MINI_FADE_CONTROLS, MINI_PLAYER_HUD, MINI_PLAYER_ON_TOP, MINI_SHOW_LYRICS, MINI_SHOW_VOLUME,
};
use apollo_app::style::Fonts;
use apollo_app::ui::mini_player::{self, MiniPlayer};
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_app::window_state::{self, SavedWindow, KEY_HUD, KEY_MAIN, KEY_MINI};
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId};
use apollo_lyrics::{FetchError, Line, Lyrics, LyricsQuery, LyricsService, LyricsSource};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, px, size, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, WindowHandle, WindowOptions,
};

/// An in-memory settings table that counts its writes.
#[derive(Default)]
struct MemoryStore {
    rows: Mutex<HashMap<String, String>>,
    writes: Mutex<Vec<(String, String)>>,
}

impl MemoryStore {
    fn with(rows: &[(&str, &str)]) -> Arc<Self> {
        let store = Self::default();
        for (k, v) in rows {
            store.rows.lock().unwrap().insert((*k).into(), (*v).into());
        }
        Arc::new(store)
    }

    fn writes_of(&self, key: &str) -> Vec<String> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

impl SettingsBackend for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.rows.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, value: &str) {
        self.rows.lock().unwrap().insert(key.into(), value.into());
        self.writes.lock().unwrap().push((key.into(), value.into()));
    }
}

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

/// Three queued songs behind the mock resolver and engine.
fn prepared() -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: (0..3)
            .map(|i| LibraryTrack {
                track: Track::new(format!("track{i:06}"), format!("Track {i}")),
            })
            .collect(),
        art: Arc::new(TestArt),
        search: Arc::new(Unavailable),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(apollo_app::library::sim::SimLibrary::instant()),
    }
}

struct World {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    main: WindowHandle<Shell>,
}

fn setup(cx: &mut TestAppContext, store: Option<Arc<dyn SettingsBackend>>) -> World {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, cx| m.install(prepared(), cx));
    let settings = cx.new(|_| SettingsModel::new(None));
    let main = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), {
            let player = player.clone();
            move |_, cx| cx.new(|cx| Shell::new(player, cx))
        })
        .expect("main window")
    });
    cx.update(|cx| mini::install(cx, main, player.clone(), settings.clone(), store));
    World {
        player,
        settings,
        main,
    }
}

fn open_mini(cx: &mut TestAppContext) -> (WindowHandle<MiniPlayer>, VisualTestContext) {
    cx.update(mini::open);
    cx.run_until_parked();
    let handle = cx
        .update(|cx| mini::window(cx))
        .expect("the mini player opened");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    (handle, vcx)
}

#[gpui::test]
fn opening_the_mini_player_makes_one_small_window_in_the_corner(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    assert!(!cx.update(|cx| mini::is_open(cx)));
    let (handle, _vcx) = open_mini(cx);
    assert!(cx.update(|cx| mini::is_open(cx)));
    let bounds = handle
        .update(cx, |_, window, _| window.bounds())
        .expect("window");
    assert_eq!(
        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
        (mini_player::WIDTH, mini_player::HEIGHT)
    );
    // Nothing saved: the bottom-right corner of the 1920x1080 test display, above the taskbar.
    assert_eq!(
        (f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
        (1920.0 - 360.0 - 24.0, 1080.0 - 96.0 - 72.0)
    );
    // Asking again brings the same window forward; there is never a second one.
    let windows = cx.update(|cx| cx.windows().len());
    cx.update(mini::open);
    assert_eq!(cx.update(|cx| cx.windows().len()), windows);
}

#[gpui::test]
fn the_player_bar_button_opens_it_from_inside_a_main_window_event(cx: &mut TestAppContext) {
    // The button runs inside the main window's own event, where that window cannot be read or
    // updated again: opening has to wait until the event is over (it crashed the app once).
    let world = setup(cx, None);
    let mut vcx = VisualTestContext::from_window(world.main.into(), cx);
    vcx.run_until_parked();
    let button = vcx
        .debug_bounds("bar-mini")
        .expect("the player bar has a mini player button");
    vcx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.update(|cx| mini::is_open(cx)),
        "clicking the button opens the mini player"
    );
}

#[gpui::test]
fn returning_closes_the_mini_player_and_keeps_the_main_window(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    let (_handle, _vcx) = open_mini(cx);
    let windows = cx.update(|cx| cx.windows().len());
    cx.update(mini::return_to_main);
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));
    assert_eq!(cx.update(|cx| cx.windows().len()), windows - 1);
    assert!(
        world.main.update(cx, |_, _, _| ()).is_ok(),
        "main window is still there"
    );
    // And it can be opened again afterwards.
    let (_handle, _vcx) = open_mini(cx);
    assert!(cx.update(|cx| mini::is_open(cx)));
}

#[gpui::test]
fn showing_the_main_window_for_the_tray_or_a_second_launch_closes_the_mini_player(
    cx: &mut TestAppContext,
) {
    let world = setup(cx, None);
    let (_handle, _vcx) = open_mini(cx);
    let main = world.main;
    cx.update(|cx| mini::show_main(cx, main));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));
}

#[gpui::test]
fn the_transport_keys_work_in_the_mini_player_and_escape_goes_back(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    let (_handle, mut vcx) = open_mini(cx);
    let player = world.player.clone();
    let current = |cx: &mut VisualTestContext| {
        player.read_with(cx, |m, _| {
            let c = m.coordinator().unwrap();
            (c.intent(), c.queue().current().map(|t| t.id.clone()))
        })
    };
    let (intent, first) = current(&mut vcx);
    assert_eq!(intent, Intent::Paused);
    vcx.simulate_keystrokes("space");
    assert_eq!(current(&mut vcx).0, Intent::Playing, "Space plays");
    vcx.simulate_keystrokes("space");
    assert_eq!(current(&mut vcx).0, Intent::Paused, "and pauses");
    vcx.simulate_keystrokes("ctrl-right");
    assert_ne!(current(&mut vcx).1, first, "Ctrl+Right is next");
    vcx.simulate_keystrokes("ctrl-left");
    assert_eq!(current(&mut vcx).1, first, "Ctrl+Left is previous");
    assert!(cx.update(|cx| mini::is_open(cx)));
    vcx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)), "Esc is the way back");
}

#[gpui::test]
fn double_clicking_the_art_goes_back_but_one_click_does_not(cx: &mut TestAppContext) {
    let _world = setup(cx, None);
    let (_handle, mut vcx) = open_mini(cx);
    // The art is the 80px square after the 8px padding, vertically centred.
    let art = point(px(8.0 + 40.0), px(48.0));
    vcx.simulate_click(art, Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.update(|cx| mini::is_open(cx)),
        "one click leaves it open"
    );
    vcx.simulate_event(MouseDownEvent {
        position: art,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent {
        position: art,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
    });
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)), "a double click returns");
}

#[gpui::test]
fn the_expand_button_shows_only_while_the_pointer_is_over_the_window(cx: &mut TestAppContext) {
    let _world = setup(cx, None);
    let (handle, mut vcx) = open_mini(cx);
    let visible = |cx: &mut TestAppContext| {
        handle
            .read_with(cx, |m, _| m.expand_visible())
            .expect("window")
    };
    // The pointer starts off the window.
    vcx.simulate_mouse_move(point(px(-50.0), px(-50.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(!visible(cx), "hidden until the pointer arrives");
    vcx.simulate_mouse_move(point(px(200.0), px(40.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(visible(cx), "shown over the text");
    // And leaving hides it again.
    vcx.simulate_mouse_move(point(px(-50.0), px(-50.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(!visible(cx), "hidden again once the pointer leaves");
    vcx.simulate_mouse_move(point(px(200.0), px(40.0)), None, Modifiers::default());
    cx.run_until_parked();
    // Over the art and over the buttons, which sit above the drag layer.
    vcx.simulate_mouse_move(point(px(48.0), px(48.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(visible(cx), "still shown over the art");
    vcx.simulate_mouse_move(point(px(310.0), px(48.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(visible(cx), "still shown over the buttons");
}

#[gpui::test]
fn the_always_on_top_setting_reaches_the_window_and_follows_changes(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    let (handle, _vcx) = open_mini(cx);
    let applied = |cx: &mut TestAppContext| {
        handle
            .read_with(cx, |m, _| m.on_top_applied())
            .expect("window")
    };
    cx.run_until_parked();
    assert_eq!(applied(cx), Some(true), "on by default");
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_PLAYER_ON_TOP, false, cx));
    cx.run_until_parked();
    assert_eq!(applied(cx), Some(false));
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_PLAYER_ON_TOP, true, cx));
    cx.run_until_parked();
    assert_eq!(applied(cx), Some(true));
}

#[gpui::test]
fn the_mini_player_opens_where_it_was_left_and_a_vanished_monitor_sends_it_home(
    cx: &mut TestAppContext,
) {
    let store = MemoryStore::with(&[(KEY_MINI, "300,200,360,96,0")]);
    let _world = setup(cx, Some(store.clone()));
    let (handle, _vcx) = open_mini(cx);
    let origin = handle
        .update(cx, |_, window, _| window.bounds().origin)
        .unwrap();
    assert_eq!((f32::from(origin.x), f32::from(origin.y)), (300.0, 200.0));
    cx.update(mini::dismiss);

    // A record from a monitor that is no longer there lands in the corner instead.
    store.set(KEY_MINI, "-2000,200,360,96,0");
    let (handle, _vcx) = open_mini(cx);
    let origin = handle
        .update(cx, |_, window, _| window.bounds().origin)
        .unwrap();
    assert_eq!((f32::from(origin.x), f32::from(origin.y)), (1536.0, 912.0));
}

#[gpui::test]
fn a_stretched_or_maximized_record_still_opens_the_mini_player_small_and_is_put_right(
    cx: &mut TestAppContext,
) {
    // What a full-screen mini player left behind: the whole work area, maximized.
    let store = MemoryStore::with(&[(KEY_MINI, "0,0,1535,1100,1")]);
    let _world = setup(cx, Some(store.clone()));
    let (handle, _vcx) = open_mini(cx);
    let bounds = handle
        .update(cx, |_, window, _| window.bounds())
        .unwrap();
    assert_eq!(
        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
        (mini_player::WIDTH, mini_player::HEIGHT)
    );
    assert!(!handle.update(cx, |_, window, _| window.is_maximized()).unwrap());
    // The bad row is replaced on this launch, not on the next move.
    let row = store.get(KEY_MINI).expect("a row");
    let healed = SavedWindow::decode(&row).expect("a valid record");
    assert_eq!((healed.rect.w, healed.rect.h), (360.0, 96.0));
    assert!(!healed.maximized);
}

#[gpui::test]
fn a_mini_player_that_gets_stretched_shrinks_back_and_never_records_it(
    cx: &mut TestAppContext,
) {
    let store = MemoryStore::with(&[]);
    let _world = setup(cx, Some(store.clone()));
    let (handle, vcx) = open_mini(cx);
    vcx.simulate_resize(size(px(1535.0), px(1100.0)));
    // Long enough for the poll to see it and for any save delay to run out.
    for _ in 0..40 {
        vcx.executor().advance_clock(mini_player::POLL);
        vcx.run_until_parked();
    }
    let bounds = handle
        .update(cx, |_, window, _| window.bounds())
        .unwrap();
    assert_eq!(
        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
        (mini_player::WIDTH, mini_player::HEIGHT)
    );
    for text in store.writes_of(KEY_MINI) {
        let saved = SavedWindow::decode(&text).expect("a valid record");
        assert_eq!((saved.rect.w, saved.rect.h), (360.0, 96.0), "{text}");
        assert!(!saved.maximized);
    }
}

#[gpui::test]
fn a_window_that_stops_moving_is_written_down_once(cx: &mut TestAppContext) {
    cx.update(Fonts::install);
    let store = MemoryStore::with(&[]);
    let (shell, vcx) = {
        let player = cx.new(|_| PlayerModel::new());
        cx.add_window_view(move |_, cx| Shell::new(player, cx))
    };
    let backend: Arc<dyn SettingsBackend> = store.clone();
    shell.update_in(vcx, |s, window, cx| {
        let sub = window_state::remember(window, cx, Some(backend), KEY_MAIN, None);
        s.keep(sub);
    });
    // A drag-resize is a stream of sizes; only where it ends up is kept.
    for width in [900.0, 950.0, 1000.0, 1040.0] {
        vcx.simulate_resize(size(px(width), px(700.0)));
        vcx.executor().advance_clock(Duration::from_millis(100));
    }
    assert!(
        store.writes_of(KEY_MAIN).is_empty(),
        "nothing yet: it is still moving"
    );
    vcx.executor().advance_clock(Duration::from_secs(1));
    vcx.run_until_parked();
    let writes = store.writes_of(KEY_MAIN);
    assert_eq!(writes.len(), 1, "{writes:?}");
    let saved = SavedWindow::decode(&writes[0]).expect("a valid record");
    assert_eq!((saved.rect.w, saved.rect.h), (1040.0, 700.0));
    assert!(!saved.maximized);

    // Nothing changed: nothing written again.
    vcx.executor().advance_clock(Duration::from_secs(2));
    vcx.run_until_parked();
    assert_eq!(store.writes_of(KEY_MAIN).len(), 1);
}

// ---- HUD mode -------------------------------------------------------------------------------

/// A clock the test moves by hand, and a pointer it places by hand.
struct Hand {
    base: Instant,
    offset: Rc<Cell<Duration>>,
    inside: Rc<Cell<bool>>,
}

impl Hand {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Rc::new(Cell::new(Duration::ZERO)),
            inside: Rc::new(Cell::new(false)),
        }
    }

    /// Hands the clock and the pointer to the HUD.
    fn attach(&self, handle: WindowHandle<MiniPlayer>, cx: &mut TestAppContext) {
        let (base, offset, inside) = (self.base, self.offset.clone(), self.inside.clone());
        handle
            .update(cx, move |view, _, cx| {
                view.set_clock(move || base + offset.get(), cx);
                view.set_pointer_probe(move |_| inside.get());
            })
            .unwrap();
    }

    /// Lets `seconds` pass, a poll at a time.
    fn pass(&self, seconds: f32, cx: &mut TestAppContext) {
        let ticks = (seconds / 0.05).round() as u32;
        for _ in 0..ticks {
            self.offset
                .set(self.offset.get() + Duration::from_millis(50));
            cx.executor().advance_clock(mini_player::POLL);
            cx.run_until_parked();
        }
    }
}

fn open_hud(
    cx: &mut TestAppContext,
    store: Option<Arc<dyn SettingsBackend>>,
) -> (World, WindowHandle<MiniPlayer>, Hand) {
    let world = setup(cx, store);
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_PLAYER_HUD, true, cx));
    cx.update(mini::open);
    cx.run_until_parked();
    let handle = cx.update(|cx| mini::window(cx)).expect("opened");
    let hand = Hand::new();
    hand.attach(handle, cx);
    (world, handle, hand)
}

#[gpui::test]
fn the_hud_setting_opens_the_hud_click_through_and_off_by_default(cx: &mut TestAppContext) {
    // Off: an ordinary mini player.
    let _world = setup(cx, None);
    let (plain, _vcx) = open_mini(cx);
    assert!(!plain.read_with(cx, |m, _| m.is_hud()).unwrap());
    assert_eq!(
        plain
            .read_with(cx, |m, _| m.click_through_requested())
            .unwrap(),
        None
    );
    cx.update(mini::dismiss);

    // On: a HUD that lets clicks through.
    let (_world, handle, _hand) = open_hud(cx, None);
    assert!(handle.read_with(cx, |m, _| m.is_hud()).unwrap());
    assert_eq!(
        handle
            .read_with(cx, |m, _| m.click_through_requested())
            .unwrap(),
        Some(true)
    );
}

#[gpui::test]
fn the_hud_is_solid_and_clickable_only_while_the_pointer_is_over_it(cx: &mut TestAppContext) {
    let (_world, handle, hand) = open_hud(cx, None);
    let through = |cx: &mut TestAppContext| {
        handle
            .read_with(cx, |m, _| m.click_through_requested())
            .unwrap()
    };
    hand.pass(0.2, cx);
    assert_eq!(through(cx), Some(true));

    hand.inside.set(true);
    hand.pass(0.2, cx);
    assert_eq!(
        through(cx),
        Some(false),
        "clicks land on the HUD under the pointer"
    );
    assert!(handle.read_with(cx, |m, _| m.expand_visible()).unwrap());

    hand.inside.set(false);
    hand.pass(0.2, cx);
    assert_eq!(
        through(cx),
        Some(true),
        "and fall through again when it leaves"
    );
}

#[gpui::test]
fn auto_hide_dims_the_hud_after_a_hold_and_a_new_track_brings_it_back(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_hud(cx, None);
    let level = |cx: &mut TestAppContext| handle.read_with(cx, |m, _| m.hud_level()).unwrap();
    hand.pass(HOLD.as_secs_f32() - 0.5, cx);
    assert_eq!(level(cx), 1.0, "solid through the hold");
    hand.pass(1.5, cx);
    assert!(
        (level(cx) - DIM).abs() < 1e-3,
        "dim after it: {}",
        level(cx)
    );

    // The pointer over it makes it solid, leaving lets it dim again.
    hand.inside.set(true);
    hand.pass(0.5, cx);
    assert_eq!(level(cx), 1.0);
    hand.inside.set(false);
    hand.pass(1.0, cx);
    assert!((level(cx) - DIM).abs() < 1e-3);

    // A track change pops it back for the hold.
    world.player.update(cx, |m, cx| m.next(cx));
    hand.pass(0.5, cx);
    assert_eq!(level(cx), 1.0, "a new song wakes it");
    hand.pass(HOLD.as_secs_f32() + 1.0, cx);
    assert!((level(cx) - DIM).abs() < 1e-3);
}

#[gpui::test]
fn with_auto_hide_off_the_hud_stays_solid(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_hud(cx, None);
    world
        .settings
        .update(cx, |m, cx| m.set(&HUD_AUTO_HIDE, false, cx));
    hand.pass(HOLD.as_secs_f32() + 5.0, cx);
    assert_eq!(handle.read_with(cx, |m, _| m.hud_level()).unwrap(), 1.0);
}

#[gpui::test]
fn the_hud_snaps_to_the_nearest_corner_once_it_sits_still(cx: &mut TestAppContext) {
    // Left near the bottom-right corner of the 1920x1080 test display.
    let store = MemoryStore::with(&[(KEY_HUD, "1580,1000,320,64,0")]);
    let (_world, handle, hand) = open_hud(cx, Some(store));
    hand.pass(0.3, cx);
    assert_eq!(
        handle.read_with(cx, |m, _| m.last_snap()).unwrap(),
        None,
        "not yet settled"
    );
    hand.pass(0.5, cx);
    let snap = handle
        .read_with(cx, |m, _| m.last_snap())
        .unwrap()
        .expect("snapped");
    // It is the 320x64 panel (the whole window) that sits 16px in.
    assert_eq!(
        (snap.x, snap.y),
        (1920.0 - 320.0 - 16.0, 1080.0 - 64.0 - 16.0),
        "16px in from the corner"
    );
}

#[gpui::test]
fn the_hud_hotkey_shows_and_hides_without_closing_the_mini_player(cx: &mut TestAppContext) {
    let _world = setup(cx, None);
    // Not open: the hotkey opens it.
    cx.update(mini::toggle_visibility);
    cx.run_until_parked();
    assert!(cx.update(|cx| mini::is_open(cx)));
    assert!(!cx.update(|cx| mini::is_hidden(cx)));
    cx.update(mini::toggle_visibility);
    assert!(cx.update(|cx| mini::is_open(cx)), "hiding keeps the window");
    assert!(cx.update(|cx| mini::is_hidden(cx)));
    cx.update(mini::toggle_visibility);
    assert!(!cx.update(|cx| mini::is_hidden(cx)));
    // Hidden and then asked to open (button, tray): it comes back.
    cx.update(mini::toggle_visibility);
    cx.update(mini::open);
    assert!(!cx.update(|cx| mini::is_hidden(cx)));
    // Closing resets it.
    cx.update(mini::toggle_visibility);
    cx.update(mini::dismiss);
    assert!(!cx.update(|cx| mini::is_hidden(cx)));
}

#[gpui::test]
fn the_hud_window_is_exactly_the_320_by_64_panel_and_has_its_own_saved_place(
    cx: &mut TestAppContext,
) {
    let store = MemoryStore::with(&[(KEY_MINI, "100,100,360,96,0")]);
    let (_world, handle, hand) = open_hud(cx, Some(store.clone()));
    hand.pass(0.1, cx);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let window = handle.update(cx, |_, w, _| w.bounds().size).unwrap();
    assert_eq!(
        (f32::from(window.width), f32::from(window.height)),
        (mini_player::HUD_WIDTH, mini_player::HUD_HEIGHT)
    );
    let panel = vcx.debug_bounds("hud-panel").expect("the panel");
    assert_eq!(
        (f32::from(panel.size.width), f32::from(panel.size.height)),
        (320.0, 64.0)
    );
    assert_eq!(
        (f32::from(panel.origin.x), f32::from(panel.origin.y)),
        (0.0, 0.0),
        "nothing round the panel: no margin, so no frame"
    );
    assert_eq!(
        (f32::from(window.width), f32::from(window.height)),
        (320.0, 64.0)
    );
    // Its place is not the mini player's: the mini player's saved spot is left alone.
    assert!(store.writes_of(KEY_MINI).is_empty());
}

#[gpui::test]
fn under_the_pointer_the_panel_goes_opaque_and_the_controls_appear(cx: &mut TestAppContext) {
    let (_world, handle, hand) = open_hud(cx, None);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let solid = |cx: &mut TestAppContext| handle.read_with(cx, |m, _| m.hud_solid()).unwrap();
    hand.pass(0.3, cx);
    assert_eq!(solid(cx), 0.0, "resting fill");
    assert!(
        vcx.debug_bounds("hud-controls").is_none(),
        "nothing but art and text at rest"
    );

    hand.inside.set(true);
    hand.pass(0.05, cx);
    let part = solid(cx);
    assert!(part > 0.0 && part < 1.0, "eased: {part}");
    hand.pass(0.2, cx);
    assert_eq!(solid(cx), 1.0);
    let controls = vcx
        .debug_bounds("hud-controls")
        .expect("controls under the pointer");
    // Previous, play/pause, next and the heart, 28px each, at the panel's right.
    assert!(f32::from(controls.size.width) >= 4.0 * 28.0);
    assert_eq!(f32::from(controls.size.height), 64.0);

    hand.inside.set(false);
    hand.pass(0.3, cx);
    assert_eq!(solid(cx), 0.0);
    assert!(vcx.debug_bounds("hud-controls").is_none());
}

#[gpui::test]
fn a_new_track_changes_over_for_a_moment_and_then_settles(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_hud(cx, None);
    let changing =
        |cx: &mut TestAppContext| handle.read_with(cx, |m, _| m.changing_over()).unwrap();
    hand.pass(0.5, cx);
    assert!(!changing(cx));
    world.player.update(cx, |m, cx| m.next(cx));
    cx.run_until_parked();
    assert!(changing(cx), "title, artist and art are crossing over");
    hand.pass(0.3, cx);
    assert!(!changing(cx), "settled after the swap time");
}

// ---- Mini player upgrades: idle fade, volume, lyrics, collapse -------------------------------

fn volume_of(world: &World, cx: &mut TestAppContext) -> u8 {
    world.player.read_with(cx, |m, _| m.view().volume)
}

fn open_plain(cx: &mut TestAppContext) -> (World, WindowHandle<MiniPlayer>, Hand) {
    let world = setup(cx, None);
    cx.update(mini::open);
    cx.run_until_parked();
    let handle = cx.update(|cx| mini::window(cx)).expect("opened");
    let hand = Hand::new();
    hand.attach(handle, cx);
    // The test pointer starts over the window; send it away so the window can go idle.
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_mouse_move(point(px(-50.0), px(-50.0)), None, Modifiers::default());
    cx.run_until_parked();
    (world, handle, hand)
}

fn levels(handle: WindowHandle<MiniPlayer>, cx: &mut TestAppContext) -> (f32, f32) {
    handle
        .read_with(cx, |m, _| (m.controls_level(), m.window_level()))
        .unwrap()
}

#[gpui::test]
fn left_alone_the_controls_fade_and_then_the_window_dims_and_the_pointer_restores_both(
    cx: &mut TestAppContext,
) {
    let (_world, handle, hand) = open_plain(cx);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    hand.pass(3.8, cx);
    assert_eq!(levels(handle, cx), (1.0, 1.0));
    assert!(vcx.debug_bounds("mini-controls").is_some());

    // The controls go first (150 ms), then the window dims toward 60%.
    hand.pass(0.25, cx);
    let (controls, window) = levels(handle, cx);
    assert!(controls < 1.0, "fading: {controls}");
    assert_eq!(window, 1.0, "the window waits for them");
    hand.pass(0.2, cx);
    assert_eq!(levels(handle, cx).0, 0.0);
    assert!(
        vcx.debug_bounds("mini-controls").is_none(),
        "nothing left to click"
    );
    hand.pass(1.0, cx);
    let (controls, window) = levels(handle, cx);
    assert_eq!(controls, 0.0);
    assert!((window - IDLE_DIM).abs() < 1e-3, "{window}");

    // The pointer over the window brings everything back at once, without waiting for a poll.
    vcx.simulate_mouse_move(point(px(200.0), px(40.0)), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(levels(handle, cx), (1.0, 1.0));
    assert!(vcx.debug_bounds("mini-controls").is_some());
    hand.pass(10.0, cx);
    assert_eq!(
        levels(handle, cx),
        (1.0, 1.0),
        "stays while the pointer is over"
    );

    // Leaving starts a fresh wait.
    vcx.simulate_mouse_move(point(px(-50.0), px(-50.0)), None, Modifiers::default());
    cx.run_until_parked();
    hand.pass(3.0, cx);
    assert_eq!(levels(handle, cx), (1.0, 1.0));
    hand.pass(2.0, cx);
    assert_eq!(levels(handle, cx).0, 0.0);
}

#[gpui::test]
fn the_idle_fades_are_each_a_setting(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_plain(cx);
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_FADE_CONTROLS, false, cx));
    hand.pass(6.0, cx);
    let (controls, window) = levels(handle, cx);
    assert_eq!(controls, 1.0, "controls stay with Fade controls off");
    assert!((window - IDLE_DIM).abs() < 1e-3, "{window}");

    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_DIM, false, cx));
    hand.pass(0.5, cx);
    assert_eq!(levels(handle, cx), (1.0, 1.0), "Dim off brings it back");
    hand.pass(8.0, cx);
    assert_eq!(levels(handle, cx), (1.0, 1.0), "and it stays");

    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_FADE_CONTROLS, true, cx));
    hand.pass(5.0, cx);
    assert_eq!(levels(handle, cx), (0.0, 1.0), "only the controls fade");
}

#[gpui::test]
fn a_new_track_shows_a_faded_mini_player_again(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_plain(cx);
    hand.pass(6.0, cx);
    assert_eq!(levels(handle, cx).0, 0.0);
    world.player.update(cx, |m, cx| m.next(cx));
    cx.run_until_parked();
    hand.pass(0.5, cx);
    assert_eq!(levels(handle, cx), (1.0, 1.0));
}

#[gpui::test]
fn the_volume_slider_is_72px_shares_the_app_volume_and_is_a_setting(cx: &mut TestAppContext) {
    let (world, handle, _hand) = open_plain(cx);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    world.player.update(cx, |m, cx| m.set_volume(30, cx));
    cx.run_until_parked();
    let slider = vcx
        .debug_bounds("mini-volume")
        .expect("the volume slider shows");
    assert_eq!(f32::from(slider.size.width), 72.0);
    // It sits on the right, under the buttons.
    assert!(f32::from(slider.left()) > 200.0);

    // A click halfway along sets half volume, in the player the main window uses.
    vcx.simulate_click(
        point(slider.left() + slider.size.width * 0.5, slider.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert_eq!(volume_of(&world, cx), 50);

    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_VOLUME, false, cx));
    cx.run_until_parked();
    assert!(vcx.debug_bounds("mini-volume").is_none(), "off hides it");
    assert!(
        vcx.debug_bounds("mini-controls").is_some(),
        "the buttons stay"
    );
}

#[gpui::test]
fn the_wheel_over_the_window_changes_the_volume_five_percent_a_notch(cx: &mut TestAppContext) {
    let (world, handle, _hand) = open_plain(cx);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    world.player.update(cx, |m, cx| m.set_volume(50, cx));
    cx.run_until_parked();
    let wheel = |vcx: &mut VisualTestContext, at: (f32, f32), delta: ScrollDelta| {
        vcx.simulate_event(ScrollWheelEvent {
            position: point(px(at.0), px(at.1)),
            delta,
            ..Default::default()
        });
        vcx.run_until_parked();
    };
    // Over the text, then over the art (which blocks the pointer from the window behind it).
    wheel(&mut vcx, (200.0, 40.0), ScrollDelta::Lines(point(0.0, 3.0)));
    assert_eq!(volume_of(&world, cx), 55, "up is louder");
    wheel(&mut vcx, (48.0, 48.0), ScrollDelta::Lines(point(0.0, -3.0)));
    wheel(&mut vcx, (48.0, 48.0), ScrollDelta::Lines(point(0.0, -3.0)));
    assert_eq!(
        volume_of(&world, cx),
        45,
        "down is quieter, over the art too"
    );
    // A touchpad needs a good stretch of scrolling for one step.
    wheel(
        &mut vcx,
        (200.0, 40.0),
        ScrollDelta::Pixels(point(px(0.0), px(15.0))),
    );
    assert_eq!(volume_of(&world, cx), 45);
    wheel(
        &mut vcx,
        (200.0, 40.0),
        ScrollDelta::Pixels(point(px(0.0), px(30.0))),
    );
    assert_eq!(volume_of(&world, cx), 50);

    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_VOLUME, false, cx));
    wheel(&mut vcx, (200.0, 40.0), ScrollDelta::Lines(point(0.0, 3.0)));
    assert_eq!(
        volume_of(&world, cx),
        50,
        "Show volume off leaves the wheel alone"
    );
}

// -- lyrics --

/// Synced lines every five seconds for the first track; nothing for the others.
struct Script;
impl LyricsSource for Script {
    fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        Ok((q.video_id == "track000000").then(|| {
            Lyrics::Synced(
                (0..6)
                    .map(|i| Line {
                        start_ms: 5_000 * (i + 1),
                        text: if i == 3 {
                            "   ".into()
                        } else {
                            format!("Line number {i}")
                        },
                    })
                    .collect(),
            )
        }))
    }
}

fn with_lyrics(world: &World, cx: &mut TestAppContext) {
    let service = Arc::new(LyricsService::new(vec![Arc::new(Script)], None));
    world
        .main
        .update(cx, |shell, _, cx| {
            shell.lyrics().update(cx, |l, cx| l.install(service, cx))
        })
        .unwrap();
    cx.run_until_parked();
}

/// Starts the first track and reports a position.
fn play_at(world: &World, cx: &mut TestAppContext, position_ms: u64) {
    let started = world
        .player
        .read_with(cx, |m, _| m.coordinator().unwrap().active_load());
    let load = match started {
        Some(load) => load,
        None => {
            world.player.update(cx, |m, cx| m.handle(Event::Play, cx));
            world
                .player
                .read_with(cx, |m, _| m.coordinator().unwrap().active_load().unwrap())
        }
    };
    world.player.update(cx, |m, cx| {
        if position_ms == 0 || m.view().position_ms == 0 {
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
        }
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Position(position_ms),
            },
            cx,
        );
    });
    cx.run_until_parked();
}

#[gpui::test]
fn the_current_synced_line_shows_under_the_artist_when_lyrics_are_on(cx: &mut TestAppContext) {
    let (world, handle, hand) = open_plain(cx);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    with_lyrics(&world, cx);
    play_at(&world, cx, 6_000);
    let line = |cx: &mut TestAppContext| {
        handle
            .read_with(cx, |m, _| m.lyric_line().map(str::to_owned))
            .unwrap()
    };
    assert_eq!(
        line(cx).as_deref(),
        Some("Line number 0"),
        "followed even while off"
    );
    assert!(
        vcx.debug_bounds("mini-lyric").is_none(),
        "Show lyrics is off by default"
    );

    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_LYRICS, true, cx));
    cx.run_until_parked();
    let row = vcx.debug_bounds("mini-lyric").expect("the lyric row");
    assert_eq!(f32::from(row.size.height), 16.0);
    // Under the title and artist, in the text column.
    assert!(f32::from(row.origin.y) > 40.0);
    assert!(f32::from(row.origin.x) > 90.0);

    play_at(&world, cx, 11_000);
    assert_eq!(line(cx).as_deref(), Some("Line number 1"));
    // The cross-fade runs and ends.
    hand.pass(0.1, cx);
    hand.pass(0.3, cx);
    assert_eq!(line(cx).as_deref(), Some("Line number 1"));

    // A blank line shows nothing; so does a track without synced lyrics.
    play_at(&world, cx, 21_000);
    assert_eq!(line(cx), None, "an instrumental gap is empty");
    assert!(
        vcx.debug_bounds("mini-lyric").is_some(),
        "the row keeps its room so the text does not jump"
    );
    play_at(&world, cx, 26_000);
    assert_eq!(line(cx).as_deref(), Some("Line number 4"));
    world.player.update(cx, |m, cx| m.next(cx));
    cx.run_until_parked();
    assert_eq!(line(cx), None, "track 1 has no lyrics");
}

/// One long line (too wide for the window), then a short one.
const LONG_LINE: &str = "Call it morning driving through the south in a rented car with the windows down and the radio loud";
struct LongScript;
impl LyricsSource for LongScript {
    fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        Ok((q.video_id == "track000000").then(|| {
            Lyrics::Synced(vec![
                Line {
                    start_ms: 5_000,
                    text: LONG_LINE.into(),
                },
                Line {
                    start_ms: 15_000,
                    text: "Short".into(),
                },
            ])
        }))
    }
}

fn with_long_lyrics(world: &World, cx: &mut TestAppContext) {
    let service = Arc::new(LyricsService::new(vec![Arc::new(LongScript)], None));
    world
        .main
        .update(cx, |shell, _, cx| {
            shell.lyrics().update(cx, |l, cx| l.install(service, cx))
        })
        .unwrap();
    cx.run_until_parked();
}

/// What the lyric line shows at each of `positions`, in order, with repeats folded away.
fn pages_over(
    world: &World,
    handle: WindowHandle<MiniPlayer>,
    cx: &mut TestAppContext,
    positions: &[u64],
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for &at in positions {
        play_at(world, cx, at);
        let line = handle
            .read_with(cx, |m, _| m.lyric_line().map(str::to_owned))
            .unwrap()
            .unwrap_or_default();
        if seen.last() != Some(&line) {
            seen.push(line);
        }
    }
    seen
}

#[gpui::test]
fn a_long_lyric_line_is_shown_in_pages_over_the_time_it_is_sung(cx: &mut TestAppContext) {
    let (world, handle, _hand) = open_plain(cx);
    with_long_lyrics(&world, cx);
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_LYRICS, true, cx));
    // Every half second across the line's ten seconds.
    let positions: Vec<u64> = (0..20).map(|i| 5_000 + i * 500).collect();
    let pages = pages_over(&world, handle, cx, &positions);
    assert!(pages.len() >= 2, "too long for one line: {pages:?}");
    assert!(
        pages
            .iter()
            .all(|p| !p.is_empty() && p.len() < LONG_LINE.len()),
        "each page is part of the line: {pages:?}"
    );
    assert_eq!(
        pages.join(" "),
        LONG_LINE,
        "in order, every word once, none lost"
    );
    // Before the line's first page ends, it is the first page.
    play_at(&world, cx, 5_100);
    let first = handle
        .read_with(cx, |m, _| m.lyric_line().map(str::to_owned))
        .unwrap();
    assert_eq!(first.as_deref(), Some(pages[0].as_str()));
    // The next line is short: one page, as it is.
    play_at(&world, cx, 15_500);
    assert_eq!(
        handle
            .read_with(cx, |m, _| m.lyric_line().map(str::to_owned))
            .unwrap()
            .as_deref(),
        Some("Short")
    );
}

#[gpui::test]
fn the_hud_pages_long_lyric_lines_too(cx: &mut TestAppContext) {
    let (world, handle, _hand) = open_hud(cx, None);
    with_long_lyrics(&world, cx);
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_LYRICS, true, cx));
    let positions: Vec<u64> = (0..20).map(|i| 5_000 + i * 500).collect();
    let pages = pages_over(&world, handle, cx, &positions);
    assert!(pages.len() >= 2, "the HUD pages it too: {pages:?}");
    assert_eq!(pages.join(" "), LONG_LINE);
}

#[gpui::test]
fn the_hud_shows_the_lyric_line_too(cx: &mut TestAppContext) {
    let (world, handle, _hand) = open_hud(cx, None);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    with_lyrics(&world, cx);
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_SHOW_LYRICS, true, cx));
    play_at(&world, cx, 6_000);
    assert_eq!(
        handle
            .read_with(cx, |m, _| m.lyric_line().map(str::to_owned))
            .unwrap()
            .as_deref(),
        Some("Line number 0")
    );
    let row = vcx.debug_bounds("mini-lyric").expect("the HUD's lyric row");
    let panel = vcx.debug_bounds("hud-panel").expect("panel");
    assert!(panel.contains(&row.origin) && panel.contains(&row.bottom_right()));
}

// -- collapse back to the mini player --

#[gpui::test]
fn closing_a_window_expanded_from_the_mini_player_collapses_back_to_it(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    open_mini(cx);
    assert!(
        !cx.update(|cx| mini::collapse_on_close(cx)),
        "nothing expanded yet"
    );
    cx.update(mini::return_to_main);
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));
    assert!(cx.update(|cx| mini::expanded_from_mini(cx)));

    // The close request (the X, Alt+F4, the taskbar) goes through the window's should-close.
    let mut main = VisualTestContext::from_window(world.main.into(), cx);
    assert!(!main.simulate_close(), "the window stays");
    cx.run_until_parked();
    assert!(cx.update(|cx| mini::is_open(cx)), "the mini player is back");
    assert!(!cx.update(|cx| mini::expanded_from_mini(cx)));
    // With the mini player showing again, there is nothing more to collapse.
    assert!(!cx.update(|cx| mini::collapse_on_close(cx)));
}

#[gpui::test]
fn showing_the_main_window_any_other_way_keeps_the_normal_close(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    open_mini(cx);
    cx.update(mini::return_to_main);
    assert!(cx.update(|cx| mini::expanded_from_mini(cx)));
    // The tray or a second launch asks for it: that is an ordinary window again.
    cx.update(|cx| mini::show_main(cx, world.main));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::expanded_from_mini(cx)));
    assert!(!cx.update(|cx| mini::collapse_on_close(cx)));
    let mut main = VisualTestContext::from_window(world.main.into(), cx);
    assert!(main.simulate_close(), "close closes");
}

#[gpui::test]
fn an_expanded_window_that_loses_focus_collapses_after_the_grace_period(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    open_mini(cx);
    cx.update(mini::return_to_main);
    cx.run_until_parked();
    // The main window is active after the expand; the user switches to another app.
    VisualTestContext::from_window(world.main.into(), cx).deactivate_window();
    cx.executor().advance_clock(Duration::from_millis(1_400));
    cx.run_until_parked();
    assert!(
        !cx.update(|cx| mini::is_open(cx)),
        "still in its grace period"
    );
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(cx.update(|cx| mini::is_open(cx)), "collapsed");
    assert!(!cx.update(|cx| mini::expanded_from_mini(cx)));
}

#[gpui::test]
fn focus_coming_back_in_time_cancels_the_collapse(cx: &mut TestAppContext) {
    let world = setup(cx, None);
    open_mini(cx);
    cx.update(mini::return_to_main);
    cx.run_until_parked();
    let mut main = VisualTestContext::from_window(world.main.into(), cx);
    main.deactivate_window();
    cx.executor().advance_clock(Duration::from_millis(1_000));
    main.update(|window, _| window.activate_window());
    main.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(2_000));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)), "it stayed expanded");
    assert!(cx.update(|cx| mini::expanded_from_mini(cx)));
    // Losing it again starts a fresh grace period.
    main.deactivate_window();
    cx.executor().advance_clock(Duration::from_millis(1_600));
    cx.run_until_parked();
    assert!(cx.update(|cx| mini::is_open(cx)));
}

#[gpui::test]
fn the_collapse_on_focus_loss_only_applies_to_an_expanded_window_with_the_setting_on(
    cx: &mut TestAppContext,
) {
    let world = setup(cx, None);
    // The main window shown any other way: losing focus does nothing.
    cx.update(|cx| mini::main_activation_changed(false, cx));
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));

    // Expanded, with the setting off: nothing either.
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_COLLAPSE_ON_BLUR, false, cx));
    open_mini(cx);
    cx.update(mini::return_to_main);
    cx.update(|cx| mini::main_activation_changed(false, cx));
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));

    // Turned off during the grace period, it is honoured when the time is up.
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_COLLAPSE_ON_BLUR, true, cx));
    cx.update(|cx| mini::main_activation_changed(false, cx));
    cx.executor().advance_clock(Duration::from_millis(500));
    world
        .settings
        .update(cx, |m, cx| m.set(&MINI_COLLAPSE_ON_BLUR, false, cx));
    cx.executor().advance_clock(Duration::from_millis(1_500));
    cx.run_until_parked();
    assert!(!cx.update(|cx| mini::is_open(cx)));
}

// -- Settings > Mini player --

#[gpui::test]
fn settings_has_a_mini_player_section_after_window_with_the_new_rows_in_order(
    cx: &mut TestAppContext,
) {
    use apollo_app::ui::nav::Route;
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    vcx.simulate_resize(size(px(1280.0), px(2600.0)));
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
    let ids = [
        "setting-start-minimized",
        "setting-mini-player-on-top",
        "setting-mini-fade-controls",
        "setting-mini-dim",
        "setting-mini-show-volume",
        "setting-mini-show-lyrics",
        "setting-mini-collapse-on-blur",
        "setting-mini-player-hud",
        "setting-hud-auto-hide",
        "setting-discord-presence",
    ];
    let mut last = f32::MIN;
    for id in ids {
        let top = f32::from(
            vcx.debug_bounds(id)
                .unwrap_or_else(|| panic!("{id} is on the page"))
                .origin
                .y,
        );
        assert!(top > last, "{id} is out of order");
        last = top;
    }
    let settings = shell.read_with(vcx, |s, cx| s.settings(cx));
    let defaults = settings.read_with(vcx, |m, _| {
        [
            m.get(&MINI_PLAYER_ON_TOP),
            m.get(&MINI_FADE_CONTROLS),
            m.get(&MINI_DIM),
            m.get(&MINI_SHOW_VOLUME),
            m.get(&MINI_SHOW_LYRICS),
            m.get(&MINI_COLLAPSE_ON_BLUR),
            m.get(&MINI_PLAYER_HUD),
            m.get(&HUD_AUTO_HIDE),
        ]
    });
    assert_eq!(defaults, [true, true, true, true, false, true, false, true]);
}
