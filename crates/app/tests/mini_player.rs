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
use apollo_app::hud::{DIM, HOLD};
use apollo_app::mini;
use apollo_app::player::PlayerModel;
use apollo_app::search::Unavailable;
use apollo_app::settings::{
    SettingsBackend, SettingsModel, HUD_AUTO_HIDE, MINI_PLAYER_HUD, MINI_PLAYER_ON_TOP,
};
use apollo_app::style::Fonts;
use apollo_app::ui::mini_player::{self, MiniPlayer};
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_app::window_state::{self, SavedWindow, KEY_MAIN, KEY_MINI};
use apollo_domain::model::Track;
use apollo_domain::playback::Intent;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, px, size, AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    TestAppContext, VisualTestContext, WindowHandle, WindowOptions,
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
    let vcx = VisualTestContext::from_window(handle.into(), cx);
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
    drop(world);
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
fn a_window_that_stops_moving_is_written_down_once(cx: &mut TestAppContext) {
    cx.update(Fonts::install);
    let store = MemoryStore::with(&[]);
    let (shell, vcx) = {
        let player = cx.new(|_| PlayerModel::new());
        cx.add_window_view(move |_, cx| Shell::new(player, cx))
    };
    let backend: Arc<dyn SettingsBackend> = store.clone();
    shell.update_in(vcx, |s, window, cx| {
        let sub = window_state::remember(window, cx, Some(backend), KEY_MAIN, true);
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
    let store = MemoryStore::with(&[(KEY_MINI, "1500,950,360,96,0")]);
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
    assert_eq!(
        (snap.x, snap.y),
        (1920.0 - 360.0 - 16.0, 1080.0 - 96.0 - 16.0),
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
