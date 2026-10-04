use super::*;
use innertube::models::browse::{parse_cards, parse_explore, parse_moods};
use serde_json::{json, Value};

/// Real anonymous answers (2026-10-04), cut down to four rows a shelf with menus and tracking
/// removed. Anonymous is the only kind recorded: the service refuses a guest the new-releases
/// and moods pages, so the signed-in shapes below are built by hand from the same renderers.
const EXPLORE: &str = include_str!("../../tests/fixtures/explore_anonymous_recorded.json");
const CHARTS: &str = include_str!("../../tests/fixtures/charts_anonymous_recorded.json");

fn thumbs() -> Value {
    json!({ "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
        { "url": "https://example.test/small.jpg", "width": 60, "height": 60 },
        { "url": "https://example.test/large.jpg", "width": 226, "height": 226 }
    ] } } })
}

fn run(text: &str, browse: Option<&str>) -> Value {
    match browse {
        Some(id) => {
            json!({ "text": text, "navigationEndpoint": { "browseEndpoint": { "browseId": id } } })
        }
        None => json!({ "text": text }),
    }
}

fn list_song(id: &str, title: &str) -> Value {
    json!({ "musicResponsiveListItemRenderer": {
        "thumbnail": thumbs(),
        "playlistItemData": { "videoId": id },
        "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                run("Some Artist", Some("UCartist")), run(" • ", None), run("Some Album", Some("MPREb_x"))
            ] } } }
        ]
    } })
}

fn album_card(id: &str, title: &str) -> Value {
    json!({ "musicTwoRowItemRenderer": {
        "thumbnailRenderer": thumbs(),
        "title": { "runs": [{ "text": title }] },
        "subtitle": { "runs": [run("Single", None), run(" • ", None), run("Some Artist", Some("UCartist"))] },
        "navigationEndpoint": { "browseEndpoint": { "browseId": id } }
    } })
}

fn playlist_card(id: &str, title: &str) -> Value {
    json!({ "musicTwoRowItemRenderer": {
        "thumbnailRenderer": thumbs(),
        "title": { "runs": [{ "text": title }] },
        "subtitle": { "runs": [run("Playlist", None)] },
        "navigationEndpoint": { "browseEndpoint": { "browseId": id } }
    } })
}

fn carousel(title: &str, more: Option<&str>, contents: Vec<Value>) -> Value {
    let mut header = json!({ "title": { "runs": [{ "text": title }] } });
    if let Some(id) = more {
        header["moreContentButton"] =
            json!({ "buttonRenderer": { "navigationEndpoint": { "browseEndpoint": { "browseId": id } } } });
    }
    json!({ "musicCarouselShelfRenderer": {
        "header": { "musicCarouselShelfBasicHeaderRenderer": header },
        "contents": contents
    } })
}

fn list_shelf(title: &str, more: &str, contents: Vec<Value>) -> Value {
    json!({ "musicShelfRenderer": {
        "title": { "runs": [{ "text": title }] },
        "bottomEndpoint": { "browseEndpoint": { "browseId": more } },
        "contents": contents
    } })
}

fn response(shelves: Vec<Value>) -> Value {
    json!({ "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": {
        "content": { "sectionListRenderer": { "contents": shelves } } } }] } } })
}

fn songs(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| list_song(&format!("song{i:07}"), &format!("Song {i}")))
        .collect()
}

#[test]
fn the_recorded_trending_shelf_is_the_song_chart_and_the_rest_is_left_out() {
    let explore = parse_explore(&serde_json::from_str(EXPLORE).unwrap());
    let charts = parse_explore(&serde_json::from_str(CHARTS).unwrap());
    let page = explore_page(Some(&explore), Some(&charts), None, None);

    assert_eq!(page.top_songs.len(), 4, "the four rows the fixture keeps");
    assert!(!page.top_songs[0].track.title.is_empty());
    let all = page.top_songs_all.expect("Trending names its chart page");
    assert_eq!(all.kind, CollectionKind::Playlist);
    assert!(all.id.as_str().starts_with("VLOLAK5uy_"), "{}", all.id.as_str());
    // Podcast episodes, music videos and the chart playlists of videos are not Explore's.
    assert!(page.shelves.is_empty(), "{:?}", page.shelves);
    assert!(page.new_releases.is_empty() && page.moods.is_empty());
    assert!(page.skipped > 0, "what was left out is counted");
}

#[test]
fn a_chart_page_with_a_ranked_list_beats_the_explore_trending_shelf() {
    let explore = parse_explore(&response(vec![carousel("Trending", None, songs(3))]));
    let charts = parse_explore(&response(vec![list_shelf("Top songs", "VLPLtop100", songs(25))]));
    let page = explore_page(Some(&explore), Some(&charts), None, None);
    assert_eq!(page.top_songs.len(), 25);
    assert_eq!(page.top_songs_all.unwrap().id.as_str(), "VLPLtop100");
    assert!(page.shelves.is_empty(), "the unused Trending shelf is not shown twice");
}

#[test]
fn trending_cards_stay_a_shelf_and_a_song_shelf_with_another_name_is_not_a_chart() {
    let explore = parse_explore(&response(vec![
        carousel(
            "Trending",
            None,
            vec![album_card("MPREb_a", "Album A"), playlist_card("VLPLb", "Mix B")],
        ),
        carousel("Fresh finds", None, songs(2)),
    ]));
    let page = explore_page(Some(&explore), None, None, None);
    assert!(page.top_songs.is_empty());
    let titles: Vec<&str> = page.shelves.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Trending", "Fresh finds"]);
}

#[test]
fn new_releases_come_from_the_albums_page_else_from_the_explore_shelf() {
    let releases = parse_cards(&response(vec![
        carousel(
            "",
            None,
            vec![
                album_card("MPREb_one", "One"),
                album_card("MPREb_two", "Two"),
                album_card("MPREb_one", "One again"),
            ],
        ),
    ]));
    let explore = parse_explore(&response(vec![carousel(
        "New albums & singles",
        Some("FEmusic_new_releases_albums"),
        vec![album_card("MPREb_shelf", "From the shelf")],
    )]));

    let page = explore_page(Some(&explore), None, Some(&releases), None);
    let ids: Vec<&str> = page.new_releases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["MPREb_one", "MPREb_two"], "a card seen twice is kept once");
    assert_eq!(page.new_releases[0].kind, CollectionKind::Album);
    assert_eq!(page.new_releases[0].subtitle.as_deref(), Some("Single • Some Artist"));
    assert!(page.shelves.is_empty(), "the shelf is the same list, not a second one");

    let page = explore_page(Some(&explore), None, None, None);
    assert_eq!(page.new_releases.len(), 1);
    assert_eq!(page.new_releases[0].id.as_str(), "MPREb_shelf");
}

#[test]
fn mood_tiles_are_one_grid_without_repeats() {
    let moods = parse_moods(&json!({ "contents": [
        { "gridRenderer": {
            "header": { "gridHeaderRenderer": { "title": { "runs": [{ "text": "Moods & moments" }] } } },
            "items": [tile("Chill", "p1"), tile("Workout", "p2")] } },
        { "gridRenderer": {
            "header": { "gridHeaderRenderer": { "title": { "runs": [{ "text": "Genres" }] } } },
            "items": [tile("Rock", "p3"), tile("Chill", "p1")] } }
    ] }));
    let page = explore_page(None, None, None, Some(&moods));
    let titles: Vec<&str> = page.moods.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, ["Chill", "Workout", "Rock"]);
    assert_eq!(page.moods[1].params, "p2");
    assert!(!page.is_empty());
}

fn tile(title: &str, params: &str) -> Value {
    json!({ "musicNavigationButtonRenderer": {
        "buttonText": { "runs": [{ "text": title }] },
        "solid": { "leftStripeColor": 4_294_901_760u64 },
        "clickCommand": { "browseEndpoint": {
            "browseId": "FEmusic_moods_and_genres_category", "params": params } }
    } })
}

#[test]
fn a_mood_page_lists_its_playlists_once_each() {
    let cards = parse_cards(&response(vec![carousel(
        "",
        None,
        vec![
            playlist_card("VLPLa", "A"),
            playlist_card("VLPLb", "B"),
            playlist_card("VLPLa", "A again"),
            album_card("MPREb_x", "An album, not a playlist"),
        ],
    )]));
    let found = mood_playlists(&cards);
    let ids: Vec<&str> = found.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["VLPLa", "VLPLb"]);
}

#[test]
fn nothing_readable_is_an_empty_page() {
    assert!(explore_page(None, None, None, None).is_empty());
}

#[test]
fn the_worst_failure_is_the_one_named() {
    use LoadFailure::*;
    assert_eq!(worst([Other, Network, Parse]), Some(Network));
    assert_eq!(worst([Parse, Throttled, Network]), Some(Throttled));
    assert_eq!(worst([Network, Session]), Some(Session));
    assert_eq!(worst(Vec::new()), None);
}
