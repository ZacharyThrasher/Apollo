//! Swapping a Crate song for its explicit (or clean) version: the matching rules, pure and
//! tested without a window. The Crate model (`crate_list`) runs the searches and applies what is
//! picked here.
//!
//! A song is looked up by its title and first artist (a songs-only search). A result is the same
//! song in the other version when all of these hold:
//!
//! * it carries the flag wanted (the service's "E" badge for explicit, none for clean);
//! * its title is the same after [`normalize_title`]: case, punctuation and version words such as
//!   "(Clean)", "[Explicit]" or "- Radio Edit" do not count, but "(Live)" or "(Remix)" do;
//! * it credits the song's first artist (by normalized name);
//! * its length is within [`DURATION_SLACK`] of the song's. A length that is not known on either
//!   side is no match: nothing is swapped on a guess.
//!
//! Of several that qualify the nearest in length wins, then the one the service ranked first.

use std::collections::HashSet;
use std::time::Duration;

use apollo_domain::model::Track;
use apollo_provider::ListedTrack;

/// How far apart two versions' lengths may be.
pub const DURATION_SLACK: Duration = Duration::from_secs(5);

/// How many searches run at once. Low on purpose: a long Crate must not hammer the service.
pub const CONCURRENCY: usize = 2;

/// Which version a swap looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Explicit,
    Clean,
}

impl Want {
    /// The explicit flag a song must carry to be what is wanted.
    pub fn explicit(self) -> bool {
        self == Want::Explicit
    }

    /// "explicit" or "clean", for the pill's words.
    pub fn word(self) -> &'static str {
        match self {
            Want::Explicit => "explicit",
            Want::Clean => "clean",
        }
    }
}

/// What the tip pill says while a batch runs: "Finding explicit versions… 3/15".
pub fn progress_text(want: Want, done: usize, total: usize) -> String {
    format!("Finding {} versions\u{2026} {done}/{total}", want.word())
}

/// What the tip pill says when a batch is over.
pub fn finished_text(
    want: Want,
    swapped: usize,
    total: usize,
    already: usize,
    unreachable: bool,
) -> String {
    if total == 0 {
        "Nothing to swap".to_owned()
    } else if unreachable {
        "Couldn't reach YouTube Music".to_owned()
    } else if swapped == 0 && already == total {
        format!("Already {}", want.word())
    } else if swapped == 0 {
        format!("No {} versions found", want.word())
    } else {
        format!("Swapped {swapped} of {total}")
    }
}

/// Words that name a version of a song rather than a different song (lowercase, letters and
/// spaces only, as [`words`] leaves them).
const VERSION_WORDS: &[&str] = &[
    "clean",
    "clean version",
    "explicit",
    "explicit version",
    "radio edit",
    "radio version",
    "edited",
    "edited version",
    "censored",
    "censored version",
    "uncensored",
    "dirty",
    "dirty version",
    "amended",
    "amended version",
    "album version",
];

/// Lowercase, every run of non-alphanumeric characters one space, trimmed.
fn words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            gap = false;
        } else if !gap {
            out.push(' ');
            gap = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn is_version_word(text: &str) -> bool {
    VERSION_WORDS.contains(&words(text).as_str())
}

/// The title without its version words: "Song (feat. X) (Clean)" is "Song (feat. X)". Only a
/// bracketed group or a last " - " part that is wholly a version word goes; the case and the
/// rest are kept (this is also what the search is asked for).
pub fn strip_versions(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(len) = rest[open + 1..].find(close_char) else {
            break;
        };
        let close = open + 1 + len;
        if is_version_word(&rest[open + 1..close]) {
            out.push_str(rest[..open].trim_end());
            out.push(' ');
        } else {
            out.push_str(&rest[..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    let mut out = out.trim().to_owned();
    if let Some((head, tail)) = out.rsplit_once(" - ") {
        if is_version_word(tail) {
            out = head.trim_end().to_owned();
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title as compared: version words, case and punctuation do not count.
pub fn normalize_title(title: &str) -> String {
    words(&strip_versions(title))
}

/// What a song is searched for: its title without version words and its first artist.
pub fn query(track: &Track) -> String {
    let title = strip_versions(&track.title);
    match track.artists.first() {
        Some(artist) => format!("{title} {}", artist.name.trim()),
        None => title,
    }
}

/// Whether `found` is the same recording as `current`, apart from the version.
pub fn same_song(current: &Track, found: &Track) -> bool {
    if normalize_title(&current.title) != normalize_title(&found.title) {
        return false;
    }
    // The first artist must be credited on both; a song with no artist cannot be confirmed.
    let Some(first) = current.artists.first().map(|a| words(&a.name)) else {
        return false;
    };
    if first.is_empty() || !found.artists.iter().any(|a| words(&a.name) == first) {
        return false;
    }
    match length_gap(current.duration, found.duration) {
        Some(gap) => gap <= DURATION_SLACK,
        None => false,
    }
}

fn length_gap(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    let (a, b) = (a?, b?);
    Some(if a > b { a - b } else { b - a })
}

/// What a search found for one song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The result at this index of the list is the other version.
    Swap(usize),
    /// The song itself came back with the wanted flag: it is that version already (and the
    /// Crate's flag for it can be corrected).
    Already,
    /// No confident match: the song stays.
    None,
}

/// Choose from the songs a search found. `taken` says a video the Crate already holds (a swap
/// must never make it hold a song twice).
pub fn pick(
    current: &Track,
    want: Want,
    found: &[ListedTrack],
    taken: impl Fn(&str) -> bool,
) -> Pick {
    let mut best: Option<(usize, Duration)> = None;
    for (at, candidate) in found.iter().enumerate() {
        if candidate.explicit != want.explicit() {
            continue;
        }
        let id = candidate.track.video_id.as_str();
        if id == current.video_id.as_str() {
            return Pick::Already;
        }
        if taken(id) || !same_song(current, &candidate.track) {
            continue;
        }
        let gap = length_gap(current.duration, candidate.track.duration).unwrap_or_default();
        if best.is_none_or(|(_, nearest)| gap < nearest) {
            best = Some((at, gap));
        }
    }
    best.map_or(Pick::None, |(at, _)| Pick::Swap(at))
}

/// The songs of a search without the ones that are music videos: a swap looks for the audio
/// track, not a video of it.
pub fn audio_only(results: &apollo_provider::SearchResults) -> Vec<ListedTrack> {
    let videos: &std::collections::BTreeSet<String> = &results.video_ids;
    results
        .songs
        .iter()
        .filter(|s| !videos.contains(s.track.video_id.as_str()))
        .cloned()
        .collect()
}

/// The video ids of `rows` that a batch should look at: songs not already the wanted version,
/// limited to `only` when it is given (a selection). In the Crate's order.
pub fn targets<'a>(
    rows: impl Iterator<Item = (&'a str, bool)>,
    want: Want,
    only: Option<&[String]>,
) -> Vec<String> {
    let only: Option<HashSet<&str>> = only.map(|ids| ids.iter().map(String::as_str).collect());
    rows.filter(|(id, explicit)| {
        *explicit != want.explicit() && only.as_ref().is_none_or(|set| set.contains(id))
    })
    .map(|(id, _)| id.to_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use apollo_domain::model::ArtistRef;

    use super::*;

    fn song(id: &str, title: &str, artist: &str, secs: u64) -> Track {
        Track {
            video_id: apollo_domain::model::VideoId::new(id),
            title: title.into(),
            artists: vec![ArtistRef { name: artist.into(), id: None }],
            album: None,
            duration: Some(Duration::from_secs(secs)),
        }
    }

    fn listed(id: &str, title: &str, artist: &str, secs: u64, explicit: bool) -> ListedTrack {
        ListedTrack {
            track: song(id, title, artist, secs),
            occurrence: None,
            thumbnail: None,
            explicit,
            liked: None,
        }
    }

    #[test]
    fn version_words_are_stripped_and_other_brackets_are_kept() {
        assert_eq!(strip_versions("Song (Clean)"), "Song");
        assert_eq!(strip_versions("Song [Explicit]"), "Song");
        assert_eq!(strip_versions("Song - Radio Edit"), "Song");
        assert_eq!(strip_versions("Song (feat. X) (Clean Version)"), "Song (feat. X)");
        assert_eq!(strip_versions("Song (Clean) (feat. X)"), "Song (feat. X)");
        assert_eq!(strip_versions("Song (Live)"), "Song (Live)");
        assert_eq!(strip_versions("Song (Remix)"), "Song (Remix)");
        assert_eq!(strip_versions("Song - Remastered 2011"), "Song - Remastered 2011");
        assert_eq!(strip_versions("Song (Unclosed"), "Song (Unclosed");
        assert_eq!(strip_versions("  Plain  Title "), "Plain Title");
    }

    #[test]
    fn titles_compare_without_case_punctuation_or_version_words() {
        assert_eq!(normalize_title("Don't Stop (Clean)"), normalize_title("DON\u{2019}T STOP"));
        assert_eq!(normalize_title("Don't Stop"), "don t stop");
        assert_eq!(normalize_title("Song (Radio Edit)"), normalize_title("Song"));
        assert_ne!(normalize_title("Song (Live)"), normalize_title("Song"));
        assert_ne!(normalize_title("Song (Remix)"), normalize_title("Song"));
        assert_eq!(normalize_title("Caf\u{e9} (Clean)"), "caf\u{e9}");
    }

    #[test]
    fn the_search_is_for_the_title_without_version_words_and_the_first_artist() {
        let mut t = song("a", "Song (Clean)", "Artist", 200);
        t.artists.push(ArtistRef { name: "Other".into(), id: None });
        assert_eq!(query(&t), "Song Artist");
        t.artists.clear();
        assert_eq!(query(&t), "Song");
    }

    #[test]
    fn the_same_song_needs_title_artist_and_length() {
        let clean = song("a", "Song (Clean)", "Artist", 200);
        assert!(same_song(&clean, &song("b", "Song", "artist", 203)));
        assert!(same_song(&clean, &song("b", "Song", "Artist", 195)), "5 s either way");
        assert!(!same_song(&clean, &song("b", "Song", "Artist", 206)), "too far apart");
        assert!(!same_song(&clean, &song("b", "Song 2", "Artist", 200)));
        assert!(!same_song(&clean, &song("b", "Song", "Someone Else", 200)));
        // A credited artist anywhere in the found song's list counts.
        let mut feat = song("b", "Song", "Guest", 200);
        feat.artists.push(ArtistRef { name: "Artist".into(), id: None });
        assert!(same_song(&clean, &feat));
        // Nothing is swapped on a guess.
        let mut no_length = song("b", "Song", "Artist", 200);
        no_length.duration = None;
        assert!(!same_song(&clean, &no_length));
        let mut unknown = clean.clone();
        unknown.duration = None;
        assert!(!same_song(&unknown, &song("b", "Song", "Artist", 200)));
        let mut anon = clean.clone();
        anon.artists.clear();
        assert!(!same_song(&anon, &song("b", "Song", "Artist", 200)));
    }

    #[test]
    fn the_explicit_result_wins_and_the_nearest_in_length_of_several() {
        let current = song("clean", "Song (Clean)", "Artist", 200);
        let found = vec![
            listed("same-clean", "Song", "Artist", 200, false),
            listed("far", "Song", "Artist", 204, true),
            listed("near", "Song", "Artist", 201, true),
            listed("also-near", "Song", "Artist", 201, true),
            listed("other", "Different", "Artist", 200, true),
        ];
        assert_eq!(pick(&current, Want::Explicit, &found, |_| false), Pick::Swap(2));
        // The same results the other way: the clean one.
        let current = song("dirty", "Song", "Artist", 200);
        assert_eq!(pick(&current, Want::Clean, &found, |_| false), Pick::Swap(0));
    }

    #[test]
    fn no_match_leaves_the_song_and_a_taken_video_is_never_picked() {
        let current = song("clean", "Song", "Artist", 200);
        assert_eq!(pick(&current, Want::Explicit, &[], |_| false), Pick::None);
        let only_clean = vec![listed("x", "Song", "Artist", 200, false)];
        assert_eq!(pick(&current, Want::Explicit, &only_clean, |_| false), Pick::None);
        let found = vec![
            listed("held", "Song", "Artist", 200, true),
            listed("free", "Song", "Artist", 202, true),
        ];
        assert_eq!(pick(&current, Want::Explicit, &found, |id| id == "held"), Pick::Swap(1));
        assert_eq!(pick(&current, Want::Explicit, &found[..1], |id| id == "held"), Pick::None);
    }

    #[test]
    fn the_song_itself_coming_back_with_the_wanted_flag_is_already_that_version() {
        let current = song("same", "Song", "Artist", 200);
        let found = vec![
            listed("other", "Song", "Artist", 200, true),
            listed("same", "Song", "Artist", 200, true),
        ];
        assert_eq!(pick(&current, Want::Explicit, &found, |_| false), Pick::Already);
        // With the other flag it is just a result that does not count.
        let clean_copy = vec![listed("same", "Song", "Artist", 200, false)];
        assert_eq!(pick(&current, Want::Explicit, &clean_copy, |_| false), Pick::None);
    }

    #[test]
    fn music_videos_are_not_what_a_swap_looks_for() {
        let mut results = apollo_provider::SearchResults::default();
        results.songs = vec![
            listed("video", "Song", "Artist", 200, true),
            listed("audio", "Song", "Artist", 200, true),
        ];
        results.video_ids.insert("video".into());
        let songs = audio_only(&results);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].track.video_id.as_str(), "audio");
    }

    #[test]
    fn a_batch_looks_at_the_songs_that_are_not_the_wanted_version_yet() {
        let rows = [("a", false), ("b", true), ("c", false), ("d", false)];
        let ids = |want, only: Option<&[String]>| targets(rows.iter().copied(), want, only);
        assert_eq!(ids(Want::Explicit, None), ["a", "c", "d"]);
        assert_eq!(ids(Want::Clean, None), ["b"]);
        let picked = ["c".to_owned(), "b".to_owned()];
        assert_eq!(ids(Want::Explicit, Some(&picked)), ["c"], "the selection only");
        assert!(ids(Want::Explicit, Some(&[])).is_empty());
    }

    #[test]
    fn the_pill_words() {
        assert_eq!(progress_text(Want::Explicit, 3, 15), "Finding explicit versions\u{2026} 3/15");
        assert_eq!(progress_text(Want::Clean, 0, 4), "Finding clean versions\u{2026} 0/4");
        assert_eq!(finished_text(Want::Explicit, 12, 15, 0, false), "Swapped 12 of 15");
        assert_eq!(finished_text(Want::Explicit, 0, 15, 0, false), "No explicit versions found");
        assert_eq!(finished_text(Want::Explicit, 0, 15, 4, false), "No explicit versions found");
        assert_eq!(finished_text(Want::Explicit, 0, 3, 3, false), "Already explicit");
        assert_eq!(finished_text(Want::Clean, 0, 3, 3, false), "Already clean");
        assert_eq!(finished_text(Want::Clean, 0, 3, 0, false), "No clean versions found");
        assert_eq!(finished_text(Want::Explicit, 0, 3, 0, true), "Couldn't reach YouTube Music");
        assert_eq!(finished_text(Want::Explicit, 0, 0, 0, false), "Nothing to swap");
    }
}
