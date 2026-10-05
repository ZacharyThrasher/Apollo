use std::time::Duration;

use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_domain::playback::{Coordinator, Effect, Event, Intent, Phase};
use apollo_domain::queue::{AdvanceKind, Queue, Repeat};

use crate::{Migration, Store, StoreError, MIGRATIONS};

fn rich_track(id: &str) -> Track {
    Track {
        video_id: apollo_domain::model::VideoId::new(id),
        title: format!("Title {id} \u{65e5}\u{672c}\u{8a9e} \u{1f3b5}"),
        artists: vec![
            ArtistRef {
                name: "Artist One".into(),
                id: Some(BrowseId::new("UC1")),
            },
            ArtistRef {
                name: "Artist Two".into(),
                id: None,
            },
        ],
        album: Some(AlbumRef {
            title: "Album".into(),
            id: Some(BrowseId::new("MPREb_1")),
        }),
        duration: Some(Duration::from_millis(215_500)),
    }
}

fn busy_queue() -> Queue {
    let mut q = Queue::new();
    // Duplicate video IDs on purpose: occurrences must stay distinct.
    q.play_context(
        vec![
            rich_track("a"),
            rich_track("x"),
            rich_track("x"),
            Track::new("b", "bare"),
            rich_track("c"),
        ],
        1,
    );
    q.add_to_queue(vec![Track::new("m1", "manual one")]);
    q.append_radio(vec![Track::new("r1", "radio one")]);
    q.set_shuffle(true, u64::MAX - 7); // exercises the u64 -> i64 bit cast
    q.set_repeat(Repeat::All);
    q.advance(AdvanceKind::Explicit);
    q
}

#[test]
fn the_source_and_where_autoplay_began_round_trip() {
    use apollo_domain::queue::{QueueSource, SourceKind};
    let mut store = Store::open_in_memory().unwrap();
    let mut q = busy_queue();
    q.set_source(Some(
        QueueSource::new(SourceKind::Playlist, "Gym")
            .with_target("VLPL1")
            .with_thumbnail(Some("https://img/1".into())),
    ));
    q.append_autoplay(vec![Track::new("r9", "auto")], "seed");
    store.save_queue("acct", &q.snapshot(), 3).unwrap();
    let (back, _) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(back.snapshot(), q.snapshot());
    assert_eq!(back.source().unwrap().label(), "Gym");

    // No source saves and loads as no source.
    let plain = busy_queue();
    store.save_queue("acct", &plain.snapshot(), 3).unwrap();
    let (back, _) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(back.source(), None);
}

#[test]
fn a_source_kind_from_a_newer_build_is_dropped_not_fatal() {
    use apollo_domain::queue::{QueueSource, SourceKind};
    let mut store = Store::open_in_memory().unwrap();
    let mut q = busy_queue();
    q.set_source(Some(QueueSource::new(SourceKind::Album, "Blue")));
    store.save_queue("acct", &q.snapshot(), 0).unwrap();
    store
        .conn
        .execute("UPDATE queue_state SET source_kind = 'podcast'", [])
        .unwrap();
    let (back, _) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(back.source(), None);
}

#[test]
fn queue_round_trips_exactly() {
    let mut store = Store::open_in_memory().unwrap();
    let q = busy_queue();
    store.save_queue("acct-1", &q.snapshot(), 73_250).unwrap();
    let (restored, pos) = store.load_queue("acct-1").unwrap().expect("saved queue");
    assert_eq!(restored.snapshot(), q.snapshot());
    assert_eq!(pos, 73_250);
}

#[test]
fn empty_queue_round_trips_and_missing_queue_is_none() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.load_queue("acct").unwrap().is_none());
    let q = Queue::new();
    store.save_queue("acct", &q.snapshot(), 0).unwrap();
    let (restored, pos) = store.load_queue("acct").unwrap().unwrap();
    assert!(restored.current().is_none());
    assert_eq!(pos, 0);
}

#[test]
fn saving_again_replaces_rather_than_accumulates() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_queue("acct", &busy_queue().snapshot(), 1)
        .unwrap();
    let mut small = Queue::new();
    small.play_context(vec![Track::new("only", "Only")], 0);
    store.save_queue("acct", &small.snapshot(), 2).unwrap();
    let (restored, pos) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(restored.snapshot(), small.snapshot());
    assert_eq!(pos, 2);
}

// TEST-22: restore paused, with a fresh stream on resume.
#[test]
fn restart_restores_paused_and_resolves_a_fresh_stream() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_queue("acct", &busy_queue().snapshot(), 0)
        .unwrap();
    store.checkpoint_position(41_000).unwrap();

    let (queue, pos) = store.load_queue("acct").unwrap().unwrap();
    let mut c = Coordinator::restored(queue, pos);
    assert_eq!(c.phase(), Phase::Idle);
    assert_eq!(c.intent(), Intent::Paused);
    assert!(c.active_load().is_none(), "never auto-start on launch");
    let fx = c.handle(Event::Play);
    assert!(
        matches!(
            &fx[0],
            Effect::Resolve {
                resume_ms: 41_000,
                ..
            }
        ),
        "{fx:?}"
    );
}

// TEST-04/15: private state never crosses identities.
#[test]
fn a_different_account_purges_instead_of_replaying() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_queue("acct-1", &busy_queue().snapshot(), 5)
        .unwrap();
    assert!(store.load_queue("acct-2").unwrap().is_none());
    // The old account's queue is gone, not merely hidden.
    assert!(store.load_queue("acct-1").unwrap().is_none());
}

#[test]
fn logout_purge_removes_every_private_row() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_queue("acct", &busy_queue().snapshot(), 5)
        .unwrap();
    store.set_setting("volume", "0.4").unwrap();
    store.purge_private().unwrap();
    assert!(store.load_queue("acct").unwrap().is_none());
    for table in [
        "queue_state",
        "queue_entries",
        "entry_artists",
        "queue_canonical",
    ] {
        let n: i64 = store
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    // Non-private settings may be retained.
    assert_eq!(store.get_setting("volume").unwrap().as_deref(), Some("0.4"));
}

#[test]
fn a_failed_save_leaves_the_previous_queue_intact() {
    let mut store = Store::open_in_memory().unwrap();
    let good = busy_queue();
    store.save_queue("acct", &good.snapshot(), 9).unwrap();

    // Corrupt snapshot (duplicate entry ID) fails partway through the transaction.
    let mut bad = good.snapshot();
    let dup = bad.context[0].clone();
    bad.context.push(dup);
    assert!(store.save_queue("acct", &bad, 1).is_err());

    let (restored, pos) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(restored.snapshot(), good.snapshot());
    assert_eq!(pos, 9);
}

#[test]
fn corrupt_stored_data_is_reported_not_trusted() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_queue("acct", &busy_queue().snapshot(), 0)
        .unwrap();
    store
        .conn
        .execute("UPDATE queue_state SET next_id = 1", [])
        .unwrap();
    match store.load_queue("acct") {
        Err(StoreError::Corrupt(_)) => {}
        other => panic!("expected Corrupt, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn settings_round_trip() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.get_setting("volume").unwrap(), None);
    store.set_setting("volume", "0.25").unwrap();
    store.set_setting("volume", "0.5").unwrap();
    assert_eq!(store.get_setting("volume").unwrap().as_deref(), Some("0.5"));
}

#[test]
fn file_database_persists_across_reopen_and_sets_the_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apollo.sqlite");
    {
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            MIGRATIONS.last().unwrap().version
        );
        store
            .save_queue("acct", &busy_queue().snapshot(), 12)
            .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    let (q, pos) = store.load_queue("acct").unwrap().unwrap();
    assert_eq!(q.snapshot(), busy_queue().snapshot());
    assert_eq!(pos, 12);
}

#[test]
fn a_newer_schema_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apollo.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 99;")
            .unwrap();
    }
    match Store::open(&path) {
        Err(StoreError::NewerSchema {
            found: 99,
            supported,
        }) => {
            assert_eq!(supported, MIGRATIONS.last().unwrap().version)
        }
        other => panic!("expected NewerSchema, got {:?}", other.map(|_| ())),
    }
    // No backup, no migration side effects on the main table set.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 99);
    let has_queue: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'queue_state'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_queue, 0);
}

#[test]
fn a_failed_migration_rolls_back_and_a_backup_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apollo.sqlite");
    {
        let mut store = Store::open(&path).unwrap();
        store
            .save_queue("acct", &busy_queue().snapshot(), 7)
            .unwrap();
    }
    let v1 = MIGRATIONS.last().unwrap().version;
    let broken = [
        Migration {
            version: MIGRATIONS[0].version,
            sql: MIGRATIONS[0].sql,
        },
        Migration {
            version: v1 + 1,
            // The first statement succeeds, the second fails: the whole step must roll back.
            sql: "CREATE TABLE half_done (x INTEGER); INSERT INTO no_such_table VALUES (1);",
        },
    ];
    assert!(Store::open_with(&path, &broken).is_err());

    // The database is exactly as before: same version, no half-applied table, queue intact.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), v1);
    let half: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(half, 0);
    assert_eq!(store.load_queue("acct").unwrap().unwrap().1, 7);

    // A backup of the pre-migration database was written.
    let backup = dir.path().join(format!("apollo.sqlite.bak-v{v1}"));
    assert!(backup.exists());
    let mut from_backup = Store::open(&backup).unwrap();
    assert!(from_backup.load_queue("acct").unwrap().is_some());
}

// TEST-23 (DB part): the schema has nowhere to put secrets or signed URLs.
#[test]
fn schema_has_no_secret_bearing_columns() {
    let store = Store::open_in_memory().unwrap();
    let mut stmt = store
        .conn
        .prepare("SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p WHERE m.type = 'table'")
        .unwrap();
    let cols: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (table, col) in cols {
        let c = col.to_ascii_lowercase();
        for banned in [
            "url", "cookie", "token", "password", "secret", "header", "lease", "auth",
        ] {
            assert!(!c.contains(banned), "{table}.{col} looks secret-bearing");
        }
    }
}

#[test]
fn pages_are_kept_per_account_and_replaced_in_place() {
    let store = Store::open_in_memory().unwrap();
    store.put_page("a", "home", "{\"v\":1}", 100).unwrap();
    store.put_page("a", "home", "{\"v\":2}", 200).unwrap();
    store.put_page("b", "home", "other", 150).unwrap();
    let page = store.get_page("a", "home").unwrap().expect("stored");
    assert_eq!((page.body.as_str(), page.fetched_at), ("{\"v\":2}", 200));
    assert_eq!(store.pages_for("a").unwrap().len(), 1);
    assert!(store.get_page("a", "nope").unwrap().is_none());
    assert!(store.get_page("c", "home").unwrap().is_none());

    store.delete_page("a", "home").unwrap();
    assert!(store.get_page("a", "home").unwrap().is_none());
    assert!(store.get_page("b", "home").unwrap().is_some(), "other accounts are untouched");
}

#[test]
fn another_account_or_a_sign_out_purges_the_pages() {
    let store = Store::open_in_memory().unwrap();
    store.put_page("a", "home", "x", 1).unwrap();
    store.put_page("b", "home", "y", 1).unwrap();
    store.clear_pages_except("b").unwrap();
    assert!(store.pages_for("a").unwrap().is_empty());
    assert_eq!(store.pages_for("b").unwrap().len(), 1);
    store.clear_pages().unwrap();
    assert!(store.pages_for("b").unwrap().is_empty());
}

#[test]
fn the_oldest_pages_go_past_the_cap() {
    let store = Store::open_in_memory().unwrap();
    for n in 0..(crate::pages::MAX_PAGES_PER_ACCOUNT as i64 + 5) {
        store.put_page("a", &format!("p{n}"), "x", n).unwrap();
    }
    let kept = store.pages_for("a").unwrap();
    assert_eq!(kept.len(), crate::pages::MAX_PAGES_PER_ACCOUNT);
    assert!(store.get_page("a", "p0").unwrap().is_none(), "oldest dropped");
    assert!(store.get_page("a", "p164").unwrap().is_some(), "newest kept");
}

// ---- the Crate -------------------------------------------------------------------------------

fn crate_row(id: &str, added_ms: i64) -> crate::CrateRow {
    crate::CrateRow {
        track: rich_track(id),
        thumbnail: Some(format!("https://example.invalid/{id}.jpg")),
        explicit: false,
        added_ms,
    }
}

fn crate_ids(rows: &[crate::CrateRow]) -> Vec<&str> {
    rows.iter().map(|r| r.track.video_id.as_str()).collect()
}

#[test]
fn the_crate_round_trips_with_new_songs_at_the_bottom() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.crate_load("acct").unwrap().is_empty());
    store.crate_add("acct", &crate_row("a", 10)).unwrap();
    store.crate_add("acct", &crate_row("b", 20)).unwrap();
    // The same millisecond: the later insert still goes below.
    store.crate_add("acct", &crate_row("c", 20)).unwrap();
    let rows = store.crate_load("acct").unwrap();
    assert_eq!(crate_ids(&rows), ["a", "b", "c"]);
    assert_eq!(rows[0], crate_row("a", 10), "tracks come back whole");
}

#[test]
fn adding_a_song_twice_keeps_one_copy_where_it_was() {
    let mut store = Store::open_in_memory().unwrap();
    store.crate_add("acct", &crate_row("a", 10)).unwrap();
    store.crate_add("acct", &crate_row("b", 20)).unwrap();
    store.crate_add("acct", &crate_row("a", 30)).unwrap();
    let rows = store.crate_load("acct").unwrap();
    assert_eq!(crate_ids(&rows), ["a", "b"]);
    assert_eq!(rows[0].added_ms, 10);
}

#[test]
fn reordering_the_crate_is_kept_and_new_songs_still_go_below() {
    let mut store = Store::open_in_memory().unwrap();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        store.crate_add("acct", &crate_row(id, i as i64)).unwrap();
    }
    let order = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    store.crate_reorder("acct", &order(&["c", "a", "b"])).unwrap();
    assert_eq!(crate_ids(&store.crate_load("acct").unwrap()), ["c", "a", "b"]);
    store.crate_add("acct", &crate_row("d", 9)).unwrap();
    assert_eq!(
        crate_ids(&store.crate_load("acct").unwrap()),
        ["c", "a", "b", "d"]
    );
    // A removal leaves a gap that changes nothing; a song the new order forgot keeps its place
    // in the list rather than vanishing.
    store.crate_remove("acct", "a").unwrap();
    store.crate_reorder("acct", &order(&["b", "c"])).unwrap();
    assert_eq!(crate_ids(&store.crate_load("acct").unwrap()), ["b", "c", "d"]);
    // Another account is untouched.
    store.crate_add("other", &crate_row("z", 1)).unwrap();
    store.crate_reorder("acct", &order(&["z"])).unwrap();
    assert_eq!(crate_ids(&store.crate_load("other").unwrap()), ["z"]);
}

#[test]
fn migration_9_keeps_the_order_the_crate_page_showed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apollo.sqlite");
    {
        // The schema as it was before the crate could be arranged: newest first by time.
        let store = Store::open_with(&path, &MIGRATIONS[..8]).unwrap();
        assert_eq!(store.schema_version().unwrap(), 8);
        for (acct, id, ms) in [
            ("acct", "old", 10),
            ("acct", "new", 30),
            ("acct", "mid", 20),
            ("acct", "tie", 20),
            ("other", "x", 5),
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO crate_items (account_key, video_id, title, added_ms)
                     VALUES (?1, ?2, ?2, ?3)",
                    rusqlite::params![acct, id, ms],
                )
                .unwrap();
        }
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), MIGRATIONS.last().unwrap().version);
    // Newest on top, the later insert of a tie above the earlier, as the page showed.
    assert_eq!(
        crate_ids(&store.crate_load("acct").unwrap()),
        ["new", "tie", "mid", "old"]
    );
    store.crate_add("acct", &crate_row("fresh", 99)).unwrap();
    assert_eq!(
        crate_ids(&store.crate_load("acct").unwrap()).last(),
        Some(&"fresh")
    );
}

#[test]
fn a_swap_takes_the_place_of_the_song_it_replaces() {
    let mut store = Store::open_in_memory().unwrap();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        store.crate_add("acct", &crate_row(id, i as i64)).unwrap();
    }
    let mut explicit = crate_row("b2", 1);
    explicit.explicit = true;
    assert!(store.crate_swap("acct", "b", &explicit).unwrap());
    let rows = store.crate_load("acct").unwrap();
    assert_eq!(crate_ids(&rows), ["a", "b2", "c"], "same slot");
    assert_eq!(rows[1], explicit, "whole, flag included");
    // The old song's artists went with it, the new one's came in.
    let n: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM crate_artists WHERE video_id = 'b'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    // Nothing to replace, or a song the crate already holds elsewhere: nothing happens.
    assert!(!store.crate_swap("acct", "zzz", &crate_row("q", 1)).unwrap());
    assert!(!store.crate_swap("acct", "a", &crate_row("c", 1)).unwrap());
    assert_eq!(crate_ids(&store.crate_load("acct").unwrap()), ["a", "b2", "c"]);
    // Another account's crate is not reached.
    store.crate_add("other", &crate_row("a", 1)).unwrap();
    assert!(!store.crate_swap("other", "b2", &crate_row("x", 1)).unwrap());
    // A swap back restores the old song in the same slot.
    assert!(store.crate_swap("acct", "b2", &crate_row("b", 1)).unwrap());
    assert_eq!(crate_ids(&store.crate_load("acct").unwrap()), ["a", "b", "c"]);
}

#[test]
fn songs_added_before_migration_10_are_not_known_to_be_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apollo.sqlite");
    {
        let store = Store::open_with(&path, &MIGRATIONS[..9]).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO crate_items (account_key, video_id, title, added_ms)
                 VALUES ('acct', 'old', 'old', 1)",
                [],
            )
            .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    let rows = store.crate_load("acct").unwrap();
    assert_eq!(crate_ids(&rows), ["old"]);
    assert!(!rows[0].explicit);
    let mut explicit = crate_row("e", 2);
    explicit.explicit = true;
    store.crate_add("acct", &explicit).unwrap();
    assert!(store.crate_load("acct").unwrap()[1].explicit);
}

#[test]
fn removing_and_clearing_the_crate() {
    let mut store = Store::open_in_memory().unwrap();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        store.crate_add("acct", &crate_row(id, i as i64)).unwrap();
    }
    store.crate_remove("acct", "b").unwrap();
    assert_eq!(crate_ids(&store.crate_load("acct").unwrap()), ["a", "c"]);
    store.crate_clear("acct").unwrap();
    assert!(store.crate_load("acct").unwrap().is_empty());
    // The artists went with their songs.
    let n: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM crate_artists", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn the_crate_never_crosses_accounts() {
    let mut store = Store::open_in_memory().unwrap();
    store.crate_add("acct-1", &crate_row("a", 1)).unwrap();
    assert!(store.crate_load("acct-2").unwrap().is_empty());
    // The first account's crate is gone, not merely hidden.
    assert!(store.crate_load("acct-1").unwrap().is_empty());
}

#[test]
fn sign_out_purges_the_crate() {
    let mut store = Store::open_in_memory().unwrap();
    store.crate_add("acct", &crate_row("a", 1)).unwrap();
    store.crate_purge().unwrap();
    assert!(store.crate_load("acct").unwrap().is_empty());
}

fn blocked(id: Option<&str>, name: &str) -> apollo_domain::blocklist::BlockedArtist {
    apollo_domain::blocklist::BlockedArtist {
        id: id.map(Into::into),
        name: name.into(),
    }
}

#[test]
fn blocked_artists_round_trip_newest_first_and_once_each() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.blocked_load("acct").unwrap().is_empty());
    store.blocked_add("acct", &blocked(Some("UCa"), "A"), 10).unwrap();
    store.blocked_add("acct", &blocked(Some("UCb"), "B"), 20).unwrap();
    store.blocked_add("acct", &blocked(None, "Nameless"), 30).unwrap();
    // Blocking again changes nothing, not even the order.
    store.blocked_add("acct", &blocked(Some("UCa"), "A, renamed"), 40).unwrap();
    store.blocked_add("acct", &blocked(None, " nameless "), 50).unwrap();
    let rows = store.blocked_load("acct").unwrap();
    assert_eq!(
        rows,
        [blocked(None, "Nameless"), blocked(Some("UCb"), "B"), blocked(Some("UCa"), "A")]
    );
}

#[test]
fn unblocking_removes_only_that_artist() {
    let mut store = Store::open_in_memory().unwrap();
    store.blocked_add("acct", &blocked(Some("UCa"), "A"), 1).unwrap();
    store.blocked_add("acct", &blocked(Some("UCb"), "B"), 2).unwrap();
    store.blocked_remove("acct", &blocked(Some("UCa"), "whatever the name was")).unwrap();
    assert_eq!(store.blocked_load("acct").unwrap(), [blocked(Some("UCb"), "B")]);
}

#[test]
fn blocked_artists_never_cross_accounts_and_sign_out_forgets_them() {
    let mut store = Store::open_in_memory().unwrap();
    store.blocked_add("acct", &blocked(Some("UCa"), "A"), 1).unwrap();
    assert!(store.blocked_load("other").unwrap().is_empty());
    assert!(
        store.blocked_load("acct").unwrap().is_empty(),
        "reading for another account purged the first one's"
    );
    store.blocked_add("acct", &blocked(Some("UCa"), "A"), 1).unwrap();
    store.blocked_purge().unwrap();
    assert!(store.blocked_load("acct").unwrap().is_empty());
}
