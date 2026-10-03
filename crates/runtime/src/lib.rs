//! Executes the coordinator's effects against service ports.
//!
//! The ports ([`Resolver`], [`MediaEngine`]) are deliberately synchronous
//! "start work" interfaces: real adapters run the work on their own
//! threads/runtime and later feed completions back as [`apollo_domain::playback::Event`]s.
//! That keeps the driver deterministic, so the same tests run against the mocks
//! here and, later, against recorded real-adapter traces.

pub mod mock;
pub mod ports;

pub use ports::{Driver, MediaEngine, ResolveRequest, Resolver};

#[cfg(test)]
mod tests;
