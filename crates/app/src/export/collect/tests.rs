use std::collections::HashMap;
use std::sync::Mutex;

use apollo_domain::library::{Collection, CollectionKind, WriteFailure};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{ArtistRef, BrowseId, Track};
use apollo_provider::Hit;

use super::*;
use crate::art::DecodedArt;
use crate::library::sim::SimLibrary;
use crate::library::source::ArtistContent;

const INSTANT: Options = Options {
    pause: Duration::ZERO,
};

fn playlist(id: &str, title: &str) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: Some("By you".into()),
        thumbnail: None,
    }
}

fn album(id: &str, title: &str) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: Some("Artist \u{2022} 2013".into()),
        thumbnail: None,
    }
}

fn row(video: &str, occurrence: &str) -> TrackRow {
    let mut track = Track::new(video, format!("Song {video}"));
    track.artists = vec![ArtistRef {
        name: "Artist".into(),
        id: None,
    }];
    TrackRow {
        track,
        occurrence: Some(occurrence.into()),
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

fn page(rows: Vec<TrackRow>, next: Option<&str>) -> Page<TrackRow> {
    Page {
        items: rows,
        continuation: next.map(str::to_owned),
        skipped: 0,
    }
}

/// A library that answers from a script and records every call, reads and writes alike.
#[derive(Default)]
struct Scripted {
    collections: Vec<Collection>,
    /// First pages by collection id, continuations by token.
    first: HashMap<String, Vec<Result<Page<TrackRow>, LoadFailure>>>,
    more: HashMap<String, Vec<Result<Page<TrackRow>, LoadFailure>>>,
    artists: Option<Result<Vec<Hit>, LoadFailure>>,
    /// Account tokens handed out in order (the last one repeats).
    accounts: Mutex<Vec<Option<String>>>,
    calls: Mutex<Vec<String>>,
}

impl Scripted {
    fn log(&self, what: impl Into<String>) {
        self.calls.lock().unwrap().push(what.into());
    }

    fn take(
        map: &HashMap<String, Vec<Result<Page<TrackRow>, LoadFailure>>>,
        calls: &Mutex<Vec<String>>,
        key: &str,
    ) -> Result<Page<TrackRow>, LoadFailure> {
        let n = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.ends_with(key))
            .count();
        let script = map
            .get(key)
            .unwrap_or_else(|| panic!("no script for {key}"));
        script[(n - 1).min(script.len() - 1)].clone()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn writes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("WRITE"))
            .collect()
    }
}

impl LibrarySource for Scripted {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        self.log("collections");
        Ok(self.collections.clone())
    }
    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        self.log(format!("tracks:{}", collection.id.as_str()));
        Self::take(&self.first, &self.calls, collection.id.as_str())
    }
    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        self.log(format!("more:{token}"));
        Self::take(&self.more, &self.calls, token)
    }
    fn artist(&self, _: &BrowseId) -> Result<ArtistContent, LoadFailure> {
        self.log("artist");
        Err(LoadFailure::Other)
    }
    fn subscribed_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        self.log("subscribed_artists");
        self.artists.clone().unwrap_or(Ok(Vec::new()))
    }
    fn artwork(&self, _: &str) -> Option<DecodedArt> {
        None
    }
    fn account_token(&self) -> Option<String> {
        let mut tokens = self.accounts.lock().unwrap();
        if tokens.len() > 1 {
            tokens.remove(0)
        } else {
            tokens.first().cloned().flatten()
        }
    }

    // Every write the trait has: recorded, never wanted.
    fn set_liked(&self, v: &str, _: bool) -> Result<(), WriteFailure> {
        self.log(format!("WRITE set_liked {v}"));
        Err(WriteFailure::Unsupported)
    }
    fn set_subscribed(&self, c: &str, _: bool) -> Result<(), WriteFailure> {
        self.log(format!("WRITE set_subscribed {c}"));
        Err(WriteFailure::Unsupported)
    }
    fn add_to_playlist(&self, p: &str, _: &str) -> Result<(), WriteFailure> {
        self.log(format!("WRITE add_to_playlist {p}"));
        Err(WriteFailure::Unsupported)
    }
    fn create_playlist(&self, t: &str) -> Result<String, WriteFailure> {
        self.log(format!("WRITE create_playlist {t}"));
        Err(WriteFailure::Unsupported)
    }
    fn delete_playlist(&self, p: &str) -> Result<(), WriteFailure> {
        self.log(format!("WRITE delete_playlist {p}"));
        Err(WriteFailure::Unsupported)
    }
    fn rename_playlist(&self, p: &str, _: &str) -> Result<(), WriteFailure> {
        self.log(format!("WRITE rename_playlist {p}"));
        Err(WriteFailure::Unsupported)
    }
    fn remove_from_playlist(&self, p: &str, _: &str, _: &str) -> Result<(), WriteFailure> {
        self.log(format!("WRITE remove_from_playlist {p}"));
        Err(WriteFailure::Unsupported)
    }
    fn move_in_playlist(&self, p: &str, _: &str, _: Option<&str>) -> Result<(), WriteFailure> {
        self.log(format!("WRITE move_in_playlist {p}"));
        Err(WriteFailure::Unsupported)
    }
    fn set_saved(&self, s: &str, _: bool) -> Result<(), WriteFailure> {
        self.log(format!("WRITE set_saved {s}"));
        Err(WriteFailure::Unsupported)
    }
}

fn hit(id: &str, title: &str) -> Hit {
    Hit {
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: Some("1M subscribers".into()),
        thumbnail: None,
        explicit: false,
    }
}

fn library() -> Scripted {
    let mut s = Scripted {
        collections: vec![
            playlist("VLPLroad", "Road trip"),
            album("MPREb_1", "An Album"),
            playlist("VLLM", "Liked Music"),
            playlist("VLPLgym", "Gym"),
        ],
        ..Default::default()
    };
    s.first.insert(
        "VLLM".into(),
        vec![Ok(page(
            vec![row("liked000001", "o1"), row("liked000002", "o2")],
            None,
        ))],
    );
    // Road trip: three pages; the same song twice (two occurrences) and a row repeated by an
    // overlapping page.
    s.first.insert(
        "VLPLroad".into(),
        vec![Ok(page(
            vec![row("road0000001", "a1"), row("road0000001", "a2")],
            Some("t1"),
        ))],
    );
    s.more.insert(
        "t1".into(),
        vec![Ok(page(
            vec![row("road0000001", "a2"), row("road0000002", "a3")],
            Some("t2"),
        ))],
    );
    s.more.insert(
        "t2".into(),
        vec![Ok(page(vec![row("road0000003", "a4")], None))],
    );
    s.first
        .insert("VLPLgym".into(), vec![Ok(page(Vec::new(), None))]);
    s.artists = Some(Ok(vec![
        hit("UC1", "First Artist"),
        hit("UC2", "Second Artist"),
    ]));
    s
}

#[test]
fn reads_everything_in_the_services_order_and_files_it_by_kind() {
    let source = library();
    let export = collect(&source, 1_759_565_553, INSTANT).unwrap();

    assert!(export.is_complete());
    assert_eq!(
        export
            .liked_songs
            .iter()
            .map(|t| t.video_id.as_str())
            .collect::<Vec<_>>(),
        ["liked000001", "liked000002"]
    );
    // Liked Music is its own section, not a playlist.
    assert_eq!(
        export
            .playlists
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["Road trip", "Gym"]
    );
    let road = &export.playlists[0];
    assert_eq!(road.id, "PLroad");
    // Every page followed; the song in two places stays twice; the row two pages carried once.
    assert_eq!(
        road.tracks
            .iter()
            .map(|t| t.video_id.as_str())
            .collect::<Vec<_>>(),
        ["road0000001", "road0000001", "road0000002", "road0000003"]
    );
    assert!(
        export.playlists[1].tracks.is_empty(),
        "an empty playlist is still listed"
    );
    assert_eq!(export.saved_albums.len(), 1);
    assert_eq!(export.saved_albums[0].title, "An Album");
    assert_eq!(
        export
            .subscribed_artists
            .iter()
            .map(|a| a.title.as_str())
            .collect::<Vec<_>>(),
        ["First Artist", "Second Artist"]
    );
}

#[test]
fn it_only_reads() {
    let source = library();
    collect(&source, 0, INSTANT).unwrap();
    assert!(
        source.writes().is_empty(),
        "no write was called: {:?}",
        source.writes()
    );
    let calls = source.calls();
    assert!(
        calls.iter().all(|c| c == "collections"
            || c == "subscribed_artists"
            || c.starts_with("tracks:")
            || c.starts_with("more:")),
        "{calls:?}"
    );
}

#[test]
fn the_simulated_library_exports_without_touching_the_account() {
    let sim = SimLibrary::instant();
    let artist = crate::library::sim::library_artist_hits()[0]
        .id
        .as_str()
        .to_owned();
    sim.preset_subscription(&artist, true);
    let before = (sim.liked_ids(), sim.owned_ids());
    let export = collect(&sim, 0, INSTANT).unwrap();
    // The simulation has one playlist that is made never to load; that is the only thing missing.
    assert_eq!(export.problems.len(), 1, "{:?}", export.problems);
    assert_eq!(export.problems[0].what, "Playlist \"Won't Load\"");
    assert!(!export.liked_songs.is_empty());
    assert_eq!(export.subscribed_artists.len(), 1);
    assert!(sim.write_log().is_empty(), "{:?}", sim.write_log());
    assert_eq!(before, (sim.liked_ids(), sim.owned_ids()));
}

#[test]
fn a_network_blip_is_retried_once() {
    let mut source = library();
    source.first.insert(
        "VLPLgym".into(),
        vec![
            Err(LoadFailure::Network),
            Ok(page(vec![row("gym00000001", "g1")], None)),
        ],
    );
    let export = collect(&source, 0, INSTANT).unwrap();
    assert!(export.is_complete());
    assert_eq!(export.playlists[1].tracks.len(), 1);
    assert_eq!(
        source
            .calls()
            .iter()
            .filter(|c| *c == "tracks:VLPLgym")
            .count(),
        2
    );
}

#[test]
fn a_list_that_keeps_failing_is_reported_and_the_rest_is_still_written() {
    let mut source = library();
    source
        .first
        .insert("VLPLgym".into(), vec![Err(LoadFailure::Throttled)]);
    let export = collect(&source, 0, INSTANT).unwrap();
    assert_eq!(export.problems.len(), 1);
    assert_eq!(export.problems[0].what, "Playlist \"Gym\"");
    assert_eq!(export.problems[0].reason, "Too many requests");
    assert_eq!(
        export.playlists[0].tracks.len(),
        4,
        "the other playlists are whole"
    );
    assert_eq!(
        export.problem_text().as_deref(),
        Some("Saved without 1 item")
    );
    // Retried once, not forever.
    assert_eq!(
        source
            .calls()
            .iter()
            .filter(|c| *c == "tracks:VLPLgym")
            .count(),
        2
    );
}

#[test]
fn a_failure_halfway_keeps_what_was_read_and_says_so() {
    let mut source = library();
    source
        .more
        .insert("t2".into(), vec![Err(LoadFailure::Parse)]);
    let export = collect(&source, 0, INSTANT).unwrap();
    assert_eq!(
        export.playlists[0].tracks.len(),
        3,
        "pages 1 and 2 are kept"
    );
    assert_eq!(export.problems.len(), 1);
}

#[test]
fn a_repeated_continuation_stops_instead_of_looping() {
    let mut source = library();
    source.more.insert(
        "t1".into(),
        vec![Ok(page(vec![row("road0000009", "z1")], Some("t1")))],
    );
    let export = collect(&source, 0, INSTANT).unwrap();
    assert_eq!(export.problems.len(), 1);
    assert_eq!(source.calls().iter().filter(|c| *c == "more:t1").count(), 1);
}

#[test]
fn a_dead_session_stops_everything_and_is_not_retried() {
    let mut source = library();
    source
        .first
        .insert("VLPLroad".into(), vec![Err(LoadFailure::Session)]);
    assert_eq!(
        collect(&source, 0, INSTANT),
        Err(ExportError::Failed(LoadFailure::Session))
    );
    assert_eq!(
        ExportError::Failed(LoadFailure::Session).text(),
        "Sign-in expired"
    );
    // Liked Music comes first and is read; nothing after the failing list is asked.
    assert!(!source
        .calls()
        .iter()
        .any(|c| c == "subscribed_artists" || c == "tracks:VLPLgym"));
    assert_eq!(
        source
            .calls()
            .iter()
            .filter(|c| *c == "tracks:VLPLroad")
            .count(),
        1
    );
}

#[test]
fn subscribed_artists_failing_is_a_problem_not_a_failure() {
    let mut source = library();
    source.artists = Some(Err(LoadFailure::Other));
    let export = collect(&source, 0, INSTANT).unwrap();
    assert!(export.subscribed_artists.is_empty());
    assert_eq!(export.problems[0].what, "Subscribed artists");
}

#[test]
fn a_switch_of_account_midway_writes_nothing() {
    let source = library();
    *source.accounts.lock().unwrap() = vec![Some("a".into()), Some("a".into()), Some("b".into())];
    assert_eq!(
        collect(&source, 0, INSTANT),
        Err(ExportError::AccountChanged)
    );
    assert_eq!(ExportError::AccountChanged.text(), "Account changed");
}

#[test]
fn a_library_without_liked_music_in_its_list_still_exports_it() {
    let mut source = library();
    source.collections.retain(|c| !c.is_liked_music());
    let export = collect(&source, 0, INSTANT).unwrap();
    assert_eq!(export.liked_songs.len(), 2);
}
