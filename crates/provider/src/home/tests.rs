use super::*;
use innertube::models::browse::parse_home;
use serde_json::{json, Value};

/// A real anonymous response (2026-10-03), cut down: one carousel of podcast shows and the
/// "tell us which artists you like" shelf, with tracking params and menus removed. Anonymous is
/// the only kind recorded so far; the signed-in shapes below are built by hand from the same
/// renderers the service uses for the account's Quick picks and recommendations.
const RECORDED: &str = include_str!("../../tests/fixtures/home_anonymous_recorded.json");

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

/// A "Quick picks" style row: title, then "Artist • 3:21" under it, plays a video.
fn list_song(id: &str, title: &str, artist: &str, artist_id: &str) -> Value {
    json!({ "musicResponsiveListItemRenderer": {
        "thumbnail": thumbs(),
        "playlistItemData": { "videoId": id },
        "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                run(artist, Some(artist_id)), run(" • ", None), run("Some Album", Some("MPREb_x"))
            ] } } }
        ]
    } })
}

/// A card: title that navigates, a subtitle, and the card's own endpoint.
fn card(title: &str, subtitle: Vec<Value>, nav: Value) -> Value {
    json!({ "musicTwoRowItemRenderer": {
        "thumbnailRenderer": thumbs(),
        "title": { "runs": [{ "text": title }] },
        "subtitle": { "runs": subtitle },
        "navigationEndpoint": nav
    } })
}

fn browse(id: &str) -> Value {
    json!({ "browseEndpoint": { "browseId": id } })
}

fn song_card(id: &str, title: &str, artist: &str) -> Value {
    card(
        title,
        vec![
            run("Song", None),
            run(" • ", None),
            run(artist, Some("UCartist")),
        ],
        json!({ "watchEndpoint": { "videoId": id } }),
    )
}

fn shelf(title: &str, contents: Vec<Value>) -> Value {
    json!({ "musicCarouselShelfRenderer": {
        "header": { "musicCarouselShelfBasicHeaderRenderer": { "title": { "runs": [{ "text": title }] } } },
        "contents": contents
    } })
}

fn response(shelves: Vec<Value>, token: Option<&str>) -> Value {
    let mut section_list = json!({ "contents": shelves });
    if let Some(t) = token {
        section_list["continuations"] = json!([{ "nextContinuationData": { "continuation": t } }]);
    }
    json!({ "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": {
        "content": { "sectionListRenderer": section_list } } }] } } })
}

fn mapped(shelves: Vec<Value>, token: Option<&str>) -> Result<HomePage, LoadFailure> {
    home_page(&parse_home(&response(shelves, token)))
}

#[test]
fn a_shelf_of_songs_is_a_song_shelf_and_keeps_its_order() {
    let page = mapped(
        vec![shelf(
            "Quick picks",
            vec![
                list_song("aaaaaaaaaaa", "First", "Daft Punk", "UCdaft"),
                list_song("bbbbbbbbbbb", "Second", "Justice", "UCjust"),
            ],
        )],
        None,
    )
    .unwrap();
    assert_eq!(page.shelves.len(), 1);
    let quick = &page.shelves[0];
    assert_eq!(quick.title, "Quick picks");
    assert!(quick.is_songs());
    let songs = quick.songs();
    assert_eq!(songs[0].track.title, "First");
    assert_eq!(songs[0].track.artists[0].name, "Daft Punk");
    assert_eq!(songs[1].track.video_id.as_str(), "bbbbbbbbbbb");
    assert_eq!(
        songs[0].thumbnail.as_deref(),
        Some("https://example.test/large.jpg")
    );
    assert_eq!(page.skipped, 0);
    assert_eq!(page.continuation, None);
}

#[test]
fn cards_map_to_albums_playlists_and_artists_and_lose_their_kind_label() {
    let page = mapped(
        vec![shelf(
            "Picked for you",
            vec![
                card(
                    "Discovery",
                    vec![
                        run("Album", None),
                        run(" • ", None),
                        run("Daft Punk", Some("UCdaft")),
                        run(" • ", None),
                        run("2001", None),
                    ],
                    browse("MPREb_discovery"),
                ),
                card(
                    "Chill mix",
                    vec![
                        run("Playlist", None),
                        run(" • ", None),
                        run("YouTube Music", None),
                    ],
                    browse("VLPLchill"),
                ),
                card(
                    "Daft Punk",
                    vec![
                        run("Artist", None),
                        run(" • ", None),
                        run("12M subscribers", None),
                    ],
                    browse("UCdaft"),
                ),
                song_card("ccccccccccc", "Digital Love", "Daft Punk"),
            ],
        )],
        None,
    )
    .unwrap();
    let shelf = &page.shelves[0];
    assert!(!shelf.is_songs(), "a mixed shelf is a row of cards");
    let HomeItem::Collection(album) = &shelf.items[0] else {
        panic!("album: {:?}", shelf.items[0])
    };
    assert_eq!(album.kind, CollectionKind::Album);
    assert_eq!(album.id.as_str(), "MPREb_discovery");
    assert_eq!(album.subtitle.as_deref(), Some("Daft Punk • 2001"));
    let HomeItem::Collection(playlist) = &shelf.items[1] else {
        panic!("playlist: {:?}", shelf.items[1])
    };
    assert_eq!(playlist.kind, CollectionKind::Playlist);
    assert_eq!(playlist.id.as_str(), "VLPLchill");
    assert_eq!(playlist.subtitle.as_deref(), Some("YouTube Music"));
    let HomeItem::Artist(artist) = &shelf.items[2] else {
        panic!("artist: {:?}", shelf.items[2])
    };
    assert_eq!(artist.id.as_str(), "UCdaft");
    assert_eq!(artist.subtitle.as_deref(), Some("12M subscribers"));
    let HomeItem::Song(song) = &shelf.items[3] else {
        panic!("song: {:?}", shelf.items[3])
    };
    assert_eq!(song.track.title, "Digital Love");
    assert_eq!(shelf.songs().len(), 1);
}

#[test]
fn what_cannot_be_shown_is_counted_not_dropped_silently() {
    let page = mapped(
        vec![
            shelf(
                "Mixed",
                vec![
                    list_song("aaaaaaaaaaa", "Fine", "A", "UCa"),
                    // A renderer the parser does not know.
                    json!({ "musicMysteryRenderer": {} }),
                    // A podcast show: not music, and no track list to open.
                    card("A Show", vec![run("Podcast", None)], browse("MPSPPLshow")),
                    // A song with no usable video id.
                    song_card("short", "Broken", "A"),
                ],
            ),
            // A shelf kind the app does not read.
            json!({ "musicTastebuilderShelfRenderer": { "primaryText": { "runs": [{ "text": "Tell us" }] } } }),
            // A shelf whose every item is unreadable is a shelf lost.
            shelf("All unreadable", vec![json!({ "musicMysteryRenderer": {} })]),
        ],
        None,
    )
    .unwrap();
    assert_eq!(page.shelves.len(), 1);
    assert_eq!(page.shelves[0].items.len(), 1);
    // Mystery renderer, podcast show and short id (3 items), the unknown shelf and the lost shelf.
    assert_eq!(page.skipped, 5, "{page:?}");
}

#[test]
fn nothing_readable_is_a_parse_failure_and_nothing_at_all_is_an_empty_feed() {
    assert_eq!(
        mapped(vec![json!({ "musicTastebuilderShelfRenderer": {} })], None),
        Err(LoadFailure::Parse)
    );
    let empty = mapped(Vec::new(), None).unwrap();
    assert!(empty.shelves.is_empty());
    assert_eq!(empty.skipped, 0);
}

#[test]
fn a_repeated_item_in_a_shelf_shows_once_and_an_untitled_shelf_is_dropped() {
    let page = mapped(
        vec![
            shelf(
                "Listen again",
                vec![
                    song_card("aaaaaaaaaaa", "One", "A"),
                    song_card("aaaaaaaaaaa", "One", "A"),
                    song_card("bbbbbbbbbbb", "Two", "A"),
                ],
            ),
            shelf("", vec![song_card("ccccccccccc", "Three", "A")]),
        ],
        None,
    )
    .unwrap();
    assert_eq!(page.shelves.len(), 1);
    assert_eq!(page.shelves[0].items.len(), 2);
    assert!(
        page.shelves[0].is_songs(),
        "a shelf of song cards plays as songs"
    );
    assert_eq!(page.skipped, 1, "the untitled shelf");
}

#[test]
fn the_continuation_token_comes_through_and_a_blank_one_does_not() {
    let shelves = || vec![shelf("S", vec![song_card("aaaaaaaaaaa", "One", "A")])];
    assert_eq!(
        mapped(shelves(), Some("NEXT_BATCH"))
            .unwrap()
            .continuation
            .as_deref(),
        Some("NEXT_BATCH")
    );
    assert_eq!(mapped(shelves(), Some("  ")).unwrap().continuation, None);
}

#[test]
fn a_continuation_response_maps_like_the_first_page() {
    // The next batch lives under `continuationContents`; the shelves read the same.
    let root = json!({ "continuationContents": { "sectionListContinuation": {
        "contents": [shelf("More for you", vec![song_card("aaaaaaaaaaa", "One", "A")])],
        "continuations": [{ "nextContinuationData": { "continuation": "AFTER" } }]
    } } });
    let page = home_page(&parse_home(&root)).unwrap();
    assert_eq!(page.shelves[0].title, "More for you");
    assert_eq!(page.continuation.as_deref(), Some("AFTER"));
}

#[test]
fn the_recorded_anonymous_feed_is_all_podcasts_and_a_taste_builder_so_it_is_unreadable() {
    // Everything it carries is something the app skips on purpose; it must say so rather than
    // draw an empty page.
    let root: Value = serde_json::from_str(RECORDED).expect("fixture parses");
    let parsed = parse_home(&root);
    assert_eq!(parsed.sections.len(), 1);
    assert_eq!(parsed.skipped_shelves, 1, "the taste-builder shelf");
    assert_eq!(home_page(&parsed), Err(LoadFailure::Parse));
}

#[test]
fn the_recorded_anonymous_feed_beside_a_music_shelf_keeps_the_music() {
    let mut root: Value = serde_json::from_str(RECORDED).expect("fixture parses");
    let contents = root
        .pointer_mut("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array_mut)
        .expect("section list");
    contents.insert(
        0,
        shelf(
            "Quick picks",
            vec![list_song("aaaaaaaaaaa", "Song", "A", "UCa")],
        ),
    );
    let page = home_page(&parse_home(&root)).unwrap();
    assert_eq!(page.shelves.len(), 1);
    assert_eq!(page.shelves[0].title, "Quick picks");
    // Three podcast shows, the shelf they sat on and the taste-builder shelf.
    assert_eq!(page.skipped, 5);
}

#[test]
fn the_recorded_chips_come_through_without_podcasts_and_with_their_tokens() {
    let mut root: Value = serde_json::from_str(RECORDED).expect("fixture parses");
    let contents = root
        .pointer_mut("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array_mut)
        .expect("section list");
    contents.insert(
        0,
        shelf(
            "Quick picks",
            vec![list_song("aaaaaaaaaaa", "Song", "A", "UCa")],
        ),
    );
    let page = home_page(&parse_home(&root)).unwrap();
    let titles: Vec<&str> = page.chips.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Energize", "Party", "Feel good", "Relax", "Workout", "Commute", "Romance", "Sad", "Focus", "Sleep"],
        "the service's order, Podcasts left out"
    );
    assert!(
        page.chips.iter().all(|c| c.params.starts_with("ggNC")),
        "each carries the token that re-browses the feed for it"
    );
}

#[test]
fn a_page_saved_before_chips_existed_still_reads() {
    let old = r#"{"shelves":[],"continuation":null,"skipped":0}"#;
    let page: HomePage = serde_json::from_str(old).expect("old pages read");
    assert!(page.chips.is_empty());
}
