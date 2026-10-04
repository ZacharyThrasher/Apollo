//! Settings > Data: the Export library row (format select, Export… button, Windows save dialog)
//! and the Diagnostics report row (Create… button). The real shell and views on GPUI's test
//! platform, over the simulated library; the save dialog is the test platform's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{SettingsBackend, SettingsModel, EXPORT_FORMAT};
use apollo_app::style::Fonts;
use apollo_app::ui::data_page::{Job, BUTTON_EXPORT, BUTTON_REPORT, ROW_EXPORT, ROW_REPORT};
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

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

struct Page {
    shell: Entity<Shell>,
    settings: Entity<SettingsModel>,
    player: Entity<PlayerModel>,
    library: Arc<SimLibrary>,
}

fn setup(cx: &mut TestAppContext, install_library: bool) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let settings = cx.new(|_| SettingsModel::new(Some(Arc::new(Memory::default()))));
    let player = cx.new(|_| PlayerModel::new());
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
    let library = Arc::new(SimLibrary::instant());
    if install_library {
        let ready = Prepared {
            label: "test",
            ports: Ports {
                resolver: Box::new(MockResolver::default()),
                engine: Box::new(MockEngine::default()),
            },
            queue: vec![LibraryTrack {
                track: Track::new("track000001", "Song"),
            }],
            art: Arc::new(NoArt),
            search: Arc::new(NoSearch),
            thumbs: Arc::new(NoThumbs),
            library: library.clone(),
        };
        player.update(vcx, |m, cx| m.install(ready, cx));
    }
    vcx.run_until_parked();
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
    (
        Page {
            shell,
            settings,
            player,
            library,
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

/// The Data section sits at the bottom of the page: scroll down like a person would, until the
/// page stops moving, so the last row is clear of the player bar.
fn scroll_to_data(vcx: &mut VisualTestContext) {
    let mut last = None;
    for _ in 0..40 {
        let now = vcx.debug_bounds(ROW_REPORT).map(|b| b.top());
        if now.is_some() && now == last {
            return;
        }
        last = now;
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(500.0), gpui::px(300.0)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-200.0))),
            ..Default::default()
        });
        vcx.run_until_parked();
    }
}

fn click(vcx: &mut VisualTestContext, selector: &str) {
    scroll_to_data(vcx);
    let at = bounds(vcx, selector).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn data_jobs(page: &Page, vcx: &mut VisualTestContext) -> (Job, Job) {
    let data = page
        .shell
        .read_with(vcx, |s, cx| s.settings_page().read(cx).data_page().clone());
    data.read_with(vcx, |d, _| (d.export_job().clone(), d.report_job().clone()))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apollo-data-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn menu_labels(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    menu.read_with(vcx, |m, _| {
        m.labels().iter().map(|l| l.to_string()).collect()
    })
}

// ---- the section --------------------------------------------------------------------------------

#[gpui::test]
fn data_is_the_last_section_with_two_rows_and_two_buttons(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx, true);
    scroll_to_data(vcx);
    for id in [ROW_EXPORT, ROW_REPORT, BUTTON_EXPORT, BUTTON_REPORT] {
        assert!(on_screen(vcx, id), "{id} is on the page");
    }
    let hotkeys = bounds(vcx, "setting-global-hotkeys");
    let export = bounds(vcx, ROW_EXPORT);
    let report = bounds(vcx, ROW_REPORT);
    assert!(hotkeys.top() < export.top(), "Data comes after Keyboard");
    assert!(
        export.top() < report.top(),
        "Export library is the first row"
    );
    // Two 48px rows.
    assert_eq!(export.size.height, gpui::px(48.0));
    assert_eq!(report.size.height, gpui::px(48.0));
    // The button sits to the right of the format choice on the export row.
    assert!(
        bounds(vcx, &format!("{ROW_EXPORT}-value")).right() < bounds(vcx, BUTTON_EXPORT).left()
    );
}

#[gpui::test]
fn the_format_defaults_to_json_and_the_select_offers_both_and_remembers(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    assert_eq!(
        page.settings
            .read_with(vcx, |s, _| s.get_text(&EXPORT_FORMAT)),
        "json"
    );
    click(vcx, ROW_EXPORT);
    assert_eq!(menu_labels(&page, vcx), ["JSON", "CSV"]);
    click(vcx, "choice-menu-item-1");
    assert_eq!(
        page.settings
            .read_with(vcx, |s, _| s.get_text(&EXPORT_FORMAT)),
        "csv"
    );
}

#[gpui::test]
fn pressing_export_opens_the_row_s_button_not_the_format_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    click(vcx, BUTTON_EXPORT);
    assert!(vcx.did_prompt_for_new_path(), "the save dialog opens");
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    assert!(
        !menu.read_with(vcx, |m, _| m.is_open()),
        "the format popover stays shut"
    );
}

// ---- export -------------------------------------------------------------------------------------

#[gpui::test]
fn export_writes_json_to_the_chosen_file_without_touching_the_account(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    let dir = temp_dir("json");
    let target = dir.join("my-library.json");
    click(vcx, BUTTON_EXPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();

    let text = std::fs::read_to_string(&target).expect("the file is written");
    let json: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
    assert_eq!(json["format"], "apollo-library-export");
    assert!(!json["liked_songs"].as_array().unwrap().is_empty());
    assert!(!json["playlists"].as_array().unwrap().is_empty());
    assert!(!json["saved_albums"].as_array().unwrap().is_empty());
    assert!(page.library.write_log().is_empty(), "export is read-only");
    assert!(
        !dir.join("my-library.json.part").exists(),
        "no leftover part file"
    );

    // The simulation has one playlist made never to load: the row says what is missing.
    let (export, report) = data_jobs(&page, vcx);
    assert_eq!(export, Job::Failed("Saved without 1 item".into()));
    assert_eq!(report, Job::Idle);
    assert!(json["problems"][0]["what"]
        .as_str()
        .unwrap()
        .contains("Won't Load"));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn export_as_csv_follows_the_setting_and_adds_the_extension(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    page.settings
        .update(vcx, |s, cx| s.set_text(&EXPORT_FORMAT, "csv", cx));
    let dir = temp_dir("csv");
    click(vcx, BUTTON_EXPORT);
    let base = dir.join("mine");
    vcx.simulate_new_path_selection(move |_| Some(base.clone()));
    vcx.run_until_parked();
    let written = dir.join("mine.csv");
    let text = std::fs::read_to_string(&written).expect("a path with no extension gets .csv");
    assert!(text.starts_with('\u{feff}'));
    assert!(text.contains("liked_song,Liked Music,LM,1,"));
    assert!(text.contains("playlist_song,"));
    assert!(text.contains("saved_album,"));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn while_it_runs_a_second_press_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    let dir = temp_dir("running");
    let target = dir.join("library.json");
    click(vcx, BUTTON_EXPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    // Step the executor until the job has started but not finished.
    let mut seen_running = false;
    for _ in 0..2000 {
        if !vcx.executor().tick() {
            break;
        }
        if data_jobs(&page, vcx).0 == Job::Running {
            seen_running = true;
            break;
        }
    }
    assert!(seen_running, "the export is running at some point");
    // A second press while it runs opens no second dialog and starts nothing.
    let data = page
        .shell
        .read_with(vcx, |s, cx| s.settings_page().read(cx).data_page().clone());
    data.update(vcx, |d, cx| d.start_export(cx));
    assert!(!vcx.did_prompt_for_new_path(), "no second dialog");
    vcx.run_until_parked();
    assert!(target.exists(), "the first export still finishes");
    assert_ne!(data_jobs(&page, vcx).0, Job::Running);
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn cancelling_the_dialog_changes_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    click(vcx, BUTTON_EXPORT);
    vcx.simulate_new_path_selection(|_| None);
    vcx.run_until_parked();
    assert_eq!(data_jobs(&page, vcx), (Job::Idle, Job::Idle));
}

#[gpui::test]
fn a_failed_export_shows_in_the_row_and_the_next_try_clears_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    page.library
        .set_collections_failure(Some(LoadFailure::Network));
    let dir = temp_dir("fail");
    let target = dir.join("never.json");
    click(vcx, BUTTON_EXPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();
    assert_eq!(
        data_jobs(&page, vcx).0,
        Job::Failed("Connection problem".into())
    );
    assert!(!target.exists(), "a failed export writes nothing");

    // Fixed: the next export works and the message is gone.
    page.library.set_collections_failure(None);
    click(vcx, BUTTON_EXPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();
    assert!(target.exists());
    assert_ne!(
        data_jobs(&page, vcx).0,
        Job::Failed("Connection problem".into())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn export_before_the_library_is_ready_says_so_and_opens_no_dialog(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, false);
    click(vcx, BUTTON_EXPORT);
    assert!(!vcx.did_prompt_for_new_path());
    assert_eq!(data_jobs(&page, vcx).0, Job::Failed("Not ready yet".into()));
    let _ = &page.player;
}

#[gpui::test]
fn an_unwritable_destination_is_reported_and_leaves_no_part_file(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    let dir = temp_dir("unwritable");
    let target = dir.join("no-such-folder").join("library.json");
    click(vcx, BUTTON_EXPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();
    assert_eq!(
        data_jobs(&page, vcx).0,
        Job::Failed("Couldn't write the file".into())
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- diagnostics report -------------------------------------------------------------------------

#[gpui::test]
fn create_writes_a_scrubbed_text_report_to_the_chosen_file(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    // Something that must not survive, printed the way the app prints.
    apollo_app::diagnostics::log::record(
        "resolve failed: Cookie: SAPISID=AbC123xyz https://rr1.googlevideo.com/videoplayback?sig=AOq0QJ8wRQ me@example.com 203.0.113.9",
    );
    let dir = temp_dir("report");
    let target = dir.join("report.txt");
    click(vcx, BUTTON_REPORT);
    assert!(vcx.did_prompt_for_new_path());
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();

    let text = std::fs::read_to_string(&target).expect("the report is written");
    assert!(text.starts_with("Apollo diagnostics report"));
    assert!(text.contains("== Settings =="));
    assert!(text.contains("resolve failed"), "the line itself is kept");
    for secret in [
        "AbC123xyz",
        "AOq0QJ8wRQ",
        "me@example.com",
        "203.0.113.9",
        "googlevideo",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into the report");
    }
    assert_eq!(data_jobs(&page, vcx), (Job::Idle, Job::Idle));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
fn a_report_that_cannot_be_written_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    let dir = temp_dir("report-fail");
    let target = dir.join("missing").join("report.txt");
    click(vcx, BUTTON_REPORT);
    let chosen = target.clone();
    vcx.simulate_new_path_selection(move |_| Some(chosen.clone()));
    vcx.run_until_parked();
    assert_eq!(
        data_jobs(&page, vcx).1,
        Job::Failed("Couldn't write the file".into())
    );
    let _ = std::fs::remove_dir_all(dir);
}
