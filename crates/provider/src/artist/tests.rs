use super::*;
use innertube::models::browse::parse_artist;
use serde_json::{json, Value};
use std::time::Duration;

/// A real anonymous artist page (2026-10-03), cut down to its header and two carousels of three
/// cards. Anonymous pages carry no top-songs shelf and no albums (the service offers those to
/// signed-in sessions), so they exercise the carousels the page must tolerate: a "Videos" row
/// that opens nothing here and a "Playlists" row that does.
const RECORDED: &str = include_str!("../../tests/fixtures/artist_anonymous_recorded.json");

fn song_row(video_id: &str, title: &str) -> Value {
    json!({ "musicResponsiveListItemRenderer": {
        "playlistItemData": { "videoId": video_id },
        "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                { "text": "An Artist", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCartist" } } }
            ] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                { "text": "Some Album", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_album" } } }
            ] } } }
        ],
        "fixedColumns": [
            { "musicResponsiveListItemFixedColumnRenderer": { "text": { "runs": [{ "text": "3:21" }] } } }
        ]
    } })
}

fn card(title: &str, subtitle: &str, browse_id: &str) -> Value {
    json!({ "musicTwoRowItemRenderer": {
        "title": { "runs": [{ "text": title }] },
        "subtitle": { "runs": [{ "text": subtitle }] },
        "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
            { "url": "https://example.test/card.jpg", "width": 226, "height": 226 }
        ] } } },
        "navigationEndpoint": { "browseEndpoint": { "browseId": browse_id } }
    } })
}

fn carousel(title: &str, items: Vec<Value>) -> Value {
    json!({ "musicCarouselShelfRenderer": {
        "header": { "musicCarouselShelfBasicHeaderRenderer": { "title": { "runs": [{ "text": title }] } } },
        "contents": items
    } })
}

fn response(name: Option<&str>, shelves: Vec<Value>) -> Value {
    let mut header = json!({});
    if let Some(name) = name {
        header["title"] = json!({ "runs": [{ "text": name }] });
        header["thumbnail"] = json!({ "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
            { "url": "https://example.test/hero.jpg", "width": 1440, "height": 600 }
        ] } } });
    }
    json!({
        "header": { "musicImmersiveHeaderRenderer": header },
        "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
            "sectionListRenderer": { "contents": shelves } } } }] } }
    })
}

fn mapped(root: &Value) -> Result<ArtistPage, LoadFailure> {
    page_from_artist(&parse_artist(root, "UCartist"), "UCartist")
}

fn full() -> Value {
    response(
        Some("An Artist"),
        vec![
            json!({ "musicShelfRenderer": { "contents": [
                song_row("aaaaaaaaaaa", "First"),
                song_row("bbbbbbbbbbb", "Second")
            ] } }),
            carousel(
                "Albums",
                vec![
                    card("Debut", "Album • 2019", "MPREb_debut"),
                    card("Second", "Album • 2021", "MPREb_second"),
                ],
            ),
            carousel(
                "Singles",
                vec![card("A Single", "Single • 2022", "MPREb_single")],
            ),
        ],
    )
}

#[test]
fn a_signed_in_page_maps_into_top_songs_and_album_rows() {
    let page = mapped(&full()).unwrap();
    assert_eq!(page.id.as_str(), "UCartist");
    assert_eq!(page.name, "An Artist");
    assert_eq!(
        page.thumbnail.as_deref(),
        Some("https://example.test/hero.jpg")
    );
    assert_eq!(page.top_songs.len(), 2);
    let first = &page.top_songs[0].track;
    assert_eq!(first.video_id.as_str(), "aaaaaaaaaaa");
    assert_eq!(first.title, "First");
    assert_eq!(first.artists[0].name, "An Artist");
    assert_eq!(first.album.as_ref().unwrap().title, "Some Album");
    assert_eq!(first.duration, Some(Duration::from_secs(201)));
    let titles: Vec<&str> = page.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Albums", "Singles"]);
    let albums = &page.sections[0].items;
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].kind, CollectionKind::Album);
    assert_eq!(albums[0].id.as_str(), "MPREb_debut");
    assert_eq!(albums[0].title, "Debut");
    assert_eq!(albums[0].subtitle.as_deref(), Some("Album • 2019"));
    assert_eq!(
        albums[0].thumbnail.as_deref(),
        Some("https://example.test/card.jpg")
    );
    assert_eq!(page.skipped, 0);
    assert!(!page.is_empty());
}

#[test]
fn music_videos_are_left_out_without_a_failure() {
    let root = response(
        Some("An Artist"),
        vec![
            carousel(
                "Videos",
                vec![json!({ "musicTwoRowItemRenderer": {
                    "title": { "runs": [{ "text": "A Video" }] },
                    "navigationEndpoint": { "watchEndpoint": { "videoId": "vvvvvvvvvvv" } }
                } })],
            ),
            carousel("Featured on", vec![card("A Mix", "Playlist", "VLPLmix")]),
        ],
    );
    let page = mapped(&root).unwrap();
    assert_eq!(page.sections.len(), 1);
    assert_eq!(page.sections[0].title, "Featured on");
    assert_eq!(page.sections[0].items[0].kind, CollectionKind::Playlist);
    assert_eq!(page.skipped, 0, "a video row is not an unreadable row");
}

#[test]
fn similar_artists_become_their_own_list_in_order_without_repeats() {
    let root = response(
        Some("An Artist"),
        vec![carousel(
            "Fans might also like",
            vec![
                card("Someone", "1M subscribers", "UCone"),
                card("Another", "2M subscribers", "UCtwo"),
                card("Someone", "1M subscribers", "UCone"),
                card("   ", "No name", "UCthree"),
            ],
        )],
    );
    let page = mapped(&root).unwrap();
    let names: Vec<&str> = page.related.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Someone", "Another"]);
    assert_eq!(page.related[0].id.as_str(), "UCone");
    assert_eq!(
        page.related[0].thumbnail.as_deref(),
        Some("https://example.test/card.jpg")
    );
    assert!(page.sections.is_empty(), "artists are not an album shelf");
    assert_eq!(page.skipped, 1, "the nameless card is counted, not hidden");
    assert!(!page.is_empty(), "similar artists alone are something to show");
}

#[test]
fn the_header_carries_the_bio_the_counts_and_the_subscription() {
    let mut root = full();
    let header = &mut root["header"]["musicImmersiveHeaderRenderer"];
    header["description"] = json!({ "runs": [{ "text": "  A long bio.  " }] });
    header["monthlyListenerCount"] = json!({ "runs": [{ "text": "137M monthly audience" }] });
    header["subscriptionButton"] = json!({ "subscribeButtonRenderer": {
        "channelId": "UCchannel",
        "subscribed": true,
        "longSubscriberCountText": { "runs": [{ "text": "32.7M subscribers" }] }
    } });
    header["playButton"] = json!({ "buttonRenderer": { "navigationEndpoint": {
        "watchPlaylistEndpoint": { "playlistId": "RDAOshuffle" } } } });
    header["startRadioButton"] = json!({ "buttonRenderer": { "navigationEndpoint": {
        "watchPlaylistEndpoint": { "playlistId": "RDEMradio" } } } });
    let page = mapped(&root).unwrap();
    assert_eq!(page.description.as_deref(), Some("A long bio."));
    assert_eq!(page.subscribers.as_deref(), Some("32.7M subscribers"));
    assert_eq!(
        page.monthly_listeners.as_deref(),
        Some("137M monthly audience")
    );
    assert_eq!(page.channel_id, "UCchannel");
    assert!(page.subscribed);
    assert_eq!(page.radio_playlist_id.as_deref(), Some("RDEMradio"));
    assert_eq!(page.shuffle_playlist_id.as_deref(), Some("RDAOshuffle"));
}

#[test]
fn a_header_with_none_of_that_is_not_subscribed_and_has_nothing_to_show() {
    let page = mapped(&full()).unwrap();
    assert_eq!(page.description, None);
    assert_eq!(page.subscribers, None);
    assert_eq!(page.monthly_listeners, None);
    assert!(!page.subscribed);
    assert_eq!(page.channel_id, "UCartist", "the browse id is the channel id");
    assert_eq!(page.radio_playlist_id, None);
    assert_eq!(page.shuffle_playlist_id, None);
    assert_eq!(page.top_songs_id, None);
}

#[test]
fn show_all_opens_the_top_songs_playlist() {
    let root = response(
        Some("An Artist"),
        vec![json!({ "musicShelfRenderer": {
            "contents": [song_row("aaaaaaaaaaa", "First")],
            "bottomEndpoint": { "browseEndpoint": { "browseId": "VLOLAK5uy_top" } }
        } })],
    );
    let page = mapped(&root).unwrap();
    assert_eq!(page.top_songs_id.unwrap().as_str(), "VLOLAK5uy_top");
}

#[test]
fn unusable_rows_are_counted_not_hidden() {
    let root = response(
        Some("An Artist"),
        vec![
            json!({ "musicShelfRenderer": { "contents": [
                song_row("aaaaaaaaaaa", "Fine"),
                song_row("short", "Bad id")
            ] } }),
            carousel(
                "Albums",
                vec![card("No id", "Album", ""), card("Ok", "Album", "MPREb_ok")],
            ),
        ],
    );
    let page = mapped(&root).unwrap();
    assert_eq!(page.top_songs.len(), 1);
    assert_eq!(page.sections[0].items.len(), 1);
    assert_eq!(page.skipped, 2);
}

#[test]
fn an_artist_with_nothing_to_show_is_empty_not_failed() {
    let page = mapped(&response(Some("Quiet Artist"), vec![])).unwrap();
    assert!(page.is_empty());
    assert_eq!(page.skipped, 0);
}

#[test]
fn rows_that_all_failed_to_read_are_a_parse_failure_not_an_empty_page() {
    let root = response(
        Some("An Artist"),
        vec![json!({ "musicShelfRenderer": { "contents": [song_row("short", "Bad")] } })],
    );
    assert_eq!(mapped(&root).unwrap_err(), LoadFailure::Parse);
}

#[test]
fn a_response_that_is_not_an_artist_page_is_a_parse_failure() {
    assert_eq!(
        mapped(&response(None, vec![])).unwrap_err(),
        LoadFailure::Parse
    );
    assert_eq!(mapped(&json!({})).unwrap_err(), LoadFailure::Parse);
}

#[test]
fn a_recorded_page_maps_its_header_and_the_playlists_it_can_open() {
    let root: Value = serde_json::from_str(RECORDED).unwrap();
    let page = mapped(&root).unwrap();
    assert_eq!(page.name, "The Weeknd");
    assert!(page
        .thumbnail
        .as_deref()
        .is_some_and(|t| t.starts_with("https://")));
    assert!(
        page.top_songs.is_empty(),
        "anonymous pages carry no top songs"
    );
    // The recorded header is where the real shuffle and radio ids were first seen side by side:
    // the shuffle mix (`RDAO…`) sits under `playButton`, which sorts before the radio's button.
    assert_eq!(
        page.shuffle_playlist_id.as_deref(),
        Some("RDAOHSpo_Uv9STIRtF73zMywLg")
    );
    assert_eq!(
        page.radio_playlist_id.as_deref(),
        Some("RDEMHSpo_Uv9STIRtF73zMywLg")
    );
    assert_eq!(
        page.monthly_listeners.as_deref(),
        Some("244M monthly audience")
    );
    assert_eq!(page.sections.len(), 1, "videos open nothing here");
    let playlists = &page.sections[0];
    assert!(playlists.title.starts_with("Playlists"));
    assert_eq!(playlists.items.len(), 3);
    assert!(playlists
        .items
        .iter()
        .all(|c| c.kind == CollectionKind::Playlist && c.id.as_str().starts_with("VLPL")));
    assert_eq!(page.skipped, 0);
}
