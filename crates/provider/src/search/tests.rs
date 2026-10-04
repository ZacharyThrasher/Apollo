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

/// Rows as the service writes them for episodes, videos and songs whose subtitle has no artist
/// or no duration (shapes seen in a real anonymous search for an artist, 2026-10-03).
const ODDITIES: &str = include_str!("../../tests/fixtures/search_row_oddities.json");

fn oddity(title: &str) -> ListedTrack {
    let parsed = parse_search_all(&serde_json::from_str(ODDITIES).unwrap());
    search_results(&parsed)
        .songs
        .into_iter()
        .find(|s| s.track.title == title)
        .unwrap_or_else(|| panic!("{title} is listed"))
}

fn artist_of(t: &ListedTrack) -> Option<&str> {
    t.track.artists.first().map(|a| a.name.as_str())
}

#[test]
fn a_date_is_never_shown_as_the_artist() {
    // The row's date and type label go; what is left (the show) stands in for the artist.
    for (title, show) in [
        ("Episode with a full date", "Versus Wolves Podcast"),
        ("Episode with a relative date", "Inside the Industry w/ J Nolan"),
        ("Episode with a short date", "More Than A Coincidence Series"),
    ] {
        assert_eq!(artist_of(&oddity(title)), Some(show), "{title}");
    }
}

#[test]
fn a_view_count_is_never_shown_as_the_artist() {
    assert_eq!(artist_of(&oddity("Video with views only")), None);
    assert_eq!(artist_of(&oddity("Video with a channel")), Some("Kenneth Music"));
}

#[test]
fn a_name_that_starts_with_a_number_is_still_an_artist() {
    assert_eq!(artist_of(&oddity("Song by a numeric name")), Some("21 Savage"));
}

#[test]
fn a_missing_duration_stays_empty() {
    assert_eq!(oddity("Song with no duration").track.duration, None);
    assert_eq!(artist_of(&oddity("Song with no duration")), Some("Kendrick Lamar"));
    assert_eq!(
        oddity("Song with a duration").track.duration,
        Some(Duration::from_secs(4 * 60 + 35))
    );
}

#[test]
fn hiding_music_videos_drops_only_the_songs_flagged_as_videos() {
    let song = |id: &str| ListedTrack {
        track: Track::new(id, id),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    };
    let mut results = SearchResults {
        songs: vec![song("aaaaaaaaaaa"), song("bbbbbbbbbbb"), song("ccccccccccc")],
        ..SearchResults::default()
    };
    results.video_ids.insert("bbbbbbbbbbb".into());
    let audio = results.without_videos();
    let ids: Vec<&str> = audio.songs.iter().map(|s| s.track.video_id.as_str()).collect();
    assert_eq!(ids, ["aaaaaaaaaaa", "ccccccccccc"]);
    assert_eq!(results.songs.len(), 3, "the original is untouched");
}

fn card(title: &str, kind_browse: &str) -> Value {
    json!({ "musicCardShelfRenderer": {
        "title": { "runs": [{ "text": title, "navigationEndpoint": { "browseEndpoint": { "browseId": kind_browse } } }] },
        "subtitle": { "runs": [{ "text": "Album" }, { "text": " • " }, { "text": "Daft Punk" }, { "text": " • " }, { "text": "2001" }] },
        "contents": []
    } })
}

fn with_card(card: Value, rows: Vec<Value>) -> SearchResults {
    let mut sections: Vec<Value> = vec![card];
    sections.extend(
        rows.into_iter()
            .map(|r| json!({ "itemSectionRenderer": { "contents": [r] } })),
    );
    let root = json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
        "sectionListRenderer": { "contents": sections } } } }] } } });
    search_results(&parse_search_all(&root))
}

#[test]
fn the_cards_main_row_is_the_top_result_and_keeps_its_place_in_its_group() {
    let r = with_card(
        card("Discovery", "MPREb_disc"),
        vec![row(
            "One More Time",
            &[("Song", None), (" • ", None), ("Daft Punk", Some("UCdaft"))],
            None,
            Some("aaaaaaaaaaa"),
        )],
    );
    match &r.top {
        Some(TopResult::Album(hit)) => {
            assert_eq!(hit.title, "Discovery");
            assert_eq!(hit.subtitle.as_deref(), Some("Daft Punk • 2001"));
        }
        other => panic!("an album card is an album top result, got {other:?}"),
    }
    assert_eq!(r.albums[0].title, "Discovery", "the group is still complete");
    assert_eq!(r.songs.len(), 1);
}

#[test]
fn no_card_means_no_top_result() {
    let r = grouped(vec![row(
        "Digital Love",
        &[("Song", None)],
        None,
        Some("aaaaaaaaaaa"),
    )]);
    assert_eq!(r.top, None);
}

#[test]
fn a_music_video_top_result_goes_with_the_hidden_videos() {
    let mut r = SearchResults::default();
    let song = |id: &str| ListedTrack {
        track: apollo_domain::model::Track::new(id, "A Video"),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    };
    r.top = Some(TopResult::Song(song("video000001")));
    r.songs = vec![song("video000001"), song("audio000001")];
    r.video_ids.insert("video000001".into());
    let hidden = r.without_videos();
    assert_eq!(hidden.top, None);
    assert_eq!(hidden.songs.len(), 1);
    // A top result that is not a video stays.
    r.top = Some(TopResult::Song(song("audio000001")));
    assert!(r.without_videos().top.is_some());
}

fn page(rows: Vec<Value>, token: Option<&str>) -> SearchPage {
    let mut shelf = json!({ "contents": rows });
    if let Some(t) = token {
        shelf["continuations"] = json!([{ "nextContinuationData": { "continuation": t } }]);
    }
    let root = json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
        "sectionListRenderer": { "contents": [{ "musicShelfRenderer": shelf }] } } } }] } } });
    search_page(&innertube::models::browse::parse_search_page(&root))
}

#[test]
fn a_filtered_page_maps_its_rows_and_carries_the_token() {
    let song = |id: &str| row("A Song", &[("Daft Punk", Some("UCdaft")), (" • ", None), ("3:02", None)], None, Some(id));
    let p = page(vec![song("aaaaaaaaaaa"), song("bbbbbbbbbbb")], Some("NEXT"));
    assert_eq!(p.results.songs.len(), 2);
    assert_eq!(p.results.top, None, "a filtered page has no top result");
    assert_eq!(p.continuation.as_deref(), Some("NEXT"));
    assert_eq!(page(vec![song("ccccccccccc")], None).continuation, None);
}

#[test]
fn appending_a_page_keeps_one_copy_of_a_row_that_crossed_the_boundary() {
    let song = |id: &str| row("A Song", &[("Daft Punk", Some("UCdaft"))], None, Some(id));
    let album = |id: &str| row("An Album", &[("2001", None)], Some(id), None);
    let mut first = page(vec![song("aaaaaaaaaaa"), album("MPREb_one")], Some("T1")).results;
    let second = page(vec![song("aaaaaaaaaaa"), song("bbbbbbbbbbb"), album("MPREb_one"), album("MPREb_two")], None).results;
    first.append(second);
    assert_eq!(first.songs.len(), 2);
    assert_eq!(first.albums.len(), 2);
}

#[test]
fn every_filter_has_its_own_params() {
    let all = [
        SearchFilter::Songs,
        SearchFilter::Albums,
        SearchFilter::Artists,
        SearchFilter::Playlists,
        SearchFilter::CommunityPlaylists,
    ];
    let params: std::collections::BTreeSet<_> = all.iter().map(|f| f.params()).collect();
    assert_eq!(params.len(), all.len());
}
