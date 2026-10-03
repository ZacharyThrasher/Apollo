use super::*;
use apollo_domain::listing::{Applied, Listing, LoadState};
use innertube::models::browse::{parse_playlist, parse_playlist_continuation};
use innertube::models::metadata::{parse_account_identities, parse_account_menu};
use serde_json::{json, Value};

const MENU: &str = include_str!("../../../innertube/tests/fixtures/account_menu_single.json");
const MULTI: &str = include_str!("../../../innertube/tests/fixtures/accounts_list_multiple.json");

fn row(video_id: &str, set_id: &str, title: &str) -> Value {
    json!({ "musicResponsiveListItemRenderer": {
        "playlistItemData": { "videoId": video_id, "playlistSetVideoId": set_id },
        "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                { "text": "Some Artist", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCabc" } } },
                { "text": " • " },
                { "text": "Some Album", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_x" } } }
            ] } } }
        ],
        "fixedColumns": [
            { "musicResponsiveListItemFixedColumnRenderer": { "text": { "runs": [{ "text": "3:47" }] } } }
        ]
    } })
}

fn playlist_json(rows: Vec<Value>, token: Option<&str>) -> Value {
    let mut contents = rows;
    if let Some(t) = token {
        contents.push(
            json!({ "continuationItemRenderer": { "continuationEndpoint": {
            "continuationCommand": { "token": t } } } }),
        );
    }
    json!({ "contents": { "musicPlaylistShelfRenderer": { "contents": contents } } })
}

fn continuation_json(rows: Vec<Value>, token: Option<&str>) -> Value {
    let mut v = json!({ "continuationContents": { "musicPlaylistShelfContinuation": { "contents": rows } } });
    if let Some(t) = token {
        v["continuationContents"]["musicPlaylistShelfContinuation"]["continuations"] =
            json!([{ "nextContinuationData": { "continuation": t } }]);
    }
    v
}

#[test]
fn maps_a_track_row_into_domain_types() {
    let p = parse_playlist(&playlist_json(vec![row("aaaaaaaaaaa", "s1", "Song")], None));
    let page = page_from_playlist(&p);
    let t = &page.items[0];
    assert_eq!(t.track.video_id.as_str(), "aaaaaaaaaaa");
    assert_eq!(t.track.title, "Song");
    assert_eq!(t.track.artists[0].name, "Some Artist");
    assert_eq!(t.track.artists[0].id.as_ref().unwrap().as_str(), "UCabc");
    assert_eq!(t.track.album.as_ref().unwrap().title, "Some Album");
    assert_eq!(t.track.duration, Some(Duration::from_secs(227)));
    assert_eq!(t.occurrence.as_deref(), Some("s1"));
}

#[test]
fn durations_that_are_not_durations_stay_unknown() {
    for bad in ["", "abc", "1:2:3:4", "0:00", "-1:00", "1::2"] {
        assert_eq!(parse_duration(bad), None, "{bad:?}");
    }
    assert_eq!(parse_duration("1:02:03"), Some(Duration::from_secs(3723)));
    assert_eq!(parse_duration("0:59"), Some(Duration::from_secs(59)));
}

#[test]
fn a_row_with_a_malformed_video_id_is_skipped_not_played() {
    for bad in [
        "",
        "short",
        "waytoolongvideoid",
        "has space!!",
        "ünïcödé1234",
    ] {
        let song = SongItem {
            video_id: bad.into(),
            title: "T".into(),
            ..Default::default()
        };
        assert!(track_from_song(&song).is_none(), "{bad:?}");
    }
    let page = page(
        &[
            SongItem {
                video_id: "aaaaaaaaaaa".into(),
                title: "ok".into(),
                ..Default::default()
            },
            SongItem {
                video_id: "bad".into(),
                title: "x".into(),
                ..Default::default()
            },
        ],
        None,
        2,
    );
    assert_eq!(
        (page.items.len(), page.skipped),
        (1, 3),
        "parser skips plus mapping skips"
    );
}

#[test]
fn collab_artists_keep_only_the_linked_names() {
    let song = SongItem {
        video_id: "aaaaaaaaaaa".into(),
        title: "T".into(),
        artists: "A & B".into(),
        artist_runs: vec![
            innertube::models::metadata::ArtistRun {
                text: "A".into(),
                id: Some("UC1".into()),
            },
            innertube::models::metadata::ArtistRun {
                text: " & ".into(),
                id: None,
            },
            innertube::models::metadata::ArtistRun {
                text: "B".into(),
                id: Some("UC2".into()),
            },
        ],
        ..Default::default()
    };
    let names: Vec<_> = track_from_song(&song)
        .unwrap()
        .track
        .artists
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["A", "B"]);
}

// TEST-24: unknown renderer in a page -> counted, neighbours kept, listing not "empty".
#[test]
fn an_unknown_renderer_is_a_counted_skip_not_an_empty_list() {
    let only_unknown = playlist_json(vec![json!({ "musicBrandNewRenderer": {} })], None);
    let page = page_from_playlist(&parse_playlist(&only_unknown));
    assert!(page.items.is_empty());
    assert_eq!(page.skipped, 1);

    let mut l = Listing::new();
    let rev = l.begin();
    assert_eq!(l.apply(rev, page), Applied::Yes);
    assert_eq!(l.skipped(), 1);
    assert!(
        !l.is_confirmed_empty(),
        "an unparsed page is not a confirmed-empty library"
    );

    let mixed = playlist_json(
        vec![
            row("aaaaaaaaaaa", "s1", "One"),
            json!({ "musicBrandNewRenderer": {} }),
            row("bbbbbbbbbbb", "s2", "Two"),
        ],
        None,
    );
    let page = page_from_playlist(&parse_playlist(&mixed));
    assert_eq!((page.items.len(), page.skipped), (2, 1));
}

// TEST-24: a service that keeps returning the same cursor must stop, keeping what loaded.
#[test]
fn a_repeated_cursor_stops_paging_and_keeps_the_items() {
    let mut l = Listing::new();
    let rev = l.begin();
    l.apply(
        rev,
        page_from_playlist(&parse_playlist(&playlist_json(
            vec![row("aaaaaaaaaaa", "s1", "One")],
            Some("t1"),
        ))),
    );
    assert_eq!(l.state(), LoadState::HasMore);
    let next = |rows| {
        page_from_continuation(&parse_playlist_continuation(&continuation_json(
            rows,
            Some("t1"),
        )))
    };
    assert_eq!(
        l.apply(rev, next(vec![row("bbbbbbbbbbb", "s2", "Two")])),
        Applied::RepeatedContinuation
    );
    assert_eq!(l.state(), LoadState::Truncated);
    assert_eq!(
        l.items().len(),
        1,
        "the looping page's items are dropped, earlier ones kept"
    );
}

#[test]
fn overlapping_pages_dedupe_by_occurrence_and_keep_intentional_repeats() {
    let mut l = Listing::new();
    let rev = l.begin();
    // Same video queued twice on purpose: distinct occurrence tokens.
    l.apply(
        rev,
        page_from_playlist(&parse_playlist(&playlist_json(
            vec![
                row("aaaaaaaaaaa", "s1", "One"),
                row("aaaaaaaaaaa", "s2", "One again"),
            ],
            Some("t1"),
        ))),
    );
    // The next page repeats s2 (overlap) and adds s3.
    l.apply(
        rev,
        page_from_continuation(&parse_playlist_continuation(&continuation_json(
            vec![
                row("aaaaaaaaaaa", "s2", "One again"),
                row("ccccccccccc", "s3", "Three"),
            ],
            None,
        ))),
    );
    let occ: Vec<_> = l
        .items()
        .iter()
        .map(|i| i.occurrence.clone().unwrap())
        .collect();
    assert_eq!(occ, ["s1", "s2", "s3"]);
    assert_eq!(l.state(), LoadState::Complete);
}

#[test]
fn only_a_service_stated_rejection_is_a_session_failure() {
    assert_eq!(
        classify(&innertube::Error::SessionExpired),
        LoadFailure::Session
    );
    let json_err = serde_json::from_str::<Value>("{").unwrap_err();
    assert_eq!(
        classify(&innertube::Error::Json(json_err)),
        LoadFailure::Parse
    );
    assert_eq!(
        classify(&innertube::Error::Other("x".into())),
        LoadFailure::Other
    );
    assert_eq!(
        classify(&innertube::Error::VisitorDataNotFound),
        LoadFailure::Other
    );
}

#[test]
fn account_validation_uses_server_issued_identities() {
    let menu = parse_account_menu(&serde_json::from_str(MENU).unwrap());
    let ids = parse_account_identities(&serde_json::from_str(MULTI).unwrap());
    assert!(ids.len() >= 2, "fixture has several identities");
    let v = account_validation(&menu, &ids).unwrap();
    assert_eq!(v.identities.len(), ids.len());
    assert_eq!(
        v.premium_signal, None,
        "Premium is never inferred from the account menu"
    );
}

#[test]
fn an_unidentifying_menu_gives_no_verdict_rather_than_a_logout() {
    let empty = AccountInfo::default();
    assert_eq!(
        account_validation(&empty, &[]),
        Err(ValidationError::Unreachable)
    );
}

#[test]
fn a_single_account_session_falls_back_to_the_menu_identity() {
    let menu = AccountInfo {
        name: Some("Me".into()),
        channel_id: Some("UCme".into()),
        ..Default::default()
    };
    let v = account_validation(&menu, &[]).unwrap();
    assert_eq!(v.identities, vec![IdentityId::new("UCme")]);
}

fn card(browse_id: &str, title: &str, subtitle: Option<&str>) -> Value {
    let mut c = json!({ "musicTwoRowItemRenderer": {
        "title": { "runs": [{ "text": title }] },
        "navigationEndpoint": { "browseEndpoint": { "browseId": browse_id } },
        "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
            { "url": "https://img.example/small", "width": 60, "height": 60 },
            { "url": "https://img.example/large", "width": 226, "height": 226 }
        ] } } }
    } });
    if let Some(s) = subtitle {
        c["musicTwoRowItemRenderer"]["subtitle"] = json!({ "runs": [{ "text": s }] });
    }
    c
}

fn grid(cards: Vec<Value>) -> Value {
    json!({ "contents": { "gridRenderer": { "items": cards } } })
}

#[test]
fn library_playlist_cards_become_collections() {
    use innertube::models::browse::parse_library;
    let cards = parse_library(&grid(vec![
        card("VLPLabc", "Road trip", Some("Playlist • 42 songs")),
        card("VLLM", "Liked Music", None),
        card("UCartist", "Not a playlist", None),
    ]));
    let list = collections_from_grid(&cards, CollectionKind::Playlist);
    assert_eq!(list.len(), 2, "artists have no track list here");
    assert_eq!(list[0].kind, CollectionKind::Playlist);
    assert_eq!(list[0].id.as_str(), "VLPLabc");
    assert_eq!(list[0].title, "Road trip");
    assert_eq!(list[0].subtitle.as_deref(), Some("Playlist • 42 songs"));
    assert_eq!(
        list[0].thumbnail.as_deref(),
        Some("https://img.example/large")
    );
    assert!(list[1].is_liked_music());
    assert_eq!(list[1].subtitle, None);
}

#[test]
fn library_album_cards_become_collections() {
    use innertube::models::browse::parse_library;
    let cards = parse_library(&grid(vec![
        card("MPREb_x", "Night Shift", Some("Album • Neon Arcade • 2024")),
        card("VLPLabc", "A playlist among albums", None),
    ]));
    let list = collections_from_grid(&cards, CollectionKind::Album);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, CollectionKind::Album);
    assert_eq!(list[0].id.as_str(), "MPREb_x");
}

#[test]
fn a_card_without_a_title_or_id_is_dropped_not_shown_blank() {
    let mut c = innertube::models::browse::parse_library(&grid(vec![card("VLPLabc", "x", None)]));
    c[0].title = "  ".into();
    assert!(collections_from_grid(&c, CollectionKind::Playlist).is_empty());
    c[0].title = "x".into();
    c[0].id = String::new();
    assert!(collections_from_grid(&c, CollectionKind::Playlist).is_empty());
}
