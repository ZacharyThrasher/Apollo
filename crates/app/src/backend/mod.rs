//! What the app plays against. Two backends sit behind the same runtime ports:
//!
//! * [`sim`]: simulated resolver and engine with fixture tracks. The default, so the app
//!   builds and runs with no account, no tools and no network.
//! * `real` (cargo feature `real-services`, Windows): the M2 account session, yt-dlp resolver
//!   and libmpv engine, with the account's Liked Music as a quick test queue.
//!
//! Preparation (possibly slow: account restore, listing, tool verification) runs on a background
//! thread, so the window is up immediately; it yields the ready ports and the starting queue.

use std::sync::Arc;

use apollo_domain::model::Track;
use apollo_domain::playback::Event;
use apollo_runtime::{MediaEngine, Resolver};
use apollo_services_common::EventSink;

use crate::account::AccountPorts;
use crate::art::ArtSource;
use crate::library::LibrarySource;
use crate::search::SearchSource;
use crate::url_thumbs::ThumbSource;

pub mod account;
#[cfg(all(windows, feature = "real-services"))]
pub mod real;
#[cfg(feature = "real-services")]
pub mod real_search;
pub mod sim;
mod sim_search;

/// A track to queue plus where its artwork lives (resolved by the backend's [`ArtSource`]).
#[derive(Clone, Debug)]
pub struct LibraryTrack {
    pub track: Track,
}

pub struct Ports {
    pub resolver: Box<dyn Resolver + Send>,
    pub engine: Box<dyn MediaEngine + Send>,
}

pub struct Prepared {
    /// Short label for diagnostics (never shown in the UI).
    pub label: &'static str,
    pub ports: Ports,
    pub queue: Vec<LibraryTrack>,
    pub art: Arc<dyn ArtSource>,
    pub search: Arc<dyn SearchSource>,
    /// Artwork for list rows (search results, later the library), by thumbnail URL.
    pub thumbs: Arc<dyn ThumbSource>,
    /// What the library screen lists and opens.
    pub library: Arc<dyn LibrarySource>,
}

/// Which backend to run, from the command line / build features.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Sim,
    /// The simulation, but nobody is signed in at launch (to see the sign-in screen).
    SimSignedOut,
    #[cfg(all(windows, feature = "real-services"))]
    Real(real::Options),
}

impl Mode {
    /// `--real [--source stored|firefox|login|file:<path>] [--tracks N]` selects the real
    /// services when the feature is compiled in; anything else runs the simulation
    /// (`--signed-out` starts it without an account).
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let wants_real = args.iter().any(|a| a == "--real");
        if !wants_real {
            return Ok(if args.iter().any(|a| a == "--signed-out") {
                Mode::SimSignedOut
            } else {
                Mode::Sim
            });
        }
        #[cfg(all(windows, feature = "real-services"))]
        {
            real::Options::from_args(args).map(Mode::Real)
        }
        #[cfg(not(all(windows, feature = "real-services")))]
        {
            Err("--real needs a Windows build with the `real-services` feature".into())
        }
    }

    /// The account service and the platform's sign-in source. Cheap: nothing is read or
    /// validated until [`Mode::start_account`].
    pub fn account_ports(&self) -> AccountPorts {
        match self {
            Mode::Sim | Mode::SimSignedOut => account::ports(),
            #[cfg(all(windows, feature = "real-services"))]
            Mode::Real(opts) => real::account_ports(opts),
        }
    }

    /// Restore the stored session (and, in the simulation, sign in as the demo listener).
    /// Blocking; runs off the UI thread.
    pub fn start_account(&self, ports: &AccountPorts) {
        match self {
            Mode::Sim => account::start(ports, true),
            Mode::SimSignedOut => account::start(ports, false),
            #[cfg(all(windows, feature = "real-services"))]
            Mode::Real(opts) => real::start_account(opts, ports),
        }
    }

    /// Build the backend. Adapters report completions through `sink`; the resolver reads the
    /// session from `account`.
    pub fn prepare(
        self,
        sink: EventSink,
        account: &apollo_account::service::AccountService,
    ) -> Result<Prepared, String> {
        match self {
            Mode::Sim | Mode::SimSignedOut => {
                let _ = account;
                Ok(sim::prepare(sink))
            }
            #[cfg(all(windows, feature = "real-services"))]
            Mode::Real(opts) => real::prepare(opts, sink, account),
        }
    }
}

/// Convenience for adapters and tests: a sink that forwards into a channel.
pub fn channel_sink() -> (EventSink, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    (sink, rx)
}
