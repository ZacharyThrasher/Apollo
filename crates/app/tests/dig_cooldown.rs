//! Dig from here while YouTube is asking Apollo to slow down. Its own test binary: the cooldown is
//! the process's one shared `Throttle`, which other tests must not see.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::{PlayerModel, DIG_BUSY};
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_services_common::Throttle;
use gpui::{AppContext, TestAppContext};

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

#[gpui::test]
fn during_the_cooldown_d_sends_nothing_and_says_so(cx: &mut TestAppContext) {
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
        queue: vec![LibraryTrack { track: Track::new("coolSeed0001", "Seed") }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();

    // YouTube said "too many requests".
    Throttle::global().report_throttled(None);
    vcx.simulate_keystrokes("d");
    vcx.run_until_parked();
    assert!(sim.radio_seeds().is_empty(), "nothing is sent during the cooldown");
    assert_eq!(shell.read_with(vcx, |s, _| s.pill_text()).as_deref(), Some(DIG_BUSY));
    assert!(player.read_with(vcx, |m, _| m.view().skim.is_none()));
    // Pressing again is just as quiet.
    vcx.simulate_keystrokes("d");
    vcx.run_until_parked();
    assert!(sim.radio_seeds().is_empty());
}
