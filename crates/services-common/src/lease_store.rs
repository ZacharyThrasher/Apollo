//! Memory-only table of validated stream leases, shared by the resolver (producer)
//! and the media engine (consumer). The coordinator only ever sees opaque
//! [`LeaseId`]s. Nothing here is persisted; `clear` implements the lease half of
//! logout (SEC-03).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_domain::lease::StreamLease;
use apollo_domain::playback::LeaseId;

struct Held {
    lease: Arc<StreamLease>,
    created: Instant,
}

#[derive(Default)]
struct Inner {
    next: u64,
    leases: HashMap<u64, Held>,
}

#[derive(Clone, Default)]
pub struct LeaseStore {
    inner: Arc<Mutex<Inner>>,
}

/// Why a lease could not be handed to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseLookup {
    Missing,
    Stale,
}

impl LeaseStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, lease: StreamLease) -> LeaseId {
        let mut g = self.inner.lock().unwrap();
        g.next += 1;
        let id = g.next;
        g.leases.insert(
            id,
            Held {
                lease: Arc::new(lease),
                created: Instant::now(),
            },
        );
        LeaseId(id)
    }

    /// The lease if it exists and is still fresh (RES-04: reported expiry minus a margin;
    /// unknown expiry is only trusted for `max_unknown_age`).
    pub fn get_fresh(
        &self,
        id: LeaseId,
        now_unix: u64,
        margin: Duration,
        max_unknown_age: Duration,
    ) -> Result<Arc<StreamLease>, LeaseLookup> {
        let g = self.inner.lock().unwrap();
        let held = g.leases.get(&id.0).ok_or(LeaseLookup::Missing)?;
        if held
            .lease
            .is_fresh(now_unix, margin, held.created.elapsed(), max_unknown_age)
        {
            Ok(held.lease.clone())
        } else {
            Err(LeaseLookup::Stale)
        }
    }

    pub fn remove(&self, id: LeaseId) {
        self.inner.lock().unwrap().leases.remove(&id.0);
    }

    /// Drop every lease (logout, identity change).
    pub fn clear(&self) {
        self.inner.lock().unwrap().leases.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().leases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
