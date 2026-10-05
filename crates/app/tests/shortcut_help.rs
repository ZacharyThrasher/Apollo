//! Shortcut help: tooltips and menu rows that name their key, the `?` cheat sheet, the Ctrl-held
//! key badges and the third-use tip. The real shell on GPUI's test platform; the keys shown are
//! whatever the keymap holds.

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::keymap::CrateCurrent;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{
    SettingsModel, KEY_BADGES, SHORTCUT_HINT_STATE, SHOW_SHORTCUT_HINTS,
};
use apollo_app::shortcuts::{self, Group, Scope};
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell, BADGE_AFTER};
use apollo_app::ui::track_menu::{self, Anchor, Command, Subject};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, px, AppContext, Entity, KeyBinding, Modifiers, ModifiersChangedEvent, TestAppContext,
    VisualTestContext,
};

struct NoArt;
impl ArtSource for NoArt {
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

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    settings: Entity<SettingsModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let sim = SimLibrary::instant();
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: [("firstTrack01", "First"), ("secondTrack1", "Second")]
            .into_iter()
            .map(|(id, title)| LibraryTrack {
                track: apollo_domain::model::Track::new(id, title),
            })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let settings = shell.read_with(vcx, |s, cx| s.settings(cx));
    (
        Page {
            player,
            shell,
            settings,
        },
        vcx,
    )
}

fn tip(label: &str, scope: Scope, vcx: &mut VisualTestContext) -> String {
    vcx.update(|_, cx| shortcuts::tip_text(label, scope, cx).to_string())
}

fn on_screen(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn ctrl(vcx: &mut VisualTestContext, down: bool) {
    vcx.simulate_event(ModifiersChangedEvent {
        modifiers: if down {
            Modifiers::control()
        } else {
            Modifiers::none()
        },
        capslock: Default::default(),
    });
}

#[gpui::test]
fn tooltips_end_with_the_key_the_keymap_has(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx);
    assert_eq!(tip("Play", Scope::Shell, vcx), "Play  Space");
    assert_eq!(tip("Pause", Scope::Shell, vcx), "Pause  Space");
    assert_eq!(tip("Next", Scope::Shell, vcx), "Next  Shift+→");
    assert_eq!(tip("Previous", Scope::Shell, vcx), "Previous  Shift+←");
    assert_eq!(tip("Mute", Scope::Shell, vcx), "Mute  M");
    assert_eq!(tip("Unmute", Scope::Shell, vcx), "Unmute  M");
    assert_eq!(tip("Lyrics", Scope::Shell, vcx), "Lyrics  Ctrl+L");
    assert_eq!(tip("Search", Scope::Shell, vcx), "Search  Ctrl+K");
    assert_eq!(tip("Like", Scope::Shell, vcx), "Like  L");
    assert_eq!(tip("Unlike", Scope::Shell, vcx), "Unlike  L");
    // A row's actions also have their list's keys.
    assert_eq!(tip("Add to Crate", Scope::Any, vcx), "Add to Crate  C");
    assert_eq!(tip("Add to playlist", Scope::Any, vcx), "Add to playlist  P");
    // No command, no key.
    assert_eq!(tip("Queue", Scope::Shell, vcx), "Queue");
}

#[gpui::test]
fn the_key_shown_is_whatever_the_keymap_says(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx);
    vcx.update(|_, cx| {
        cx.clear_key_bindings();
        cx.bind_keys([KeyBinding::new("x", CrateCurrent, Some("Shell"))]);
    });
    assert_eq!(tip("Add to Crate", Scope::Shell, vcx), "Add to Crate  X");
    vcx.update(|_, cx| cx.clear_key_bindings());
    assert_eq!(tip("Add to Crate", Scope::Shell, vcx), "Add to Crate");
}

#[gpui::test]
fn show_shortcut_hints_off_leaves_plain_tooltips(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.settings
        .update(vcx, |m, cx| m.set(&SHOW_SHORTCUT_HINTS, false, cx));
    assert_eq!(tip("Mute", Scope::Shell, vcx), "Mute");
    vcx.update(|_, cx| assert!(shortcuts::menu_key("like", cx).is_none()));
    page.settings
        .update(vcx, |m, cx| m.set(&SHOW_SHORTCUT_HINTS, true, cx));
    assert_eq!(tip("Mute", Scope::Shell, vcx), "Mute  M");
}

#[gpui::test]
fn menu_rows_show_their_key_at_the_right(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |_, window, cx| {
        track_menu::open(
            Subject::track(apollo_domain::model::Track::new("menuTrack1", "Menu"), None),
            Anchor::Point(point(px(300.), px(200.))),
            window,
            cx,
        );
    });
    vcx.run_until_parked();
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    let commands = menu.read_with(vcx, |m, cx| m.commands(cx));
    let selector = |command: Command| -> &'static str {
        let ix = commands.iter().position(|c| *c == command).expect("in menu");
        Box::leak(format!("track-menu-key-{ix}").into_boxed_str())
    };
    assert!(on_screen(vcx, selector(Command::AddToCrate)));
    assert!(on_screen(vcx, selector(Command::ToggleLike)));
    assert!(on_screen(vcx, selector(Command::AddToPlaylist)));
    // Right-aligned: the key sits at the menu's right edge.
    let panel = vcx.debug_bounds("track-menu").expect("menu");
    let key = vcx.debug_bounds(selector(Command::AddToCrate)).expect("key");
    assert!(key.right() <= panel.right());
    assert!(key.left() > panel.left() + panel.size.width / 2.0);
    // A row with no key has none.
    let play_next = commands
        .iter()
        .position(|c| *c == Command::PlayNext)
        .expect("play next");
    assert!(vcx
        .debug_bounds(Box::leak(format!("track-menu-key-{play_next}").into_boxed_str()))
        .is_none());
}

#[gpui::test]
fn question_mark_opens_the_cheat_sheet_and_escape_closes_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let sheet = page.shell.read_with(vcx, |s, _| s.cheat_sheet().clone());
    assert!(!sheet.read_with(vcx, |s, _| s.is_open()));
    vcx.simulate_keystrokes("shift-/");
    assert!(sheet.read_with(vcx, |s, _| s.is_open()), "? opens it");
    assert!(on_screen(vcx, "cheat-sheet"));
    for group in ["play", "collect", "navigate", "window"] {
        let selector: &'static str = Box::leak(format!("cheat-group-{group}").into_boxed_str());
        assert!(on_screen(vcx, selector), "{group} group");
    }
    // The keys behind it stand down.
    vcx.simulate_keystrokes("m");
    assert!(!page.player.read_with(vcx, |m, _| m.view().muted));
    vcx.simulate_keystrokes("escape");
    assert!(!sheet.read_with(vcx, |s, _| s.is_open()), "Esc closes it");
    assert!(!on_screen(vcx, "cheat-sheet"));
    // Opened again, ? closes it.
    vcx.simulate_keystrokes("shift-/ shift-/");
    assert!(!sheet.read_with(vcx, |s, _| s.is_open()));
    // And now the keys work again.
    vcx.simulate_keystrokes("m");
    assert!(page.player.read_with(vcx, |m, _| m.view().muted));
}

#[gpui::test]
fn question_mark_in_a_text_field_is_a_question_mark(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("shift-/");
    let sheet = page.shell.read_with(vcx, |s, _| s.cheat_sheet().clone());
    assert!(!sheet.read_with(vcx, |s, _| s.is_open()));
}

#[gpui::test]
fn the_sheet_lists_each_group_with_its_keys_and_unnamed_letters(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx);
    let sheet = vcx.update(|_, cx| shortcuts::sheet(cx));
    let groups: Vec<Group> = sheet.iter().map(|(g, _)| *g).collect();
    assert_eq!(
        groups,
        [Group::Play, Group::Collect, Group::Navigate, Group::Window]
    );
    let keys_of = |label: &str| -> Vec<String> {
        sheet
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("{label} is listed"))
            .keys
            .clone()
    };
    assert_eq!(keys_of("Play / pause"), ["Space"]);
    assert_eq!(keys_of("Next song"), ["Shift+→"], "just the main key");
    assert_eq!(keys_of("Seek 5 s"), ["Ctrl+←", "Ctrl+→"]);
    assert_eq!(keys_of("Volume 5%"), ["Ctrl+↑", "Ctrl+↓"]);
    assert_eq!(keys_of("Move around (lists, cards, sidebar)"), ["↑", "↓", "←", "→"]);
    assert_eq!(keys_of("Search"), ["Ctrl+K", "Ctrl+F"]);
    assert_eq!(keys_of("Redo"), ["Ctrl+Shift+Z", "Ctrl+Y"]);
    assert_eq!(keys_of("Add to Crate"), ["C"]);
    assert_eq!(keys_of("This list"), ["?"]);

    // A letter a page binds that no entry names shows up by itself, named after its action.
    gpui::actions!(shortcut_help_test, [SkimNow]);
    vcx.update(|_, cx| cx.bind_keys([KeyBinding::new("k", SkimNow, Some("Shell"))]));
    let sheet = vcx.update(|_, cx| shortcuts::sheet(cx));
    let more = sheet
        .iter()
        .find(|(g, _)| *g == Group::More)
        .expect("a More group");
    assert!(more
        .1
        .iter()
        .any(|r| r.label == "Skim now" && r.keys == ["K"]));
}

#[gpui::test]
fn holding_ctrl_shows_key_badges_and_letting_go_hides_them(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx);
    assert!(!on_screen(vcx, "key-badge"));
    ctrl(vcx, true);
    vcx.executor().advance_clock(Duration::from_millis(300));
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "key-badge"), "not yet: a quick tap shows nothing");
    vcx.executor().advance_clock(BADGE_AFTER);
    vcx.run_until_parked();
    assert!(on_screen(vcx, "key-badge"), "held long enough");
    ctrl(vcx, false);
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "key-badge"), "released");
}

#[gpui::test]
fn a_tap_of_ctrl_never_shows_badges_and_a_shortcut_cancels_them(cx: &mut TestAppContext) {
    let (_page, vcx) = setup(cx);
    ctrl(vcx, true);
    ctrl(vcx, false);
    vcx.executor().advance_clock(BADGE_AFTER * 2);
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "key-badge"));

    ctrl(vcx, true);
    vcx.executor().advance_clock(BADGE_AFTER * 2);
    vcx.run_until_parked();
    assert!(on_screen(vcx, "key-badge"));
    vcx.simulate_keystrokes("ctrl-z");
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "key-badge"), "using a shortcut puts them away");
}

#[gpui::test]
fn the_badges_have_their_own_switch(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.settings.update(vcx, |m, cx| m.set(&KEY_BADGES, false, cx));
    ctrl(vcx, true);
    vcx.executor().advance_clock(BADGE_AFTER * 2);
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "key-badge"));
}

#[gpui::test]
fn the_third_mouse_use_shows_a_tip_once_ever(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let pill = |vcx: &mut VisualTestContext| page.shell.read_with(vcx, |s, _| s.pill_text());
    for _ in 0..2 {
        vcx.update(|_, cx| shortcuts::note_mouse_use("crate", Scope::Any, cx));
    }
    vcx.run_until_parked();
    assert_eq!(pill(vcx), None, "two uses: nothing yet");
    vcx.update(|_, cx| shortcuts::note_mouse_use("crate", Scope::Any, cx));
    vcx.run_until_parked();
    assert_eq!(pill(vcx).as_deref(), Some("Tip: press C"));
    // It stays about four seconds, then goes.
    vcx.executor().advance_clock(Duration::from_secs(3));
    assert_eq!(pill(vcx).as_deref(), Some("Tip: press C"));
    vcx.executor().advance_clock(Duration::from_secs(2));
    vcx.run_until_parked();
    assert_eq!(pill(vcx), None);
    // Never again for that command.
    vcx.update(|_, cx| shortcuts::note_mouse_use("crate", Scope::Any, cx));
    vcx.run_until_parked();
    assert_eq!(pill(vcx), None);
    // What was shown is remembered between runs.
    let state = page
        .settings
        .read_with(vcx, |m, _| m.get_text(&SHORTCUT_HINT_STATE));
    assert!(state.contains("crate:"), "{state}");
}

#[gpui::test]
fn clicking_the_heart_three_times_with_the_mouse_names_its_key(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    for _ in 0..3 {
        let at = vcx.debug_bounds("bar-like").expect("heart").center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.pill_text()).as_deref(),
        Some("Tip: press L")
    );
}

#[gpui::test]
fn the_key_does_not_count_and_hints_off_counts_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.settings
        .update(vcx, |m, cx| m.set(&SHOW_SHORTCUT_HINTS, false, cx));
    for _ in 0..5 {
        vcx.update(|_, cx| shortcuts::note_mouse_use("crate", Scope::Any, cx));
    }
    vcx.run_until_parked();
    assert_eq!(page.shell.read_with(vcx, |s, _| s.pill_text()), None);
    // The keyboard never counts: pressing L with the key leaves no tip.
    page.settings
        .update(vcx, |m, cx| m.set(&SHOW_SHORTCUT_HINTS, true, cx));
    vcx.simulate_keystrokes("l l l l");
    vcx.run_until_parked();
    assert!(page
        .shell
        .read_with(vcx, |s, _| s.pill_text())
        .is_none_or(|t| !t.starts_with("Tip")));
}
