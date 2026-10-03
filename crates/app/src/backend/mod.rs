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

use crate::art::ArtSource;

#[cfg(all(windows, feature = "real-services"))]
pub mod real;
pub mod sim;

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
}

/// Which backend to run, from the command line / build features.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Sim,
    #[cfg(all(windows, feature = "real-services"))]
    Real(real::Options),
}

impl Mode {
    /// `--real [--source stored|firefox|login|file:<path>] [--tracks N]` selects the real
    /// services when the feature is compiled in; anything else runs the simulation.
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let wants_real = args.iter().any(|a| a == "--real");
        if !wants_real {
            return Ok(Mode::Sim);
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

    /// Build the backend. Adapters report completions through `sink`.
    pub fn prepare(self, sink: EventSink) -> Result<Prepared, String> {
        match self {
            Mode::Sim => Ok(sim::prepare(sink)),
            #[cfg(all(windows, feature = "real-services"))]
            Mode::Real(opts) => real::prepare(opts, sink),
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
