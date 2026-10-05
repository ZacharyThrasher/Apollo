use super::*;
use crate::library::sim::{SimLibrary, WriteBehaviour, LONG_ID, LONG_LEN};

fn playlist(id: &str, title: &str) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(format!("VL{id}")),
        title: title.into(),
        subtitle: None,
        thumbnail: None,
    }
}

fn album(id: &str) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new(id),
        title: "Night Shift".into(),
        subtitle: None,
        thumbnail: None,
    }
}

fn known(sim: &SimLibrary) -> HashSet<String> {
    sim.collections()
        .unwrap()
        .iter()
        .map(|c| c.library_key().to_owned())
        .collect()
}

fn none() -> Option<String> {
    None
}

/// Rename the playlist with this bare id (an edit that carries only the title).
fn rename(
    sim: &SimLibrary,
    expect: &Option<String>,
    id: &str,
    title: &str,
) -> Result<(), WriteFailure> {
    let change = PlaylistEdit {
        title: Some(title.into()),
        ..PlaylistEdit::default()
    };
    edit(sim, expect, &playlist(id, "x"), &change)
}

fn titles(sim: &SimLibrary) -> Vec<String> {
    sim.collections().unwrap().into_iter().map(|c| c.title).collect()
}

// ---- create ----------------------------------------------------------------------------------

#[test]
fn a_created_playlist_is_in_the_library_right_after_liked_music() {
    let sim = SimLibrary::instant();
    let id = create(&sim, &none(), &known(&sim), "Apollo test").unwrap();
    assert!(is_user_playlist_id(&id));
    let list = sim.collections().unwrap();
    assert!(list[0].is_liked_music());
    assert_eq!((list[1].title.as_str(), list[1].library_key()), ("Apollo test", id.as_str()));
    assert!(sim.owned_ids().contains(&id), "and it is one the account can edit");
    assert!(sim.occurrences(&id).is_empty(), "created empty");
}

#[test]
fn a_create_whose_reply_is_lost_is_found_by_reading_back_and_is_never_sent_twice() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let id = create(&sim, &none(), &known(&sim), "Apollo test").unwrap();
    assert_eq!(
        titles(&sim).iter().filter(|t| *t == "Apollo test").count(),
        1,
        "one playlist, not two"
    );
    assert!(sim.collections().unwrap().iter().any(|c| c.library_key() == id));
    let creates = sim.write_log().iter().filter(|(w, _)| w == "create").count();
    assert_eq!(creates, 1, "the write was sent once");
}

#[test]
fn a_create_that_never_landed_is_a_failure_not_a_phantom_success() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    let result = create(&sim, &none(), &known(&sim), "Apollo test");
    assert_eq!(result, Err(WriteFailure::Unknown));
    assert!(!titles(&sim).contains(&"Apollo test".to_owned()));
    assert_eq!(sim.write_log().len(), 1, "and it was not sent again");
}

#[test]
fn a_playlist_that_was_already_there_is_not_mistaken_for_the_new_one() {
    let sim = SimLibrary::instant();
    // "Road Trip" already exists; the create never landed, so reading back must not claim it.
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    let result = create(&sim, &none(), &known(&sim), "Road Trip");
    assert_eq!(result, Err(WriteFailure::Unknown));
    // The same title that did land is found among the playlists that were not there before.
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let id = create(&sim, &none(), &known(&sim), "Road Trip").unwrap();
    assert_ne!(id, "PLsimroad");
}

#[test]
fn a_refused_create_is_a_plain_failure() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    assert_eq!(
        create(&sim, &none(), &known(&sim), "x"),
        Err(WriteFailure::Rejected)
    );
}

#[test]
fn an_unknown_create_with_no_read_back_is_reported_unknown() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    sim.set_collections_failure(Some(LoadFailure::Network));
    assert_eq!(
        create(&sim, &none(), &HashSet::new(), "x"),
        Err(WriteFailure::Unknown),
        "without a read-back nothing is believed"
    );
}

// ---- rename and delete -----------------------------------------------------------------------

#[test]
fn a_rename_changes_the_title_the_library_lists() {
    let sim = SimLibrary::instant();
    rename(&sim, &none(), "PLsimroad", "Highway").unwrap();
    assert!(titles(&sim).contains(&"Highway".to_owned()));
    assert!(!titles(&sim).contains(&"Road Trip".to_owned()));
}

#[test]
fn a_rename_with_a_lost_reply_is_confirmed_by_the_new_title_and_a_missed_one_is_not() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(rename(&sim, &none(), "PLsimroad", "Highway"), Ok(()));
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(
        rename(&sim, &none(), "PLsimlong", "Elsewhere"),
        Err(WriteFailure::Unknown)
    );
    assert!(titles(&sim).contains(&"Long Haul".to_owned()));
    assert_eq!(sim.write_log().len(), 2, "each sent once");
}

#[test]
fn a_delete_removes_the_playlist_and_a_lost_reply_is_confirmed_by_its_absence() {
    let sim = SimLibrary::instant();
    delete(&sim, &none(), "PLsimempty").unwrap();
    assert!(!sim.collections().unwrap().iter().any(|c| c.library_key() == "PLsimempty"));
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(delete(&sim, &none(), "PLsimroad"), Ok(()));
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(delete(&sim, &none(), "PLsimlong"), Err(WriteFailure::Unknown));
    assert!(sim.collections().unwrap().iter().any(|c| c.library_key() == "PLsimlong"));
}

#[test]
fn only_a_user_playlist_ever_reaches_the_service_as_an_edit() {
    let sim = SimLibrary::instant();
    for id in ["LM", "OLAK5uy_x", "RDCLAK5uy_x", "VLPLsimroad", "", "SE"] {
        assert_eq!(delete(&sim, &none(), id), Err(WriteFailure::Rejected), "{id}");
        assert_eq!(rename(&sim, &none(), id, "x"), Err(WriteFailure::Rejected), "{id}");
    }
    let liked = playlist("LM", "Liked Music");
    assert_eq!(remove(&sim, &none(), &liked, "vid", "occ"), Err(WriteFailure::Rejected));
    assert_eq!(
        move_row(&sim, &none(), &liked, "occ", None, &[]),
        Err(WriteFailure::Rejected)
    );
    assert_eq!(
        remove(&sim, &none(), &album("MPREbsimnight"), "vid", "occ"),
        Err(WriteFailure::Rejected)
    );
    assert!(sim.write_log().is_empty(), "nothing was sent: {:?}", sim.write_log());
}

// ---- rows ------------------------------------------------------------------------------------

fn road_trip() -> Collection {
    playlist("PLsimroad", "Road Trip")
}

#[test]
fn removing_a_row_removes_only_that_occurrence() {
    let sim = SimLibrary::instant();
    let rows = all_rows(&sim, &road_trip()).unwrap();
    assert_eq!(rows.len(), 3);
    let gone = &rows[1];
    remove(
        &sim,
        &none(),
        &road_trip(),
        gone.track.video_id.as_str(),
        gone.occurrence.as_deref().unwrap(),
    )
    .unwrap();
    let left = sim.occurrences("PLsimroad");
    assert_eq!(left.len(), 2);
    assert!(!left.contains(&gone.occurrence.clone().unwrap()));
}

#[test]
fn a_remove_with_a_lost_reply_is_confirmed_by_the_row_being_gone_and_a_missed_one_is_not() {
    let sim = SimLibrary::instant();
    let rows = all_rows(&sim, &road_trip()).unwrap();
    let (a, b) = (&rows[0], &rows[2]);
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(
        remove(&sim, &none(), &road_trip(), a.track.video_id.as_str(), a.occurrence.as_deref().unwrap()),
        Ok(())
    );
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(
        remove(&sim, &none(), &road_trip(), b.track.video_id.as_str(), b.occurrence.as_deref().unwrap()),
        Err(WriteFailure::Unknown)
    );
    assert_eq!(sim.occurrences("PLsimroad").len(), 2, "only the first went");
    assert_eq!(sim.write_log().len(), 2, "each sent once");
}

#[test]
fn moving_a_row_puts_it_before_the_one_named_or_at_the_end() {
    let sim = SimLibrary::instant();
    let original = sim.occurrences("PLsimroad");
    // The last row to the front.
    let desired = apollo_domain::library::moved(&original, 2, 0);
    move_row(&sim, &none(), &road_trip(), &original[2], Some(&original[0]), &desired).unwrap();
    assert_eq!(sim.occurrences("PLsimroad"), desired);
    // The new first row to the end.
    let now = sim.occurrences("PLsimroad");
    let desired = apollo_domain::library::moved(&now, 0, 3);
    move_row(&sim, &none(), &road_trip(), &now[0], None, &desired).unwrap();
    assert_eq!(sim.occurrences("PLsimroad"), desired);
}

#[test]
fn a_move_with_a_lost_reply_is_confirmed_only_by_the_exact_order() {
    let sim = SimLibrary::instant();
    let original = sim.occurrences("PLsimroad");
    let desired = apollo_domain::library::moved(&original, 2, 0);
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(
        move_row(&sim, &none(), &road_trip(), &original[2], Some(&original[0]), &desired),
        Ok(())
    );
    // A read-back that shows some other order is not a success.
    let wrong = apollo_domain::library::moved(&original, 1, 3);
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let now = sim.occurrences("PLsimroad");
    assert_eq!(
        move_row(&sim, &none(), &road_trip(), &now[0], None, &wrong),
        Err(WriteFailure::Unknown)
    );
}

#[test]
fn a_long_playlist_is_read_back_whole_across_its_pages() {
    let sim = SimLibrary::instant();
    let long = playlist(LONG_ID.trim_start_matches("VL"), "Long Haul");
    assert_eq!(all_rows(&sim, &long).unwrap().len(), LONG_LEN);
}

// ---- save to the library ---------------------------------------------------------------------

#[test]
fn an_album_is_saved_and_unsaved_on_its_audio_playlist() {
    let sim = SimLibrary::instant();
    let night = album("MPREbsimnight");
    let id = sim.save_target(&night).unwrap();
    assert!(id.starts_with("OLAK5uy_"), "{id}");
    set_saved(&sim, &none(), &night, &id, false).unwrap();
    assert!(!sim.collections().unwrap().iter().any(|c| c.library_key() == "MPREbsimnight"));
    set_saved(&sim, &none(), &night, &id, true).unwrap();
    assert!(sim.collections().unwrap().iter().any(|c| c.library_key() == "MPREbsimnight"));
}

#[test]
fn a_save_with_a_lost_reply_is_confirmed_by_the_library_and_a_missed_one_is_not() {
    let sim = SimLibrary::instant();
    let night = album("MPREbsimnight");
    let id = sim.save_target(&night).unwrap();
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(set_saved(&sim, &none(), &night, &id, false), Ok(()));
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    // Still saved (the unsave landed above, so save it back first with a clean answer).
    sim.set_write_behaviour(WriteBehaviour::Succeed);
    set_saved(&sim, &none(), &night, &id, true).unwrap();
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(
        set_saved(&sim, &none(), &night, &id, false),
        Err(WriteFailure::Unknown),
        "the unsave did not land, and the library still holds the album"
    );
}

#[test]
fn liked_music_is_never_saved_or_unsaved() {
    let sim = SimLibrary::instant();
    let liked = playlist("LM", "Liked Music");
    assert_eq!(set_saved(&sim, &none(), &liked, "LM", false), Err(WriteFailure::Rejected));
    assert_eq!(set_saved(&sim, &none(), &album("MPREx"), "", true), Err(WriteFailure::Rejected));
    assert!(sim.write_log().is_empty());
}

// ---- the account -----------------------------------------------------------------------------

#[test]
fn a_write_asked_in_one_account_sends_nothing_once_the_account_has_changed() {
    let sim = SimLibrary::instant();
    sim.set_account(Some("1:channel-a"));
    let asked_in = sim.account_token();
    sim.set_account(Some("1:channel-b"));
    assert_eq!(
        create(&sim, &asked_in, &HashSet::new(), "x"),
        Err(WriteFailure::Rejected)
    );
    assert_eq!(delete(&sim, &asked_in, "PLsimroad"), Err(WriteFailure::Rejected));
    assert_eq!(rename(&sim, &asked_in, "PLsimroad", "x"), Err(WriteFailure::Rejected));
    assert_eq!(
        remove(&sim, &asked_in, &road_trip(), "v", "o"),
        Err(WriteFailure::Rejected)
    );
    assert_eq!(
        move_row(&sim, &asked_in, &road_trip(), "o", None, &[]),
        Err(WriteFailure::Rejected)
    );
    assert_eq!(
        set_saved(&sim, &asked_in, &album("MPREbsimnight"), "OLAK5uy_x", false),
        Err(WriteFailure::Rejected)
    );
    assert!(sim.write_log().is_empty(), "nothing went anywhere: {:?}", sim.write_log());
    // Signed out then in again is a change too.
    sim.set_account(None);
    assert_eq!(create(&sim, &asked_in, &HashSet::new(), "x"), Err(WriteFailure::Rejected));
    // The same account goes through.
    sim.set_account(Some("1:channel-a"));
    assert!(create(&sim, &asked_in, &HashSet::new(), "x").is_ok());
}

// ---- words -----------------------------------------------------------------------------------

#[test]
fn every_failure_text_is_six_words_or_fewer_and_a_dead_session_is_named() {
    for edit in [
        Edit::Create,
        Edit::SaveQueue,
        Edit::SaveCrate,
        Edit::Update,
        Edit::Delete,
        Edit::RemoveSong,
        Edit::MoveSong,
        Edit::Save,
        Edit::Unsave,
    ] {
        for failure in [
            WriteFailure::Session,
            WriteFailure::Throttled,
            WriteFailure::Rejected,
            WriteFailure::Unknown,
            WriteFailure::Unsupported,
        ] {
            let text = failure_text(edit, failure);
            assert!(text.split_whitespace().count() <= 6, "{text}");
        }
        assert_eq!(failure_text(edit, WriteFailure::Session), "Sign-in expired");
        assert_eq!(failure_text(edit, WriteFailure::Throttled), "Too many requests");
    }
}

#[test]
fn only_playlists_the_service_lets_people_save_are_offered_for_saving() {
    assert!(is_saveable_playlist_id("PLabcdef"));
    assert!(is_saveable_playlist_id("RDCLAK5uy_abc"));
    // Radios, song mixes and the special lists are not.
    assert!(!is_saveable_playlist_id("RDAMVMdQw4w9WgXcQ"));
    assert!(!is_saveable_playlist_id("RDEMabc"));
    assert!(!is_saveable_playlist_id("LM"));
    assert!(!is_saveable_playlist_id("OLAK5uy_abc"));
    assert!(!is_saveable_playlist_id(""));
}

// ---- edit playlist (description and visibility) --------------------------------------------------

fn change(description: Option<&str>, privacy: Option<Privacy>) -> PlaylistEdit {
    PlaylistEdit {
        title: None,
        description: description.map(str::to_owned),
        privacy,
    }
}

#[test]
fn an_edit_sends_only_what_changed_and_the_service_holds_it_afterwards() {
    let sim = SimLibrary::instant();
    let road = playlist("PLsimroad", "Road Trip");
    edit(&sim, &none(), &road, &change(Some("Songs for the highway"), Some(Privacy::Unlisted)))
        .unwrap();
    assert_eq!(
        sim.playlist_details("PLsimroad"),
        (Some("Songs for the highway".into()), Some(Privacy::Unlisted))
    );
    assert_eq!(sim.write_log().len(), 1, "one write carries every part");
    assert!(
        !sim.write_log()[0].0.contains("title="),
        "the title was not asked to change: {:?}",
        sim.write_log()
    );
}

#[test]
fn an_edit_that_changes_nothing_is_not_sent() {
    let sim = SimLibrary::instant();
    let road = playlist("PLsimroad", "Road Trip");
    assert_eq!(edit(&sim, &none(), &road, &PlaylistEdit::default()), Ok(()));
    assert!(sim.write_log().is_empty());
}

#[test]
fn clearing_a_description_is_an_edit_of_its_own() {
    let sim = SimLibrary::instant();
    let road = playlist("PLsimroad", "Road Trip");
    edit(&sim, &none(), &road, &change(Some("old"), None)).unwrap();
    edit(&sim, &none(), &road, &change(Some(""), None)).unwrap();
    assert_eq!(sim.playlist_details("PLsimroad").0.as_deref(), Some(""));
}

#[test]
fn an_edit_with_a_lost_reply_is_confirmed_by_reading_every_part_back() {
    let sim = SimLibrary::instant();
    let road = playlist("PLsimroad", "Road Trip");
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(
        edit(&sim, &none(), &road, &change(Some("Night drives"), Some(Privacy::Public))),
        Ok(())
    );
    // A write that did not land is not believed, whatever the reply says.
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    let long = playlist("PLsimlong", "Long Haul");
    assert_eq!(
        edit(&sim, &none(), &long, &change(Some("Elsewhere"), None)),
        Err(WriteFailure::Unknown)
    );
    assert_eq!(sim.write_log().len(), 2, "each sent once");
}

#[test]
fn an_edit_is_never_sent_for_what_the_account_does_not_own_or_from_another_account() {
    let sim = SimLibrary::instant();
    for id in ["LM", "OLAK5uy_x", "RDCLAK5uy_x", "VLPLsimroad", ""] {
        assert_eq!(
            edit(&sim, &none(), &playlist(id, "x"), &change(Some("d"), None)),
            Err(WriteFailure::Rejected),
            "{id}"
        );
    }
    sim.set_account(Some("1:channel-a"));
    let asked_in = sim.account_token();
    sim.set_account(Some("1:channel-b"));
    assert_eq!(
        edit(&sim, &asked_in, &playlist("PLsimroad", "x"), &change(Some("d"), None)),
        Err(WriteFailure::Rejected)
    );
    assert!(sim.write_log().is_empty());
}

// ---- save the queue as a playlist ------------------------------------------------------------

fn videos(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_saved_queue_is_a_private_playlist_holding_its_songs_in_order() {
    let sim = SimLibrary::instant();
    let ids = videos(&["vidAAAAAAA1", "vidAAAAAAA2", "vidAAAAAAA3"]);
    let id = create_with(&sim, &none(), &known(&sim), "My queue", Privacy::Private, &ids).unwrap();
    assert!(is_user_playlist_id(&id));
    assert_eq!(sim.created_with(&id), ids);
    assert_eq!(sim.playlist_details(&id).1, Some(Privacy::Private));
    assert!(titles(&sim).contains(&"My queue".to_owned()));
    assert_eq!(sim.write_log().len(), 1, "one write for the playlist and its songs");
}

#[test]
fn a_saved_queue_whose_reply_is_lost_is_found_by_reading_back_and_never_sent_twice() {
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let ids = videos(&["vidAAAAAAA1"]);
    let id = create_with(&sim, &none(), &known(&sim), "Night queue", Privacy::Private, &ids).unwrap();
    assert!(titles(&sim).contains(&"Night queue".to_owned()));
    assert_eq!(sim.created_with(&id), ids);
    assert_eq!(sim.write_log().len(), 1);
    // A write that did not land finds nothing and fails.
    let sim = SimLibrary::instant();
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(
        create_with(&sim, &none(), &known(&sim), "Never", Privacy::Private, &ids),
        Err(WriteFailure::Unknown)
    );
}

// ---- songs in the library --------------------------------------------------------------------

#[test]
fn a_song_is_saved_to_the_library_and_taken_out_again() {
    let sim = SimLibrary::instant();
    sim.preset_library_songs(&["vidAAAAAAA1"]);
    set_song_saved(&sim, &none(), "vidAAAAAAA2", true).unwrap();
    assert_eq!(sim.library_song_ids(), ["vidAAAAAAA2", "vidAAAAAAA1"]);
    set_song_saved(&sim, &none(), "vidAAAAAAA1", false).unwrap();
    assert_eq!(sim.library_song_ids(), ["vidAAAAAAA2"]);
    assert_eq!(sim.write_log().len(), 2);
}

#[test]
fn a_song_toggle_with_a_lost_reply_is_confirmed_by_the_songs_list_and_a_missed_one_is_not() {
    let sim = SimLibrary::instant();
    sim.preset_library_songs(&[]);
    sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    assert_eq!(set_song_saved(&sim, &none(), "vidAAAAAAA1", true), Ok(()));
    assert_eq!(sim.library_song_ids(), ["vidAAAAAAA1"]);
    sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    assert_eq!(
        set_song_saved(&sim, &none(), "vidAAAAAAA2", true),
        Err(WriteFailure::Unknown)
    );
    assert_eq!(sim.write_log().len(), 2, "each sent once");
}

#[test]
fn a_song_toggle_is_not_sent_from_another_account_or_for_nothing() {
    let sim = SimLibrary::instant();
    sim.set_account(Some("1:channel-a"));
    let asked_in = sim.account_token();
    sim.set_account(Some("1:channel-b"));
    assert_eq!(set_song_saved(&sim, &asked_in, "vidAAAAAAA1", true), Err(WriteFailure::Rejected));
    assert_eq!(set_song_saved(&sim, &none(), "", true), Err(WriteFailure::Rejected));
    assert!(sim.write_log().is_empty());
}

#[test]
fn every_page_of_the_songs_list_is_read_for_a_read_back() {
    let sim = SimLibrary::instant();
    sim.preset_long_library_songs();
    let rows = all_song_rows(&sim).expect("whole list");
    assert_eq!(rows.len(), crate::library::sim::LONG_SONGS);
}
