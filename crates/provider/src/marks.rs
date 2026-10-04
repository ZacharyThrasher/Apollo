//! What the service said about songs and the library, kept for the moment a menu asks.
//!
//! Adding a song to the library is a `feedback` write whose only handle is an opaque token that
//! comes with the song's own row (see `innertube::LibraryToggle`). Rows are read in many places
//! (playlists, search, history, the Library's Songs list), so every read notes the tokens it saw
//! here, and the menu and the write look them up by video id. Memory only: a token belongs to the
//! session that fetched it, so nothing is persisted and a new account starts empty.

use std::collections::HashMap;
use std::sync::Mutex;

use innertube::{BrowseItem, LibraryToggle, SongItem};

/// The most songs remembered. A library past this is not cut, the notes just start over.
const CAPACITY: usize = 50_000;

#[derive(Default)]
pub struct LibraryMarks {
    songs: Mutex<HashMap<String, LibraryToggle>>,
}

impl LibraryMarks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, LibraryToggle>> {
        self.songs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remember what a row says. A row with no such menu changes nothing: an earlier read that
    /// had one still stands.
    pub fn note(&self, video_id: &str, toggle: Option<&LibraryToggle>) {
        let Some(toggle) = toggle else { return };
        if video_id.is_empty() {
            return;
        }
        let mut songs = self.lock();
        if songs.len() >= CAPACITY && !songs.contains_key(video_id) {
            songs.clear();
        }
        songs.insert(video_id.to_owned(), toggle.clone());
    }

    pub fn note_songs(&self, songs: &[SongItem]) {
        for song in songs {
            self.note(&song.video_id, song.library.as_ref());
        }
    }

    /// Song cards (search rows, shelf songs); other cards have no library toggle of this kind.
    pub fn note_items(&self, items: &[BrowseItem]) {
        for item in items.iter().filter(|i| i.kind == "song") {
            self.note(&item.id, item.library.as_ref());
        }
    }

    /// Whether the library holds the song as the last row said, but only when the row also gave
    /// the token to change it: `None` is "nothing can be done about this song from here".
    pub fn state(&self, video_id: &str) -> Option<bool> {
        let songs = self.lock();
        let toggle = songs.get(video_id)?;
        let usable = if toggle.in_library { &toggle.remove_token } else { &toggle.add_token };
        usable.as_ref().map(|_| toggle.in_library)
    }

    /// The token that puts the song in the library (`saved`) or takes it out.
    pub fn token(&self, video_id: &str, saved: bool) -> Option<String> {
        let songs = self.lock();
        let toggle = songs.get(video_id)?;
        if saved { toggle.add_token.clone() } else { toggle.remove_token.clone() }
    }

    /// Library > Songs listed this song, so the library holds it whatever its row's menu said
    /// (a liked song's row can still offer "Save to library", and a saved one's offer only a
    /// single action: on that page the one thing on offer is the way out). Songs with no note, or
    /// no token to act on, stay as they were.
    pub fn listed(&self, video_id: &str) {
        if let Some(toggle) = self.lock().get_mut(video_id) {
            toggle.in_library = true;
            if toggle.remove_token.is_none() {
                toggle.remove_token = toggle.add_token.take();
            }
        }
    }

    /// A change landed (or was confirmed by reading the library back).
    pub fn set(&self, video_id: &str, saved: bool) {
        if let Some(toggle) = self.lock().get_mut(video_id) {
            toggle.in_library = saved;
        }
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle(in_library: bool, add: Option<&str>, remove: Option<&str>) -> LibraryToggle {
        LibraryToggle {
            in_library,
            add_token: add.map(str::to_owned),
            remove_token: remove.map(str::to_owned),
        }
    }

    #[test]
    fn a_song_is_known_only_with_the_token_for_the_change_it_could_make() {
        let marks = LibraryMarks::default();
        assert_eq!(marks.state("a"), None, "never seen");
        marks.note("a", Some(&toggle(false, Some("add"), Some("rm"))));
        assert_eq!(marks.state("a"), Some(false));
        assert_eq!(marks.token("a", true).as_deref(), Some("add"));
        assert_eq!(marks.token("a", false).as_deref(), Some("rm"));
        marks.set("a", true);
        assert_eq!(marks.state("a"), Some(true));
        // A one-way row: it can be added, and after that nothing can be done from its tokens.
        marks.note("b", Some(&toggle(false, Some("add"), None)));
        assert_eq!(marks.state("b"), Some(false));
        marks.set("b", true);
        assert_eq!(marks.state("b"), None, "no token to take it out again");
    }

    // Reported from the PC check: on Library > Songs a liked row offered "Save to library" and
    // another row offered nothing to take it out. Whatever its menu says, a song that page lists
    // is in the library, and a lone action on it is the removal.
    #[test]
    fn a_song_the_songs_page_lists_is_in_the_library_whatever_its_menu_said() {
        let marks = LibraryMarks::default();
        // A toggle that arrived "not in library": both tokens, so Remove has its token.
        marks.note("liked", Some(&toggle(false, Some("add"), Some("rm"))));
        assert_eq!(marks.state("liked"), Some(false));
        marks.listed("liked");
        assert_eq!(marks.state("liked"), Some(true));
        assert_eq!(marks.token("liked", false).as_deref(), Some("rm"));
        // A one-way "add" on a listed song becomes its way out.
        marks.note("lone", Some(&toggle(false, Some("only"), None)));
        marks.listed("lone");
        assert_eq!(marks.state("lone"), Some(true));
        assert_eq!(marks.token("lone", false).as_deref(), Some("only"));
        assert_eq!(marks.token("lone", true), None);
        // Already right: nothing changes. Never seen: nothing is invented.
        marks.note("ok", Some(&toggle(true, Some("add"), Some("rm"))));
        marks.listed("ok");
        assert_eq!(marks.state("ok"), Some(true));
        assert_eq!(marks.token("ok", false).as_deref(), Some("rm"));
        marks.listed("unseen");
        assert_eq!(marks.state("unseen"), None);
    }

    #[test]
    fn a_row_with_no_menu_does_not_forget_an_earlier_one() {
        let marks = LibraryMarks::default();
        marks.note("a", Some(&toggle(true, None, Some("rm"))));
        marks.note("a", None);
        assert_eq!(marks.state("a"), Some(true));
        marks.clear();
        assert_eq!(marks.state("a"), None);
    }

    #[test]
    fn search_cards_note_songs_only() {
        let marks = LibraryMarks::default();
        let card = |kind: &'static str, id: &str| BrowseItem {
            kind,
            id: id.into(),
            title: "T".into(),
            subtitle: None,
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: Vec::new(),
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
            library: Some(toggle(false, Some("add"), Some("rm"))),
        };
        marks.note_items(&[card("song", "s"), card("album", "al")]);
        assert_eq!(marks.state("s"), Some(false));
        assert_eq!(marks.state("al"), None);
    }
}
