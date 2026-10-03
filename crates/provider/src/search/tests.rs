use super::*;
use innertube::models::browse::parse_search_all;
use serde_json::{json, Value};
use std::time::Duration;

/// A real unfiltered response (2026-10-03, anonymous session), cut down to a handful of rows
/// with tracking params and menus removed. Anonymous is the only kind recorded so far: it is
/// a video/episode/channel mix, which is why its rows exercise the odd shapes.
const RECORDED: &str = include_str!("../../tests/fixtures/search_mixed_recorded.json");

/// A search list row as the service writes it: title, subtitle runs, thumbnail, an optional
/// item-level browse target, an optional video id.
fn row(
    title: &str,
    subtitle: &[(&str, Option<&str>)],
    browse: Option<&str>,
    video: Option<&str>,
) -> Value {
    let runs: Vec<Value> = subtitle
        .iter()
        .map(|(text, id)| match id {
            Some(id) => json!({ "text": text, "navigationEndpoint": { "browseEndpoint": { "browseId": id } } }),
            None => json!({ "text": text }),
        })
        .collect();
    let mut node = json!({
        "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
            { "url": "https://example.test/small.jpg", "width": 60, "height": 60 },
            { "url": "https://example.test/large.jpg", "width": 226, "height": 226 }
        ] } } },
        "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": runs } } }
        ]
    });
    if let Some(b) = browse {
        node["navigationEndpoint"] = json!({ "browseEndpoint": { "browseId": b } });
    }
    if let Some(v) = video {
        node["playlistItemData"] = json!({ "videoId": v });
    }
    json!({ "musicResponsiveListItemRenderer": node })
}

fn response(rows: Vec<Value>) -> Value {
    json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
        "sectionListRenderer": { "contents": rows.into_iter()
            .map(|r| json!({ "itemSectionRenderer": { "contents": [r] } }))
            .collect::<Vec<_>>() } } } }] } } })
}

fn grouped(rows: Vec<Value>) -> SearchResults {
    search_results(&parse_search_all(&response(rows)))
}

#[test]
fn rows_are_grouped_by_what_they_open() {
    let r = grouped(vec![
        row(
            "Digital Love",
            &[
                ("Song", None),
                (" • ", None),
                ("Daft Punk", Some("UCdaft")),
                (" • ", None),
                ("5:01", None),
            ],
            None,
            Some("aaaaaaaaaaa"),
        ),
        row(
            "Discovery",
            &[
                ("Album", None),
                (" • ", None),
                ("Daft Punk", Some("UCdaft")),
                (" • ", None),
                ("2001", None),
            ],
            Some("MPREb_abc"),
            None,
        ),
        row(
            "Daft Punk",
            &[
                ("Artist", None),
                (" • ", None),
                ("84.1M monthly audience", None),
            ],
            Some("UCdaft"),
            None,
        ),
        row(
            "Daft Punk Essentials",
            &[("Playlist", None), (" • ", None), ("Someone", None)],
            Some("VLPLxyz"),
            None,
        ),
    ]);
    assert_eq!(r.songs.len(), 1);
    assert_eq!(r.albums.len(), 1);
    assert_eq!(r.artists.len(), 1);
    assert_eq!(r.playlists.len(), 1);
    assert_eq!(r.skipped, 0);
    assert!(!r.is_empty());
}

#[test]
fn a_song_row_becomes_a_playable_track() {
    let r = grouped(vec![row(
        "Digital Love",
        &[
            ("Song", None),
            (" • ", None),
            ("Daft Punk", Some("UCdaft")),
            (" • ", None),
            ("5:01", None),
        ],
        None,
        Some("aaaaaaaaaaa"),
    )]);
    let t = &r.songs[0];
    assert_eq!(t.track.video_id.as_str(), "aaaaaaaaaaa");
    assert_eq!(t.track.title, "Digital Love");
    assert_eq!(t.track.artists.len(), 1);
    assert_eq!(t.track.artists[0].name, "Daft Punk");
    assert_eq!(t.track.artists[0].id.as_ref().unwrap().as_str(), "UCdaft");
    assert_eq!(t.track.duration, Some(Duration::from_secs(301)));
    assert_eq!(
        t.thumbnail.as_deref(),
        Some("https://example.test/large.jpg")
    );
    assert_eq!(
        t.occurrence, None,
        "a search row is not a playlist occurrence"
    );
}

#[test]
fn cards_lose_the_label_the_group_heading_already_states() {
    let r = grouped(vec![
        row(
            "Discovery",
            &[
                ("Album", None),
                (" • ", None),
                ("Daft Punk", Some("UCdaft")),
                (" • ", None),
                ("2001", None),
            ],
            Some("MPREb_abc"),
            None,
        ),
        row(
            "Homework",
            &[("Single", None), (" • ", None), ("Daft Punk", None)],
            Some("MPREb_def"),
            None,
        ),
        row("Daft Punk", &[("Artist", None)], Some("UCdaft"), None),
    ]);
    assert_eq!(r.albums[0].subtitle.as_deref(), Some("Daft Punk • 2001"));
    assert_eq!(
        r.albums[1].subtitle.as_deref(),
        Some("Single • Daft Punk"),
        "Single carries information"
    );
    assert_eq!(r.artists[0].subtitle, None, "nothing left after the label");
    assert_eq!(r.albums[0].id.as_str(), "MPREb_abc");
}

#[test]
fn a_row_that_cannot_be_used_is_counted_not_hidden() {
    let r = grouped(vec![
        row("Fine", &[("Song", None)], None, Some("aaaaaaaaaaa")),
        row("Bad id", &[("Song", None)], None, Some("short")),
    ]);
    assert_eq!(r.songs.len(), 1);
    assert_eq!(r.skipped, 1);
}

#[test]
fn the_same_row_twice_is_kept_once() {
    let one = row("Digital Love", &[("Song", None)], None, Some("aaaaaaaaaaa"));
    let r = grouped(vec![one.clone(), one]);
    assert_eq!(r.songs.len(), 1);
    assert_eq!(r.skipped, 0);
}

#[test]
fn a_response_with_no_rows_is_empty_not_skipped() {
    let r = grouped(vec![]);
    assert!(r.is_empty());
    assert_eq!(r.skipped, 0);
}

#[test]
fn a_recorded_response_maps_without_loss() {
    let parsed = parse_search_all(&serde_json::from_str(RECORDED).unwrap());
    let r = search_results(&parsed);
    assert!(
        !r.songs.is_empty(),
        "video and episode rows are playable ids"
    );
    assert!(!r.artists.is_empty());
    assert!(!r.playlists.is_empty());
    assert_eq!(r.skipped, 0);
    for t in &r.songs {
        assert_eq!(t.track.video_id.as_str().len(), 11);
        assert!(!t.track.title.is_empty());
    }
    // The top card leads its group.
    assert_eq!(r.artists[0].title, "Daft Punk");
}
