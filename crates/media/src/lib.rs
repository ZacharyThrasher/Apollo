//! libmpv adapter for the runtime's `MediaEngine` port (BUILD_SPEC section 11).
//!
//! The engine owns one libmpv instance on a dedicated thread. Commands arrive over a channel;
//! the same thread pumps libmpv events and reports `EngineEvent`s through an `EventSink`. It
//! never obtains credentials, never calls metadata endpoints and never decides what plays next.
//! It does not touch the UI: the sink is the only way out.

pub mod classify;
pub mod coalesce;
pub mod config;
pub mod ramp;
#[cfg(windows)]
mod engine;

pub use config::{core_options, MediaConfig};
pub use ramp::GainRamp;
#[cfg(windows)]
pub use engine::{MediaError, MpvEngine};
