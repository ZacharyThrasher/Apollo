//! Swap to explicit (and clean) versions in the Crate: the header's More menu, a row's menu, the
//! tip pill that counts along, picks that survive, and one Ctrl+Z for a whole batch. The real
//! shell and views on GPUI's test platform, with a scripted search.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::PREFER_EXPLICIT;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Anchor, Command, Subject};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{ArtistRef, Track, VideoId};
use apollo_provider::{ListedTrack, SearchFilter, SearchPage, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{point, px, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

fn song(id: &str, title: &str, secs: u64) -> Track {
    Track {
        video_id: VideoId::new(id),
        title: title.into(),
        artists: vec![ArtistRef { name: "Artist".into(), id: None }],
        album: None,
        duration: Some(Duration::from_secs(secs)),
    }
}

fn found(id: &str, title: &str, secs: u64, explicit: bool) -> ListedTrack {
    ListedTrack {
        track: song(id, title, secs),
        occurrence: None,
        thumbnail: None,
        explicit,
        liked: None,
    }
}

/// Knows the explicit version of songs `a` and `b` (as `a2`, `b2`) and nothing of `c`.
#[derive(Default)]
struct Script {
    asked: Mutex<Vec<String>>,
}

impl SearchSource for Script {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }

    fn search_filtered(&self, query: &str, _: SearchFilter) -> Result<SearchPage, LoadFailure> {
        self.asked.lock().unwrap().push(query.to_owned());
        let mut answers: HashMap<&str, Vec<ListedTrack>> = HashMap::new();
        answers.insert("Song a Artist", vec![found("a2", "Song a", 201, true)]);
        answers.insert("Song b Artist", vec![found("b2", "Song b", 199, true)]);
        answers.insert("Song d Artist", vec![found("d2", "Song d", 200, true)]);
        Ok(SearchPage {
            results: SearchResults {
                songs: answers.remove(query).unwrap_or_default(),
                ..SearchResults::default()
            },
            continuation: None,
        })
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
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
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: ["queueAAAA01", "queueBBBB02", "queueCCCC03"]
            .iter()
            .map(|id| LibraryTrack { track: song(id, id, 200) })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(Script::default()),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let crate_model = shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| {
        for id in ["a", "b", "c"] {
            m.add(song(id, &format!("Song {id}"), 200), None, cx);
        }
    });
    vcx.run_until_parked();
    (Page { player, shell, crate_model }, vcx)
}

fn ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    })
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

/// Open `subject`'s menu and choose `command`.
fn choose(page: &Page, vcx: &mut VisualTestContext, subject: Subject, command: Command) {
    page.shell.update_in(vcx, |_, window, cx| {
        apollo_app::ui::track_menu::open(
            subject,
            Anchor::Point(point(px(300.), px(300.))),
            window,
            cx,
        );
    });
    vcx.run_until_parked();
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    let commands = menu.read_with(vcx, |m, cx| m.commands(cx));
    let at = commands
        .iter()
        .position(|c| *c == command)
        .unwrap_or_else(|| panic!("{command:?} is in the menu: {commands:?}"));
    menu.update_in(vcx, |m, window, cx| m.choose(at, window, cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn the_header_menu_swaps_the_whole_crate_and_the_pill_says_how_it_went(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    choose(&page, vcx, Subject::Crate(Vec::new()), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c"], "in place; c has no explicit version");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Swapped 2 of 3"));
}

#[gpui::test]
fn the_header_menu_offers_both_directions(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |_, window, cx| {
        apollo_app::ui::track_menu::open(
            Subject::Crate(Vec::new()),
            Anchor::Point(point(px(300.), px(300.))),
            window,
            cx,
        );
    });
    vcx.run_until_parked();
    let labels = page
        .shell
        .read_with(vcx, |s, cx| s.menu().read(cx).labels(cx));
    assert_eq!(
        labels,
        [
            (Command::SwapToExplicit, "Swap to explicit versions"),
            (Command::SwapToClean, "Swap to clean versions"),
        ]
    );
}

#[gpui::test]
fn picked_songs_are_the_only_ones_swapped_and_stay_picked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let at = |vcx: &mut VisualTestContext, id: &'static str| vcx.debug_bounds(id).unwrap().center();
    let first = at(vcx, "crate-row-0");
    vcx.simulate_click(first, Modifiers::control());
    let second = at(vcx, "crate-row-1");
    vcx.simulate_click(second, Modifiers::control());
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    assert_eq!(crate_page.read_with(vcx, |p, _| p.picked()), 2);

    // c is not picked: it is not even looked up.
    let picked = vec!["b".to_owned(), "c".to_owned()];
    choose(&page, vcx, Subject::Crate(picked), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a", "b2", "c"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Swapped 1 of 2"));

    // The picks follow their songs to their new ids.
    choose(&page, vcx, Subject::Crate(vec!["a".into()]), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c"]);
    assert_eq!(crate_page.read_with(vcx, |p, _| p.picked()), 2, "both rows are still picked");
}

#[gpui::test]
fn a_row_offers_the_swap_only_while_it_is_not_explicit(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let open = |vcx: &mut VisualTestContext, id: &str| {
        let track = song(id, &format!("Song {id}"), 200);
        page.shell.update_in(vcx, |_, window, cx| {
            apollo_app::ui::track_menu::open(
                Subject::track(track, None),
                Anchor::Point(point(px(300.), px(300.))),
                window,
                cx,
            );
        });
        vcx.run_until_parked();
        page.shell.read_with(vcx, |s, cx| s.menu().read(cx).labels(cx))
    };
    assert!(open(vcx, "a").contains(&(Command::SwapToExplicit, "Swap to explicit version")));
    // A song that is not in the Crate has nothing to swap.
    assert!(!open(vcx, "elsewhere").iter().any(|(c, _)| *c == Command::SwapToExplicit));

    choose(&page, vcx, Subject::track(song("a", "Song a", 200), None), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a2", "b", "c"]);
    // Now it is explicit: the item is gone from its menu.
    assert!(!open(vcx, "a2").iter().any(|(c, _)| *c == Command::SwapToExplicit));
}

#[gpui::test]
fn the_crate_rows_show_the_e_of_an_explicit_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("crate-explicit-0").is_none());
    choose(&page, vcx, Subject::Crate(Vec::new()), Command::SwapToExplicit);
    assert!(vcx.debug_bounds("crate-explicit-0").is_some());
    assert!(vcx.debug_bounds("crate-explicit-1").is_some());
    assert!(vcx.debug_bounds("crate-explicit-2").is_none(), "c is still clean");
}

#[gpui::test]
fn one_ctrl_z_takes_back_the_whole_batch_in_line_with_the_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // A queue edit, then a swap, then another queue edit: Ctrl+Z goes back through them newest
    // first, and the swap is one step.
    page.player.update(vcx, |m, cx| m.clear_upcoming(cx));
    choose(&page, vcx, Subject::Crate(Vec::new()), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c"]);
    page.player.update(vcx, |m, cx| m.add_to_queue(song("extraTrack1", "Extra", 100), cx));
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c"], "the newer queue edit goes first");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), ["a", "b", "c"], "both swaps, one step");
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: swapped 2 songs to explicit versions")
    );

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"), "then the older queue edit");
    assert_eq!(ids(&page, vcx), ["a", "b", "c"]);
}

#[gpui::test]
fn prefer_explicit_follows_its_setting(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |m, cx| m.set(&PREFER_EXPLICIT, true, cx));
    vcx.run_until_parked();
    page.crate_model.update(vcx, |m, cx| {
        m.add(song("a", "Song a", 200), None, cx);
        m.clear(cx);
        m.add(song("b", "Song b", 200), None, cx);
    });
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["b2"], "swapped as it was added");
    settings.update(vcx, |m, cx| m.set(&PREFER_EXPLICIT, false, cx));
    vcx.run_until_parked();
    page.crate_model.update(vcx, |m, cx| {
        m.add(song("a", "Song a", 200), None, cx);
    });
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), ["b2", "a"], "and left alone when it is off");
}

#[gpui::test]
fn a_playlist_edit_made_after_a_swap_is_taken_back_before_it(cx: &mut TestAppContext) {
    use apollo_app::library::undo::{Action, PlaylistRef};
    let (page, vcx) = setup(cx);
    // swap 1, then a playlist edit, then swap 2: Ctrl+Z goes back through them newest first,
    // whichever history each is in.
    choose(&page, vcx, Subject::Crate(Vec::new()), Command::SwapToExplicit);
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    writes.update(vcx, |w, cx| {
        w.record_playlist_step(
            PlaylistRef { id: "PLgym".into(), title: "Gym".into() },
            Action::Reordered { order: vec!["T1".into()] },
            cx,
        );
    });
    page.crate_model.update(vcx, |m, cx| {
        m.add(song("d", "Song d", 200), None, cx);
    });
    choose(&page, vcx, Subject::Crate(vec!["d".into()]), Command::SwapToExplicit);
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c", "d2"]);

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c", "d"], "swap 2 first");
    assert!(writes.read_with(vcx, |w, _| w.playlist_undo_seq().is_some()), "the playlist edit waits");

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), ["a2", "b2", "c", "d"], "then the playlist edit, not swap 1");
    assert!(writes.read_with(vcx, |w, _| w.playlist_undo_seq().is_none()), "it was taken");

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), ["a", "b", "c", "d"], "and last, swap 1");
}
