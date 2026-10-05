//! What a paste into the Crate means: the pure half of "Paste to add", tested without a window.
//! The add model (`crate_add`) runs the lookups and puts what is found in the Crate.
//!
//! The pasted text is read line by line, at most [`MAX_LINES`] of them. A line is one of:
//!
//! * a **link**: a YouTube Music or YouTube link anywhere in the line (a song, a playlist or an
//!   album; `si=` tracking and a leading "1." or "- " do not matter). A link that cannot be
//!   opened is [`Kind::BadLink`];
//! * a **query**: anything else. "Artist - Title", "Title by Artist", "Artist – Title" and a bare
//!   "Title Artist" all become one search; list numbers, bullets, quotes and noise such as
//!   "(Official Video)" are dropped first.
//!
//! A search answers with songs; [`best_match`] takes the first that shares enough words with the
//! line (and none when nothing does, so a line is never added on a wild guess).

use std::collections::HashSet;

use apollo_domain::links::ExternalLink;
use apollo_provider::ListedTrack;

use crate::search::as_link;

/// Lines of one paste that are read; the rest are left out and the paste says so.
pub const MAX_LINES: usize = 50;

/// The longest search a line makes, in characters.
const MAX_QUERY_CHARS: usize = 120;

/// At least this share of a line's words must be in a result for it to count as the line's song.
const MIN_COVERAGE: f32 = 0.5;

/// What one line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A link that opens something.
    Link(ExternalLink),
    /// A link of the right place that is not a song, playlist or album we can open, or a link
    /// to somewhere else altogether.
    BadLink,
    /// Words to search for.
    Query(Query),
}

/// A line of words, ready to search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// What the search is asked for: the line without its separator and noise.
    pub text: String,
    /// The part before a dash ("Artist - Title"), or after "by" ("Title by Artist").
    pub artist: Option<String>,
    pub title: Option<String>,
}

/// One non-blank line of a paste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// What was pasted, cleaned of list numbers and quotes (what the review shows).
    pub raw: String,
    pub kind: Kind,
}

/// A paste read line by line.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Paste {
    pub lines: Vec<Line>,
    /// Lines past [`MAX_LINES`] that were left out.
    pub left_out: usize,
}

impl Paste {
    /// More than one line: such a paste is reviewed before anything is added.
    pub fn is_multi(&self) -> bool {
        self.lines.len() > 1
    }
}

/// Read `text` (a clipboard): blank lines and repeats of a line (ignoring case) are skipped.
pub fn parse(text: &str) -> Paste {
    let mut seen: HashSet<String> = HashSet::new();
    let mut paste = Paste::default();
    for raw in text.lines() {
        let Some(line) = parse_line(raw) else { continue };
        if !seen.insert(line.raw.to_lowercase()) {
            continue;
        }
        if paste.lines.len() == MAX_LINES {
            paste.left_out += 1;
            continue;
        }
        paste.lines.push(line);
    }
    paste
}

/// One line, or `None` when nothing is left of it.
pub fn parse_line(raw: &str) -> Option<Line> {
    let cleaned = strip_quotes(strip_list_marker(raw.trim())).trim().to_owned();
    if cleaned.is_empty() {
        return None;
    }
    if let Some(kind) = link_in(&cleaned) {
        return Some(Line { raw: cleaned, kind });
    }
    let query = query_of(&cleaned)?;
    Some(Line { raw: cleaned, kind: Kind::Query(query) })
}

/// "1. ", "02) ", "- ", "• ": a list marker at the start of a line.
fn strip_list_marker(line: &str) -> &str {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &line[digits..];
        for mark in [". ", ") ", "] ", ": ", " - "] {
            if let Some(after) = rest.strip_prefix(mark) {
                return after.trim_start();
            }
        }
        // "1.Title" is a title that starts with a number only when a space follows; "3." alone
        // is a marker with nothing after it.
        if matches!(rest, "." | ")") {
            return "";
        }
    }
    for bullet in ["- ", "* ", "\u{2022} ", "\u{b7} ", "\u{2013} ", "\u{2014} ", "> "] {
        if let Some(after) = line.strip_prefix(bullet) {
            return after.trim_start();
        }
    }
    line
}

/// A line wrapped in quotes ("Title" by Artist stays as it is; only a whole-line wrap goes).
fn strip_quotes(line: &str) -> &str {
    for (open, close) in [('"', '"'), ('\u{201c}', '\u{201d}'), ('\'', '\'')] {
        if let Some(inner) = line.strip_prefix(open).and_then(|l| l.strip_suffix(close)) {
            if !inner.contains(open) {
                return inner.trim();
            }
        }
    }
    line
}

/// The link in a line: the first word that is one of ours. A word that is a link to somewhere
/// else (or one that does not open) makes the line a bad link, unless a good one is also there.
fn link_in(line: &str) -> Option<Kind> {
    let mut bad = false;
    for word in line.split_whitespace() {
        let word = word.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | ',' | '"' | '\'' | '!' | '?' | ';' | '.'));
        match as_link(word) {
            Some(Ok(link)) => return Some(Kind::Link(link)),
            Some(Err(_)) => bad = true,
            None if word.contains("://") => bad = true,
            None => {}
        }
    }
    bad.then_some(Kind::BadLink)
}

/// Words in brackets that are about the upload, not the song.
const NOISE: &[&str] = &[
    "official video",
    "official music video",
    "official audio",
    "official lyric video",
    "official visualizer",
    "music video",
    "lyric video",
    "lyrics",
    "lyric",
    "audio",
    "video",
    "visualizer",
    "hd",
    "hq",
    "4k",
    "full song",
    "full video",
];

/// The line as a search: noise in brackets and a trailing " - Topic" go, then it is split on
/// the first " - " (or en / em dash), else on the last " by ".
fn query_of(line: &str) -> Option<Query> {
    let mut text = strip_noise(line);
    if let Some(head) = text.strip_suffix(" - Topic") {
        text = head.trim_end().to_owned();
    }
    let text = collapse(&text);
    // Nothing but punctuation is no song.
    if words(&text).is_empty() {
        return None;
    }
    let (artist, title) = split(&text);
    let search = match (&artist, &title) {
        (Some(a), Some(t)) => format!("{a} {t}"),
        _ => text.clone(),
    };
    let search: String = search.chars().take(MAX_QUERY_CHARS).collect();
    Some(Query { text: collapse(&search), artist, title })
}

/// `(artist, title)` of "Artist - Title" and "Title by Artist"; `(None, None)` for plain words.
fn split(text: &str) -> (Option<String>, Option<String>) {
    for dash in [" - ", " \u{2013} ", " \u{2014} "] {
        if let Some((artist, title)) = text.split_once(dash) {
            let (artist, title) = (artist.trim(), title.trim());
            if !artist.is_empty() && !title.is_empty() {
                return (Some(artist.to_owned()), Some(title.to_owned()));
            }
        }
    }
    // The last " by " (ignoring case), so "Stand By Me by Ben E. King" splits after "Me".
    let lower = text.to_lowercase();
    if let Some(at) = lower.rfind(" by ") {
        // Lowercasing can change a text's byte length for a few scripts; only split when the
        // two agree on where the word is.
        if lower.len() == text.len() && text.is_char_boundary(at) && text.is_char_boundary(at + 4) {
            let (title, artist) = (text[..at].trim(), text[at + 4..].trim());
            if !artist.is_empty() && !title.is_empty() {
                return (Some(artist.to_owned()), Some(title.to_owned()));
            }
        }
    }
    (None, None)
}

/// The text without bracketed groups that are about the upload ("(Official Video)").
fn strip_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(len) = rest[open + 1..].find(close_char) else { break };
        let close = open + 1 + len;
        let inner = words(&rest[open + 1..close]).join(" ");
        if NOISE.contains(&inner.as_str()) {
            out.push_str(rest[..open].trim_end());
            out.push(' ');
        } else {
            out.push_str(&rest[..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lowercase alphanumeric runs: the words of a text, as compared.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// How much of the line a song covers: the share of the line's words that are in the song's
/// title or artists.
pub fn coverage(query: &Query, found: &ListedTrack) -> f32 {
    let wanted: HashSet<String> = words(&query.text).into_iter().collect();
    if wanted.is_empty() {
        return 0.0;
    }
    let mut have: HashSet<String> = words(&found.track.title).into_iter().collect();
    for artist in &found.track.artists {
        have.extend(words(&artist.name));
    }
    wanted.iter().filter(|w| have.contains(*w)).count() as f32 / wanted.len() as f32
}

/// The song a line means among a search's answers: the one that covers the most of the line (the
/// service's own order breaks a tie), provided it covers at least half. `None`: no match.
pub fn best_match(query: &Query, found: &[ListedTrack]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (at, song) in found.iter().enumerate() {
        let c = coverage(query, song);
        if c >= MIN_COVERAGE && best.is_none_or(|(_, b)| c > b) {
            best = Some((at, c));
        }
    }
    best.map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use apollo_domain::model::{ArtistRef, Track, VideoId};

    use super::*;

    fn query(line: &str) -> Query {
        match parse_line(line).expect("a line").kind {
            Kind::Query(q) => q,
            other => panic!("not a query: {other:?}"),
        }
    }

    fn song(id: &str, title: &str, artist: &str) -> ListedTrack {
        ListedTrack {
            track: Track {
                video_id: VideoId::new(id),
                title: title.into(),
                artists: vec![ArtistRef { name: artist.into(), id: None }],
                album: None,
                duration: Some(Duration::from_secs(200)),
            },
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked: None,
        }
    }

    #[test]
    fn artist_dash_title_splits_on_the_first_dash() {
        let q = query("Daft Punk - Get Lucky");
        assert_eq!(q.artist.as_deref(), Some("Daft Punk"));
        assert_eq!(q.title.as_deref(), Some("Get Lucky"));
        assert_eq!(q.text, "Daft Punk Get Lucky");
        // More dashes belong to the title.
        let q = query("Artist - Song - Remastered 2011");
        assert_eq!(q.artist.as_deref(), Some("Artist"));
        assert_eq!(q.title.as_deref(), Some("Song - Remastered 2011"));
    }

    #[test]
    fn en_and_em_dashes_split_too() {
        let q = query("Artist \u{2013} Title");
        assert_eq!((q.artist.as_deref(), q.title.as_deref()), (Some("Artist"), Some("Title")));
        let q = query("Artist \u{2014} Title");
        assert_eq!((q.artist.as_deref(), q.title.as_deref()), (Some("Artist"), Some("Title")));
    }

    #[test]
    fn title_by_artist_splits_on_the_last_by() {
        let q = query("Get Lucky by Daft Punk");
        assert_eq!(q.title.as_deref(), Some("Get Lucky"));
        assert_eq!(q.artist.as_deref(), Some("Daft Punk"));
        let q = query("Stand By Me by Ben E. King");
        assert_eq!(q.title.as_deref(), Some("Stand By Me"));
        assert_eq!(q.artist.as_deref(), Some("Ben E. King"));
        let q = query("Song BY Someone");
        assert_eq!(q.artist.as_deref(), Some("Someone"));
    }

    #[test]
    fn a_dash_wins_over_by_and_plain_words_stay_whole() {
        let q = query("Artist - Stand By Me");
        assert_eq!(q.artist.as_deref(), Some("Artist"));
        assert_eq!(q.title.as_deref(), Some("Stand By Me"));
        let q = query("Bohemian Rhapsody Queen");
        assert_eq!((q.artist, q.title), (None, None));
        assert_eq!(q.text, "Bohemian Rhapsody Queen");
        // A hyphen with no spaces is part of a name.
        let q = query("Jay-Z - 99 Problems");
        assert_eq!(q.artist.as_deref(), Some("Jay-Z"));
        let q = query("Jay-Z 99 Problems");
        assert_eq!(q.artist, None);
    }

    #[test]
    fn list_numbers_bullets_and_quotes_are_dropped() {
        for line in ["1. Artist - Title", "02) Artist - Title", "- Artist - Title", "\u{2022} Artist - Title", "\"Artist - Title\"", "  3: Artist - Title  "] {
            let parsed = parse_line(line).expect(line);
            assert_eq!(parsed.raw, "Artist - Title", "{line}");
        }
        // A title that starts with a number keeps it.
        assert_eq!(parse_line("99 Problems").unwrap().raw, "99 Problems");
        assert_eq!(parse_line("1999 - Prince").unwrap().raw, "1999 - Prince");
        // A marker with nothing after it is no line.
        assert_eq!(parse_line("3."), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("-"), None);
        assert_eq!(parse_line("--- ..."), None);
    }

    #[test]
    fn upload_noise_in_brackets_and_topic_are_dropped_other_brackets_kept() {
        let q = query("Artist - Title (Official Video)");
        assert_eq!(q.text, "Artist Title");
        let q = query("Artist - Title [Official Audio] (Lyrics)");
        assert_eq!(q.text, "Artist Title");
        let q = query("Artist - Title (feat. Guest)");
        assert_eq!(q.text, "Artist Title (feat. Guest)");
        let q = query("Artist - Title (Live)");
        assert_eq!(q.text, "Artist Title (Live)");
        let q = query("Artist - Topic");
        assert_eq!(q.text, "Artist");
    }

    #[test]
    fn links_are_found_anywhere_in_a_line() {
        let song = "https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc";
        match parse_line(song).unwrap().kind {
            Kind::Link(ExternalLink::Track { video, .. }) => assert_eq!(video.as_str(), "dQw4w9WgXcQ"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_line("1. Listen to this: https://youtu.be/dQw4w9WgXcQ!").unwrap().kind,
            Kind::Link(ExternalLink::Track { .. })
        ));
        assert!(matches!(
            parse_line("music.youtube.com/playlist?list=PLabcdefgh").unwrap().kind,
            Kind::Link(ExternalLink::Playlist(_))
        ));
        assert!(matches!(
            parse_line("<https://music.youtube.com/browse/MPREb_abcdefgh>").unwrap().kind,
            Kind::Link(ExternalLink::Album(_))
        ));
    }

    #[test]
    fn a_link_to_somewhere_else_is_a_bad_link_not_a_search() {
        assert_eq!(parse_line("https://open.spotify.com/track/abc").unwrap().kind, Kind::BadLink);
        assert_eq!(parse_line("https://music.youtube.com/").unwrap().kind, Kind::BadLink);
        // A good link beside it wins.
        assert!(matches!(
            parse_line("https://open.spotify.com/x https://youtu.be/dQw4w9WgXcQ").unwrap().kind,
            Kind::Link(_)
        ));
    }

    #[test]
    fn a_paste_skips_blank_lines_and_repeats_and_stops_at_fifty() {
        let text = "A - One\n\n   \na - one\nB - Two\r\nC - Three";
        let paste = parse(text);
        let raws: Vec<_> = paste.lines.iter().map(|l| l.raw.as_str()).collect();
        assert_eq!(raws, ["A - One", "B - Two", "C - Three"]);
        assert!(paste.is_multi());
        assert_eq!(paste.left_out, 0);

        let many: String = (0..60).map(|i| format!("Artist {i} - Song {i}\n")).collect();
        let paste = parse(&many);
        assert_eq!(paste.lines.len(), MAX_LINES);
        assert_eq!(paste.left_out, 10);

        assert!(!parse("Only - One").is_multi());
        assert!(parse("\n\n").lines.is_empty());
    }

    #[test]
    fn long_lines_are_cut_to_a_search_length() {
        let q = query(&"word ".repeat(100));
        assert!(q.text.chars().count() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn the_match_is_the_first_result_that_covers_the_line() {
        let q = query("Daft Punk - Get Lucky");
        let found = vec![
            song("a", "Get Lucky (Radio Edit)", "Daft Punk"),
            song("b", "Get Lucky", "Daft Punk"),
        ];
        assert_eq!(best_match(&q, &found), Some(0), "a tie goes to the service's order");
        // A fuller cover beats an earlier partial one.
        let found = vec![song("x", "Lucky", "Someone"), song("y", "Get Lucky", "Daft Punk")];
        assert_eq!(best_match(&q, &found), Some(1));
    }

    #[test]
    fn nothing_that_shares_less_than_half_the_words_is_a_match() {
        let q = query("Daft Punk - Get Lucky");
        assert_eq!(best_match(&q, &[]), None);
        assert_eq!(best_match(&q, &[song("a", "Unrelated Song", "Other Artist")]), None);
        // "Title by Artist" and plain order find the same song.
        let q = query("Get Lucky by Daft Punk");
        assert_eq!(best_match(&q, &[song("a", "Get Lucky", "Daft Punk")]), Some(0));
        let q = query("get lucky daft punk");
        assert_eq!(best_match(&q, &[song("a", "Get Lucky", "Daft Punk")]), Some(0));
    }

    #[test]
    fn words_are_lowercase_runs_of_letters_and_digits() {
        assert_eq!(words("Don't Stop (Café) 99!"), ["don", "t", "stop", "café", "99"]);
        assert!(words("... --").is_empty());
    }
}
