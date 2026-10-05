//! Which songs the service has marked explicit, as far as the lists shown so far said. A song
//! row knows its "E"; the Crate learns of a song only by its video id (the key shortcuts, menus
//! and drags carry), so the lists note what they showed here and the Crate looks a song up when
//! it is added. A song no list has said anything about is simply not known to be explicit; the
//! Crate's swap then finds out by asking (see `crate_swap`).
//!
//! Process-wide and in memory only: it is a hint, never a record.

use std::collections::HashSet;
use std::sync::Mutex;

/// Marks kept at most; a browsing session never gets near it, and a full set starts over.
const LIMIT: usize = 20_000;

static MARKS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashSet<String>) -> R) -> R {
    let mut guard = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashSet::new))
}

/// A list showed `video_id` with (or without) the "E".
pub fn note(video_id: &str, explicit: bool) {
    with(|marks| {
        if explicit {
            if marks.len() >= LIMIT {
                marks.clear();
            }
            marks.insert(video_id.to_owned());
        } else {
            marks.remove(video_id);
        }
    });
}

/// Whether a list has shown `video_id` as explicit.
pub fn is_explicit(video_id: &str) -> bool {
    with(|marks| marks.contains(video_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_kept_until_a_list_shows_the_song_clean() {
        assert!(!is_explicit("marks-test-1"));
        note("marks-test-1", true);
        assert!(is_explicit("marks-test-1"));
        note("marks-test-1", false);
        assert!(!is_explicit("marks-test-1"));
    }
}
