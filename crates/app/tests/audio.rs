//! The audio toolbox in the app: the saved output device and equalizer reach the engine at
//! install and when they change, Settings > Audio shows the device row and the equalizer row
//! and its editor, and the speed button in Now Playing. The real shell and views on GPUI's test
//! platform; the engine is the mock with a record of what it was told.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::audio as glue;
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{
    SettingsBackend, SettingsModel, AUDIO_DEVICE, EQUALIZER_ENABLED, EQUALIZER_GAINS,
};
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::audio::{AudioDevice, Equalizer, Preset};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{LeaseId, LoadId};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_runtime::MediaEngine;
use gpui::{px, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

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
struct NoSearch;
impl SearchSource for NoSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }
}

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

/// What the engine was told, in order.
#[derive(Default)]
struct Told {
    devices: Vec<Option<String>>,
    equalizers: Vec<Equalizer>,
    speeds: Vec<f32>,
}

/// The mock engine, a list of output devices, and a record of the audio calls.
struct Listening {
    inner: MockEngine,
    told: Arc<Mutex<Told>>,
}

impl MediaEngine for Listening {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        self.inner.open(load, lease, start_paused, start_ms)
    }
    fn pause(&mut self, load: LoadId) {
        self.inner.pause(load)
    }
    fn resume(&mut self, load: LoadId) {
        self.inner.resume(load)
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        self.inner.seek(load, generation, position_ms)
    }
    fn close(&mut self, load: LoadId) {
        self.inner.close(load)
    }
    fn stop(&mut self) {
        self.inner.stop()
    }
    fn set_audio_device(&mut self, device: Option<String>) {
        self.told.lock().unwrap().devices.push(device);
    }
    fn audio_devices(&self) -> Vec<AudioDevice> {
        vec![
            AudioDevice {
                id: "wasapi/speakers".into(),
                name: "Speakers".into(),
            },
            AudioDevice {
                id: "wasapi/headset".into(),
                name: "Headset".into(),
            },
        ]
    }
    fn set_equalizer(&mut self, eq: Equalizer) {
        self.told.lock().unwrap().equalizers.push(eq);
    }
    fn set_speed(&mut self, speed: f32) {
        self.told.lock().unwrap().speeds.push(speed);
    }
}

struct Page {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    shell: Entity<Shell>,
    backend: Arc<Memory>,
    told: Arc<Mutex<Told>>,
}

fn prepared(told: &Arc<Mutex<Told>>) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(Listening {
                inner: MockEngine::default(),
                told: told.clone(),
            }),
        },
        queue: vec![LibraryTrack {
            track: Track::new("track000001", "Song"),
        }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    }
}

/// A shell over saved settings (`saved` are written before anything starts), the player bound
/// to them as the app does, and the engine installed.
fn setup<'a>(
    cx: &'a mut TestAppContext,
    saved: &[(&str, &str)],
) -> (Page, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let backend = Arc::new(Memory::default());
    for (k, v) in saved {
        backend.set(k, v);
    }
    let told = Arc::new(Mutex::new(Told::default()));
    let settings = cx.new(|_| SettingsModel::new(Some(backend.clone())));
    let player = cx.new(|_| PlayerModel::new());
    cx.update(|cx| glue::bind(&player, &settings, cx));
    let (shell, vcx) = cx.add_window_view({
        let (player, settings) = (player.clone(), settings.clone());
        move |_, cx| {
            let ports = apollo_app::backend::account::ports();
            apollo_app::backend::account::start(&ports, true);
            let account = cx.new(|cx| apollo_app::account::AccountModel::new(ports, cx));
            account.update(cx, |m, cx| m.startup_finished(cx));
            Shell::with_settings(player, account, settings, cx)
        }
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = prepared(&told);
    player.update(vcx, |m, cx| m.install(ready, cx));
    vcx.run_until_parked();
    (
        Page {
            player,
            settings,
            shell,
            backend,
            told,
        },
        vcx,
    )
}

fn bounds(vcx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
}

fn on_screen(vcx: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    vcx.debug_bounds(selector).is_some()
}

fn click(vcx: &mut VisualTestContext, selector: &str) {
    let at = bounds(vcx, selector).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn settings_page(page: &Page, vcx: &mut VisualTestContext) {
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
}

fn menu_labels(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    menu.read_with(vcx, |m, _| {
        m.labels().iter().map(|l| l.to_string()).collect()
    })
}

fn menu_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    menu.read_with(vcx, |m, _| m.is_open())
}

fn saved_eq(page: &Page, vcx: &mut VisualTestContext) -> Equalizer {
    page.settings.read_with(vcx, |s, _| s.equalizer())
}

// ---- the saved settings reach the engine --------------------------------------------------------

#[gpui::test]
fn nothing_saved_means_the_default_device_a_flat_off_equalizer_and_normal_speed(
    cx: &mut TestAppContext,
) {
    let (page, _vcx) = setup(cx, &[]);
    let told = page.told.lock().unwrap();
    assert_eq!(told.devices, [None]);
    assert_eq!(told.equalizers, [Equalizer::flat()]);
    assert_eq!(told.speeds, [1.0]);
}

#[gpui::test]
fn a_saved_device_and_curve_reach_the_engine_at_install(cx: &mut TestAppContext) {
    let (page, _vcx) = setup(
        cx,
        &[
            (AUDIO_DEVICE.key, "wasapi/headset"),
            (EQUALIZER_ENABLED.key, "true"),
            (
                EQUALIZER_GAINS.key,
                &Equalizer::gains_to_text(&Preset::BassBoost.gains()),
            ),
        ],
    );
    let told = page.told.lock().unwrap();
    assert_eq!(told.devices, [Some("wasapi/headset".to_owned())]);
    let eq = told.equalizers.last().unwrap();
    assert!(eq.enabled);
    assert_eq!(eq.gains_db, Preset::BassBoost.gains());
}

#[gpui::test]
fn a_change_in_the_saved_settings_follows_to_the_engine_but_an_unrelated_save_does_not_undo_a_drag(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, &[]);
    // A drag in progress: the player has it, the settings do not yet.
    let mut gains = Equalizer::flat().gains_db;
    gains[3] = 5.0;
    let live = Equalizer {
        enabled: true,
        gains_db: gains,
    };
    page.player.update(vcx, |p, cx| p.set_equalizer(live, cx));
    // Some other setting is saved.
    page.settings.update(vcx, |s, cx| {
        s.set(&apollo_app::settings::CLOSE_TO_TRAY, false, cx)
    });
    vcx.run_until_parked();
    assert_eq!(page.player.read_with(vcx, |p, _| p.equalizer()), live);

    // The curve itself saved: the player follows (it is already there).
    vcx.update(|_, cx| glue::save_equalizer(&page.settings, live, cx));
    vcx.run_until_parked();
    assert_eq!(page.player.read_with(vcx, |p, _| p.equalizer()), live);
    // And a change saved from elsewhere reaches the engine.
    let other = Equalizer {
        enabled: true,
        gains_db: Preset::Vocal.gains(),
    };
    page.settings.update(vcx, |s, cx| {
        s.set_text(
            &EQUALIZER_GAINS,
            &Equalizer::gains_to_text(&other.gains_db),
            cx,
        )
    });
    vcx.run_until_parked();
    assert_eq!(page.player.read_with(vcx, |p, _| p.equalizer()), other);
    assert_eq!(*page.told.lock().unwrap().equalizers.last().unwrap(), other);
}

// ---- Settings > Audio ---------------------------------------------------------------------------

#[gpui::test]
fn settings_has_an_audio_section_with_the_device_and_the_equalizer(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    for id in [
        "setting-output-device",
        "setting-equalizer",
        "setting-equalizer-edit",
    ] {
        assert!(on_screen(vcx, id), "{id} is on the page");
    }
    let device = bounds(vcx, "setting-output-device");
    let eq = bounds(vcx, "setting-equalizer");
    assert!(device.top() < eq.top(), "the device row comes first");
    let playback = bounds(vcx, "setting-gapless-playback");
    assert!(playback.top() < device.top(), "Audio comes after Playback");
    assert!(!on_screen(vcx, "equalizer-panel"), "the editor is closed");
}

#[gpui::test]
fn the_equalizer_edit_button_sits_beside_its_switch_not_in_the_middle_of_the_row(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    let row = bounds(vcx, "setting-equalizer");
    let edit = bounds(vcx, "setting-equalizer-edit");
    let switch = bounds(vcx, "setting-equalizer-switch");
    assert!(edit.right() <= switch.left(), "Edit is left of the switch");
    assert!(
        switch.left() - edit.right() <= px(24.0),
        "and right beside it, not spread across the row"
    );
    assert!(
        edit.left() > row.left() + row.size.width * 0.6,
        "at the row's right end"
    );
    // The row's own click is aimed at its centre: that must not be the Edit button.
    assert!(!edit.contains(&row.center()), "the centre of the row is not Edit");
}

#[gpui::test]
fn the_device_row_lists_the_system_default_then_the_devices_and_choosing_saves_and_applies(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    click(vcx, "setting-output-device");
    assert!(menu_open(&page, vcx));
    assert_eq!(
        menu_labels(&page, vcx),
        ["System default", "Speakers", "Headset"]
    );

    click(vcx, "choice-menu-item-2");
    assert!(!menu_open(&page, vcx), "choosing closes the menu");
    assert_eq!(
        page.player
            .read_with(vcx, |p, _| p.audio_device().map(str::to_owned)),
        Some("wasapi/headset".to_owned())
    );
    assert_eq!(
        page.told.lock().unwrap().devices.last().unwrap().as_deref(),
        Some("wasapi/headset")
    );
    assert_eq!(
        page.settings.read_with(vcx, |s, _| s.audio_device()),
        Some("wasapi/headset".to_owned())
    );
    // The backend writes on a thread; the cache is what the app reads.
    let _ = &page.backend;

    // The row now says which device plays, and the menu checks it.
    click(vcx, "setting-output-device");
    click(vcx, "choice-menu-item-0");
    assert_eq!(
        page.player
            .read_with(vcx, |p, _| p.audio_device().map(str::to_owned)),
        None
    );
    assert_eq!(page.settings.read_with(vcx, |s, _| s.audio_device()), None);
}

#[gpui::test]
fn a_saved_device_that_is_not_plugged_in_reads_as_the_system_default(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[(AUDIO_DEVICE.key, "wasapi/unplugged")]);
    settings_page(&page, vcx);
    assert_eq!(
        page.player.read_with(vcx, |p, _| p.audio_device_label()),
        "System default"
    );
    // The choice is kept for when it comes back.
    assert_eq!(
        page.settings.read_with(vcx, |s, _| s.audio_device()),
        Some("wasapi/unplugged".to_owned())
    );
}

#[gpui::test]
fn the_equalizer_switch_turns_it_on_and_off_and_remembers(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    assert!(!saved_eq(&page, vcx).enabled, "off by default");

    // Left of the Edit button, on the row itself.
    let row = bounds(vcx, "setting-equalizer");
    vcx.simulate_click(
        gpui::point(row.left() + row.size.width / 4.0, row.center().y),
        Modifiers::none(),
    );
    vcx.run_until_parked();
    assert!(page.player.read_with(vcx, |p, _| p.equalizer().enabled));
    assert!(saved_eq(&page, vcx).enabled);
    assert!(page.told.lock().unwrap().equalizers.last().unwrap().enabled);

    vcx.simulate_click(
        gpui::point(row.left() + row.size.width / 4.0, row.center().y),
        Modifiers::none(),
    );
    vcx.run_until_parked();
    assert!(!page.player.read_with(vcx, |p, _| p.equalizer().enabled));
    assert!(!saved_eq(&page, vcx).enabled);
}

#[gpui::test]
fn edit_opens_the_editor_without_flipping_the_switch_and_done_closes_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    click(vcx, "setting-equalizer-edit");
    assert!(on_screen(vcx, "equalizer-panel"), "the editor opens");
    assert!(
        !saved_eq(&page, vcx).enabled,
        "the Edit click did not flip the switch"
    );
    assert!(!page.player.read_with(vcx, |p, _| p.equalizer().enabled));

    for band in 0..10 {
        assert!(on_screen(vcx, &format!("eq-band-{band}")), "band {band}");
    }
    for preset in Preset::ALL {
        assert!(
            on_screen(vcx, &format!("eq-preset-{}", preset.name())),
            "{}",
            preset.name()
        );
    }

    click(vcx, "setting-equalizer-edit");
    assert!(!on_screen(vcx, "equalizer-panel"), "Done closes it");
}

#[gpui::test]
fn a_preset_sets_the_curve_turns_the_equalizer_on_and_is_saved(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    settings_page(&page, vcx);
    click(vcx, "setting-equalizer-edit");

    click(vcx, "eq-preset-Bass boost");
    let eq = page.player.read_with(vcx, |p, _| p.equalizer());
    assert!(eq.enabled, "an audible change turns it on");
    assert_eq!(eq.gains_db, Preset::BassBoost.gains());
    assert_eq!(saved_eq(&page, vcx), eq);
    assert_eq!(*page.told.lock().unwrap().equalizers.last().unwrap(), eq);

    // Reset flattens the curve and leaves the switch where it is.
    click(vcx, "eq-reset");
    let eq = page.player.read_with(vcx, |p, _| p.equalizer());
    assert!(eq.enabled);
    assert!(eq.is_flat());
    assert!(saved_eq(&page, vcx).is_flat());
}

#[gpui::test]
fn dragging_a_slider_moves_the_sound_live_and_saves_when_let_go(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    // Tall enough that the editor is on screen however many sections Settings has above it.
    vcx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(1800.0)));
    settings_page(&page, vcx);
    click(vcx, "setting-equalizer-edit");

    let band = bounds(vcx, "eq-band-0");
    // The slider sits in the middle of the band's column, under the value caption.
    let column_x = band.center().x;
    let top = band.top() + gpui::px(16.0 + 4.0);
    let at = |frac: f32| gpui::point(column_x, top + gpui::px(160.0 * (1.0 - frac)));

    vcx.simulate_mouse_move(at(0.5), None, Modifiers::none());
    vcx.simulate_mouse_down(at(0.5), gpui::MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(at(0.75), None, Modifiers::none());
    vcx.run_until_parked();
    let live = page.player.read_with(vcx, |p, _| p.equalizer());
    assert!(
        live.gains_db[0] > 0.0,
        "the player follows the drag: {:?}",
        live.gains_db
    );
    assert!(
        !saved_eq(&page, vcx).enabled && saved_eq(&page, vcx).is_flat(),
        "nothing is written while the drag is on"
    );

    vcx.simulate_mouse_up(at(0.75), gpui::MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    let after = page.player.read_with(vcx, |p, _| p.equalizer());
    assert!(after.enabled);
    assert!(after.gains_db[0] > 0.0);
    assert_eq!(saved_eq(&page, vcx), after, "let go: saved");
    assert_eq!(after.gains_db[1..], [0.0; 9], "only that band moved");
}

// ---- playback speed -----------------------------------------------------------------------------

#[gpui::test]
fn the_speed_button_offers_four_speeds_and_picking_one_sets_it_without_saving(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, &[]);
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "np-speed"), "the button is in Now Playing");

    click(vcx, "np-speed");
    assert!(menu_open(&page, vcx));
    assert_eq!(
        menu_labels(&page, vcx),
        ["0.75\u{d7}", "1.0\u{d7}", "1.25\u{d7}", "1.5\u{d7}"]
    );
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    assert_eq!(
        menu.read_with(vcx, |m, _| m.cursor()),
        Some(1),
        "1.0\u{d7} is in force"
    );

    click(vcx, "choice-menu-item-3");
    assert_eq!(page.player.read_with(vcx, |p, _| p.speed()), 1.5);
    assert_eq!(*page.told.lock().unwrap().speeds.last().unwrap(), 1.5);

    // Speed is not a setting: nothing was written for it.
    let written = page
        .backend
        .0
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert!(written.iter().all(|k| !k.contains("speed")), "{written:?}");
}

#[gpui::test]
fn a_new_player_starts_at_normal_speed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, &[]);
    assert_eq!(page.player.read_with(vcx, |p, _| p.speed()), 1.0);
}
