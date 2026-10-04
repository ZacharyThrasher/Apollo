use super::*;

fn track(id: &str, title: &str, artists: &[&str]) -> ExportTrack {
    ExportTrack {
        video_id: id.to_owned(),
        title: title.to_owned(),
        artists: artists.iter().map(|a| a.to_string()).collect(),
        album: Some("Album".to_owned()),
        duration_seconds: Some(215),
        explicit: false,
        url: format!("https://music.youtube.com/watch?v={id}"),
    }
}

fn sample() -> LibraryExport {
    LibraryExport {
        exported_at: 1_759_565_553,
        liked_songs: vec![track("aaaaaaaaaaa", "Liked One", &["Artist A"])],
        playlists: vec![ExportPlaylist::new(
            "VLPLabc123",
            "Road, trip \"mix\"",
            Some("By you"),
            vec![
                track("bbbbbbbbbbb", "First", &["X", "Y"]),
                track("bbbbbbbbbbb", "First again", &["X"]),
            ],
        )],
        saved_albums: vec![ExportEntry::album(
            "MPREb_1",
            "Some Album",
            Some("Artist \u{2022} 2013"),
        )],
        subscribed_artists: vec![ExportEntry::artist(
            "UC123",
            "Some Artist",
            Some("1M subscribers"),
        )],
        problems: Vec::new(),
    }
}

#[test]
fn json_has_a_fixed_header_and_every_section() {
    let json: serde_json::Value = serde_json::from_str(&render_json(&sample())).unwrap();
    assert_eq!(json["format"], "apollo-library-export");
    assert_eq!(json["version"], 1);
    assert_eq!(json["exported_at"], "2025-10-04T08:12:33Z");
    assert_eq!(json["liked_songs"][0]["title"], "Liked One");
    assert_eq!(
        json["playlists"][0]["id"], "PLabc123",
        "the bare id, without the VL prefix"
    );
    assert_eq!(
        json["playlists"][0]["url"],
        "https://music.youtube.com/playlist?list=PLabc123"
    );
    assert_eq!(
        json["playlists"][0]["tracks"].as_array().unwrap().len(),
        2,
        "a repeated song stays repeated"
    );
    assert_eq!(json["playlists"][0]["tracks"][0]["artists"][1], "Y");
    assert_eq!(
        json["saved_albums"][0]["url"],
        "https://music.youtube.com/browse/MPREb_1"
    );
    assert_eq!(
        json["subscribed_artists"][0]["url"],
        "https://music.youtube.com/channel/UC123"
    );
    assert_eq!(json["problems"].as_array().unwrap().len(), 0);
}

#[test]
fn csv_is_one_table_with_a_section_column() {
    let csv = render_csv(&sample());
    assert!(
        csv.starts_with('\u{feff}'),
        "a byte-order mark so Excel reads the accents"
    );
    let lines: Vec<&str> = csv
        .trim_start_matches('\u{feff}')
        .split("\r\n")
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(lines[0], CSV_HEADER.join(","));
    assert_eq!(lines.len(), 1 + 1 + 2 + 1 + 1);
    assert!(lines[1].starts_with(
        "liked_song,Liked Music,LM,1,aaaaaaaaaaa,Liked One,Artist A,Album,215,false,,"
    ));
    // The playlist's title has a comma and quotes: quoted, quotes doubled.
    assert!(lines[2].starts_with(
        "playlist_song,\"Road, trip \"\"mix\"\"\",PLabc123,1,bbbbbbbbbbb,First,X; Y,"
    ));
    assert!(lines[3].contains(",2,bbbbbbbbbbb,First again,"));
    assert!(lines[4].starts_with("saved_album,,,,,Some Album,,,,,"));
    assert!(lines[5].starts_with("subscribed_artist,,,,,Some Artist,,,,,1M subscribers,"));
}

#[test]
fn csv_cells_that_would_run_as_formulas_are_neutralised() {
    for (raw, shown) in [
        ("=SUM(A1)", "'=SUM(A1)"),
        ("+1", "'+1"),
        ("-Heart", "'-Heart"),
        ("@home", "'@home"),
        ("plain", "plain"),
    ] {
        assert_eq!(csv_cell(raw), shown);
    }
    assert_eq!(csv_cell("a\nb"), "\"a\nb\"");
}

#[test]
fn problems_are_written_in_both_formats() {
    let mut export = sample();
    export.problems.push(Problem {
        what: "Playlist \"Gym\"".into(),
        reason: "Connection problem".into(),
    });
    assert!(!export.is_complete());
    assert!(render_json(&export).contains("Connection problem"));
    assert!(render_csv(&export)
        .contains("problem,,,,,\"Playlist \"\"Gym\"\"\",,,,,Connection problem,"));
}

#[test]
fn the_format_setting_round_trips_and_defaults_to_json() {
    for f in ExportFormat::ALL {
        assert_eq!(ExportFormat::from_stored(f.stored()), f);
    }
    assert_eq!(ExportFormat::from_stored(""), ExportFormat::Json);
    assert_eq!(ExportFormat::from_stored("xml"), ExportFormat::Json);
    assert_eq!(ExportFormat::default(), ExportFormat::Json);
}

#[test]
fn the_suggested_name_carries_the_date_and_extension() {
    assert_eq!(
        suggested_name(ExportFormat::Csv, 1_759_565_553),
        "apollo-library-2025-10-04.csv"
    );
}
