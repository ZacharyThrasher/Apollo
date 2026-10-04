//! libmpv adapter for the runtime's `MediaEngine` port (BUILD_SPEC section 11).
//!
//! The engine owns one libmpv instance on a dedicated thread. Commands arrive over a channel;
//! the same thread pumps libmpv events and reports `EngineEvent`s through an `EventSink`. It
//! never obtains credentials, never calls metadata endpoints and never decides what plays next.
//! It does not touch the UI: the sink is the only way out.

pub mod classify;
pub mod coalesce;
pub mod config;
pub mod device;
pub mod eq;
pub mod ramp;
#[cfg(windows)]
mod engine;

pub use config::{core_options, MediaConfig};
pub use ramp::GainRamp;
#[cfg(windows)]
pub use engine::{MediaError, MpvEngine};

/// The client API version of the libmpv the app loaded (`client API 2.3`), for the diagnostics
/// report. The API version is the one number libmpv reports without an engine instance.
#[cfg(windows)]
pub fn client_api_version() -> String {
    // SAFETY: takes no arguments and only returns a constant compiled into libmpv.
    let v = unsafe { libmpv2_sys::mpv_client_api_version() };
    format!("client API {}.{}", v >> 16, v & 0xffff)
}
