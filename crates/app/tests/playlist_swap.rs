//! Swap to explicit (and clean) versions for a playlist of your own: the More menu, the tip pill
//! that counts along and then says how many landed, each song keeping its place, two requests for
//! the whole playlist, one Ctrl+Z, and a pause at the first "too many requests". The real shell
//! and views on GPUI's test platform, a scripted search, the simulated library as the "service".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::SimLibrary;
use apollo_app::library::source::TrackRow;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Anchor, Command, Subject};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::Collection;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{ArtistRef, Track, VideoId};
use apollo_provider::{ListedTrack, SearchFilter, SearchPage, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{point, px, AppContext, Entity, TestAppContext, VisualTestContext};

const OWN: &str = "Nothing Yet";
const OWN_ID: &str = "PLsimempty";

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

fn row(id: &str, title: &str, secs: u64, explicit: bool) -> TrackRow {
    TrackRow {
        track: song(id, title, secs),
        occurrence: None,
        thumbnail: None,
        explicit,
        liked: None,
    }
}

fn listed(id: &str, title: &str, secs: u64, explicit: bool) -> ListedTrack {
    ListedTrack {
        track: song(id, title, secs),
        occurrence: None,
        thumbnail: None,
        explicit,
        liked: None,
    }
}

/// Knows the explicit versions of songs `a`, `b` and `d` (as `a2`, `b2`, `d2`), and none of `c`.
#[derive(Default)]
struct Script {
    asked: Mutex<Vec<String>>,
}

impl SearchSource for Script {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }

    fn search_filtered(&self, query: &str, filter: SearchFilter) -> Result<SearchPage, LoadFailure> {
        assert_eq!(filter, SearchFilter::Songs, "a swap asks for songs only");
        self.asked.lock().unwrap().push(query.to_owned());
        let mut answers: HashMap<&str, Vec<ListedTrack>> = HashMap::new();
        answers.insert("Song a Artist", vec![listed("a2", "Song a", 201, true)]);
        answers.insert("Song b Artist", vec![listed("b2", "Song b", 199, true)]);
        answers.insert("Song d Artist", vec![listed("d2", "Song d", 200, true)]);
        Ok(SearchPage {
            results: SearchResults {
                songs: answers.remove(query).unwrap_or_default(),
                ..SearchResults::default()
            },
            continuation: None,
        })
    }
}

/// A service that has had enough.
struct Busy;

impl SearchSource for Busy {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Err(LoadFailure::Throttled)
    }

    fn search_filtered(&self, _: &str, _: SearchFilter) -> Result<SearchPage, LoadFailure> {
        Err(LoadFailure::Throttled)
    }
}

struct Page {
    shell: Entity<Shell>,
    view: Entity<LibraryView>,
    sim: SimLibrary,
    script: Arc<Script>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let sim = SimLibrary::instant();
    // "Nothing Yet" holds four clean songs a, b, c, d; the explicit versions are known.
    sim.preset_playlist_rows(
        OWN_ID,
        &[
            row("a", "Song a", 200, false),
            row("b", "Song b", 200, false),
            row("c", "Song c", 200, false),
            row("d", "Song d", 200, false),
        ],
    );
    sim.teach(&[
        row("a2", "Song a", 201, true),
        row("b2", "Song b", 199, true),
        row("d2", "Song d", 200, true),
    ]);
    let script = Arc::new(Script::default());
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
        queue: ["queueAAAA01", "queueBBBB02"]
            .iter()
            .map(|id| LibraryTrack { track: song(id, id, 200) })
            .collect(),
        art: Arc::new(NoArt),
        search: script.clone(),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    // No wait between searches in a test: the executor's clock does not run by itself.
    vcx.update(|_, cx| {
        let swap = apollo_app::playlist_swap::handle(cx).expect("the shell made the swap");
        swap.update(cx, |m, _| m.set_pacing(Duration::ZERO, Duration::from_secs(60)));
    });
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    (Page { shell, view, sim, script }, vcx)
}

fn collection(page: &Page, vcx: &mut VisualTestContext, title: &str) -> Collection {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(Tab::Playlists)
            .iter()
            .find(|c| c.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("{title} is in the library"))
    })
}

fn open_playlist(page: &Page, vcx: &mut VisualTestContext, title: &str) -> Collection {
    let c = collection(page, vcx, title);
    page.view.update_in(vcx, |v, window, cx| v.open(c.clone(), window, cx));
    vcx.run_until_parked();
    c
}

/// The video ids of the rows the open page shows.
fn shown(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| d.listing.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect())
            .unwrap_or_default()
    })
}

/// The video ids of the playlist as the service holds it.
fn held(page: &Page) -> Vec<String> {
    use apollo_app::library::LibrarySource;
    let collection = Collection {
        kind: apollo_domain::library::CollectionKind::Playlist,
        id: apollo_domain::model::BrowseId::new(format!("VL{OWN_ID}")),
        title: String::new(),
        subtitle: None,
        thumbnail: None,
    };
    page.sim
        .tracks(&collection)
        .expect("the playlist reads")
        .items
        .into_iter()
        .map(|r| r.track.video_id.as_str().to_owned())
        .collect()
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn writes_named(page: &Page, prefix: &str) -> usize {
    page.sim.write_log().iter().filter(|(what, _)| what.starts_with(prefix)).count()
}

fn menu_commands(page: &Page, vcx: &mut VisualTestContext, subject: Subject) -> Vec<Command> {
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
    menu.read_with(vcx, |m, cx| m.commands(cx))
}

/// Open `subject`'s menu and choose `command`.
fn choose(page: &Page, vcx: &mut VisualTestContext, subject: Subject, command: Command) {
    let commands = menu_commands(page, vcx, subject);
    let at = commands
        .iter()
        .position(|c| *c == command)
        .unwrap_or_else(|| panic!("{command:?} is in the menu: {commands:?}"));
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    menu.update_in(vcx, |m, window, cx| m.choose(at, window, cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn the_menu_swaps_every_song_in_place_and_the_pill_says_how_many_landed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = open_playlist(&page, vcx, OWN);
    assert_eq!(shown(&page, vcx), ["a", "b", "c", "d"]);

    choose(&page, vcx, Subject::Collection(list), Command::SwapToExplicit);

    assert_eq!(shown(&page, vcx), ["a2", "b2", "c", "d2"], "same places; c has no explicit version");
    assert_eq!(held(&page), ["a2", "b2", "c", "d2"], "and the service holds the same");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Swapped 3 of 4 to explicit"));
    // One search per song, then two requests in all for the playlist.
    assert_eq!(page.script.asked.lock().unwrap().len(), 4);
    assert_eq!(writes_named(&page, "swap-rows"), 1, "the swap is one request");
    assert_eq!(writes_named(&page, "move-many"), 1, "putting them in place is one more");
    assert_eq!(writes_named(&page, "move:"), 0);
    assert_eq!(writes_named(&page, "remove"), 0);
}

#[gpui::test]
fn the_page_reads_the_service_again_so_swapped_rows_can_be_edited(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = open_playlist(&page, vcx, OWN);
    choose(&page, vcx, Subject::Collection(list), Command::SwapToExplicit);
    let tokens = page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| d.listing.items().iter().filter(|r| r.occurrence.is_none()).count())
    });
    assert_eq!(tokens, Some(0), "every row has the service's own token again");
}

#[gpui::test]
fn one_ctrl_z_puts_every_song_back_in_its_place(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = open_playlist(&page, vcx, OWN);
    choose(&page, vcx, Subject::Collection(list), Command::SwapToExplicit);
    assert_eq!(held(&page), ["a2", "b2", "c", "d2"]);

    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
    assert_eq!(held(&page), ["a", "b", "c", "d"]);
    assert_eq!(shown(&page, vcx), ["a", "b", "c", "d"]);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: swapped 3 songs for explicit versions in Nothing Yet")
    );
}

#[gpui::test]
fn the_first_too_many_requests_pauses_the_swap_and_changes_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.install_search(Arc::new(Busy), Arc::new(NoThumbs), cx));
    let list = open_playlist(&page, vcx, OWN);
    choose(&page, vcx, Subject::Collection(list.clone()), Command::SwapToExplicit);
    assert_eq!(held(&page), ["a", "b", "c", "d"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Paused: YouTube is busy"));
    assert_eq!(writes_named(&page, "swap-rows"), 0, "nothing is written without a match");
    // Asking again while it is busy says so without another search.
    choose(&page, vcx, Subject::Collection(list), Command::SwapToExplicit);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Paused: YouTube is busy"));
}

#[gpui::test]
fn a_playlist_with_nothing_to_swap_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.preset_playlist_rows(OWN_ID, &[row("a2", "Song a", 201, true)]);
    let list = open_playlist(&page, vcx, OWN);
    choose(&page, vcx, Subject::Collection(list.clone()), Command::SwapToExplicit);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Nothing to swap"));
    assert!(page.script.asked.lock().unwrap().is_empty(), "no search");
    // The clean direction looks at the explicit song instead.
    choose(&page, vcx, Subject::Collection(list), Command::SwapToClean);
    assert_eq!(pill(&page, vcx).as_deref(), Some("No clean versions found"));
}

#[gpui::test]
fn only_the_open_page_of_a_playlist_of_your_own_offers_the_swap(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = collection(&page, vcx, OWN);
    // Its card, with the page not open: no swap.
    let card = menu_commands(&page, vcx, Subject::Collection(list.clone()));
    assert!(!card.contains(&Command::SwapToExplicit), "{card:?}");
    open_playlist(&page, vcx, OWN);
    let on_page = menu_commands(&page, vcx, Subject::Collection(list));
    assert!(on_page.contains(&Command::SwapToExplicit), "{on_page:?}");
    assert!(on_page.contains(&Command::SwapToClean), "{on_page:?}");
    // Liked Music is not a playlist of your own.
    let liked = page.view.read_with(vcx, |v, cx| {
        v.model().read(cx).list(Tab::Playlists).iter().find(|c| c.is_liked_music()).cloned()
    });
    if let Some(liked) = liked {
        let commands = menu_commands(&page, vcx, Subject::Collection(liked));
        assert!(!commands.contains(&Command::SwapToExplicit), "{commands:?}");
    }
}
