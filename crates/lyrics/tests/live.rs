//! Live checks against lrclib.net. Ignored by default (network); run with
//! `cargo test -p apollo-lyrics --test live -- --ignored --nocapture`.

use apollo_lyrics::{Lrclib, Lyrics, LyricsQuery, LyricsSource};

fn query(title: &str, artist: &str, album: Option<&str>, secs: u32) -> LyricsQuery {
    LyricsQuery {
        video_id: "live".into(),
        title: title.into(),
        artist: artist.into(),
        first_artist: artist.split(", ").next().unwrap().into(),
        album: album.map(str::to_owned),
        duration_secs: Some(secs),
    }
}

fn kind(l: &Option<Lyrics>) -> &'static str {
    match l {
        Some(Lyrics::Synced(_)) => "synced",
        Some(Lyrics::Plain(_)) => "plain",
        None => "none",
    }
}

#[test]
#[ignore]
fn titles_as_youtube_music_lists_them_find_the_right_lyrics() {
    let lrclib = Lrclib::new();
    // (title, artist, album, seconds, first line expected somewhere in the lyrics)
    let cases = [
        (
            "Blinding Lights",
            "The Weeknd",
            Some("After Hours"),
            200,
            "Yeah",
        ),
        (
            "Starboy (feat. Daft Punk)",
            "The Weeknd, Daft Punk",
            Some("Starboy"),
            230,
            "house",
        ),
        (
            "Halo - Remastered",
            "Beyoncé",
            Some("I AM... SASHA FIERCE"),
            261,
            "Remember those walls",
        ),
        (
            "Bohemian Rhapsody - Remastered 2011",
            "Queen",
            Some("Greatest Hits (Deluxe)"),
            355,
            "real life",
        ),
        (
            "Mr. Brightside",
            "The Killers",
            Some("Hot Fuss"),
            223,
            "out of my cage",
        ),
        ("Hoppípolla", "Sigur Rós", Some("Takk..."), 268, ""),
    ];
    for (title, artist, album, secs, expect) in cases {
        let found = lrclib.fetch(&query(title, artist, album, secs)).unwrap();
        let text = match &found {
            Some(Lyrics::Synced(lines)) => lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Some(Lyrics::Plain(p)) => p.clone(),
            None => String::new(),
        };
        println!("{title} / {artist}: {}", kind(&found));
        assert!(found.is_some(), "{title}: nothing found");
        assert!(
            text.to_lowercase().contains(&expect.to_lowercase()),
            "{title}: wrong song? {}",
            &text[..text.len().min(120)]
        );
    }
}

#[test]
#[ignore]
fn songs_lrclib_does_not_have_stay_not_found_rather_than_matching_another_song() {
    let lrclib = Lrclib::new();
    for (title, artist, secs) in [
        ("World's Greatest!", "Lil Nas X", 142),
        ("Can't Be Still", "Big K.R.I.T.", 215),
    ] {
        let found = lrclib.fetch(&query(title, artist, None, secs)).unwrap();
        println!("{title} / {artist}: {}", kind(&found));
        assert!(found.is_none(), "{title}: unexpected match");
    }
}
