//! Ports and Windows primitives shared by the service adapters. Contains no
//! YouTube-, mpv- or GPUI-specific logic.

use std::sync::Arc;

use apollo_domain::playback::Event;

pub mod lease_store;
pub mod secret;
pub mod session;
#[cfg(windows)]
pub mod win;

pub use lease_store::{unix_now, LeaseLookup, LeaseStore};
pub use secret::SecretString;
pub use session::{SessionProvider, SessionSnapshot};

/// How adapters feed completions back to whoever owns the coordinator. Called from
/// adapter threads; implementations must be cheap and must not call into the UI
/// directly (typically: push to a channel the app drains).
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync + 'static>;

/// Lowercase hex of `n` random bytes, for unguessable private names.
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS randomness");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
