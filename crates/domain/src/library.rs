//! What a library lists: the account's playlists and saved albums as openable collections.
//!
//! Pure types only. The provider maps service cards into [`Collection`]; the UI lists them and
//! opens one to show its tracks (see [`crate::listing`] for the paginated track list).

use crate::model::BrowseId;

/// Browse id of the account's Liked Music playlist (`LM` with the `VL` browse prefix).
pub const LIKED_MUSIC_BROWSE_ID: &str = "VLLM";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CollectionKind {
    Playlist,
    Album,
}

/// One entry of the library: enough to draw a row and to ask the provider for its tracks.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Collection {
    pub kind: CollectionKind,
    /// Browse id: `VL…` for a playlist, `MPRE…` for an album.
    pub id: BrowseId,
    pub title: String,
    /// The service's own second line (owner, track count, artist and year); shown as given.
    pub subtitle: Option<String>,
    /// Cover art URL, when the card carries one.
    pub thumbnail: Option<String>,
}

impl Collection {
    pub fn is_liked_music(&self) -> bool {
        self.kind == CollectionKind::Playlist && self.id.as_str() == LIKED_MUSIC_BROWSE_ID
    }

    /// The id the library is keyed by: a playlist's bare id (no `VL`), an album's browse id.
    pub fn library_key(&self) -> &str {
        match self.kind {
            CollectionKind::Playlist => bare_playlist_id(self.id.as_str()),
            CollectionKind::Album => self.id.as_str(),
        }
    }
}

/// A playlist's bare id: the browse id without its `VL` prefix.
pub fn bare_playlist_id(id: &str) -> &str {
    id.strip_prefix("VL").unwrap_or(id)
}

/// Whether a bare playlist id belongs to a playlist a user makes and can edit: `PL…`. Liked Music
/// (`LM`), albums (`OLAK5uy_…`), YouTube's own mixes and every other special list are not, so
/// Apollo never sends a rename, delete or row edit for them whatever a page claims.
pub fn is_user_playlist_id(bare: &str) -> bool {
    bare.len() > 2 && bare.starts_with("PL")
}

/// The longest playlist title YouTube Music accepts, in characters.
pub const MAX_PLAYLIST_TITLE: usize = 150;

/// What a name field holds, as a playlist title: trimmed, with runs of whitespace collapsed to
/// single spaces and cut to [`MAX_PLAYLIST_TITLE`] characters. `None` when nothing is left (an
/// empty or blank name is never sent).
pub fn clean_playlist_title(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = collapsed.chars().take(MAX_PLAYLIST_TITLE).collect();
    let title = title.trim_end().to_owned();
    (!title.is_empty()).then_some(title)
}

/// What a collection's page says about editing it, beyond its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageMeta {
    /// The signed-in account owns this playlist, so its rows can be removed and reordered. The
    /// service says so by giving the page the owner's edit header; nothing else counts.
    pub owned: bool,
    /// The id a save to the library is made on: a playlist's bare id, an album's audio playlist
    /// (`OLAK5uy_…`). `None` when the page did not carry one.
    pub save_id: Option<String>,
    /// Whether the page says the collection is already saved (an album page does; a playlist
    /// page does not, so the library's own list decides).
    pub in_library: Option<bool>,
}

/// Where dropping row `from` before the insertion slot `slot` (0 is above the first row, `len`
/// below the last) puts it: the row it ends up in front of, `None` meaning the end. `None` from
/// this function means the drop changes nothing (onto itself, or just below itself), so no
/// write is made.
pub fn plan_move(len: usize, from: usize, slot: usize) -> Option<MovePlan> {
    if from >= len || slot > len || slot == from || slot == from + 1 {
        return None;
    }
    Some(MovePlan { before: (slot < len).then_some(slot) })
}

/// The outcome of [`plan_move`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovePlan {
    /// Index, in the list as it was, of the row the moved one goes in front of; `None` = the end.
    pub before: Option<usize>,
}

/// `items` with row `from` taken out and put before `slot` (an insertion slot of the list as it
/// was). A move [`plan_move`] calls a no-op is one here too.
pub fn moved<T: Clone>(items: &[T], from: usize, slot: usize) -> Vec<T> {
    let mut out = items.to_vec();
    if plan_move(items.len(), from, slot).is_none() {
        return out;
    }
    let item = out.remove(from);
    let at = if slot > from { slot - 1 } else { slot };
    out.insert(at, item);
    out
}

/// Why a library write (a like, an add to a playlist) did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFailure {
    /// The service no longer accepts the session.
    Session,
    Throttled,
    /// The service answered and said no: the write did not take effect.
    Rejected,
    /// No usable answer (a timeout, a dropped connection, an unreadable reply): the write may
    /// or may not have landed, so it is read back before anything is believed.
    Unknown,
    /// This backend cannot write.
    Unsupported,
}

impl WriteFailure {
    /// True when the write might have taken effect anyway.
    pub fn outcome_unknown(self) -> bool {
        self == WriteFailure::Unknown
    }
}

/// A playlist a track can be added to (one the account may edit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistTarget {
    /// The bare playlist id (`PL…`), as the edit takes it; not the `VL…` browse id.
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    /// The playlist already holds the track the list was asked for.
    pub holds_track: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_trimmed_collapsed_and_cut() {
        assert_eq!(clean_playlist_title("  Road   Trip \n"), Some("Road Trip".into()));
        assert_eq!(clean_playlist_title("   \t "), None);
        assert_eq!(clean_playlist_title(""), None);
        let long = "a".repeat(MAX_PLAYLIST_TITLE + 40);
        assert_eq!(clean_playlist_title(&long).unwrap().chars().count(), MAX_PLAYLIST_TITLE);
        // Cutting on characters, never bytes, and no trailing space left by the cut.
        let wide = format!("{}é x", "é".repeat(MAX_PLAYLIST_TITLE - 1));
        let cut = clean_playlist_title(&wide).unwrap();
        assert!(cut.chars().count() <= MAX_PLAYLIST_TITLE);
        assert!(!cut.ends_with(' '));
    }

    #[test]
    fn only_a_pl_id_is_an_editable_playlist() {
        assert!(is_user_playlist_id("PLabc123"));
        for no in ["LM", "SE", "OLAK5uy_x", "RDCLAK5uy_x", "VLPLabc", "PL", "", "RDAMVMx"] {
            assert!(!is_user_playlist_id(no), "{no}");
        }
        assert_eq!(bare_playlist_id("VLPLabc"), "PLabc");
        assert_eq!(bare_playlist_id("PLabc"), "PLabc");
    }

    #[test]
    fn the_library_key_is_the_bare_playlist_or_the_album_browse_id() {
        let mut c = Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new("VLPLabc"),
            title: "T".into(),
            subtitle: None,
            thumbnail: None,
        };
        assert_eq!(c.library_key(), "PLabc");
        c.kind = CollectionKind::Album;
        c.id = BrowseId::new("MPREb_x");
        assert_eq!(c.library_key(), "MPREb_x");
    }

    #[test]
    fn a_drop_on_itself_or_just_below_changes_nothing() {
        assert_eq!(plan_move(5, 2, 2), None);
        assert_eq!(plan_move(5, 2, 3), None);
        assert_eq!(plan_move(5, 9, 0), None, "no such row");
        assert_eq!(plan_move(5, 2, 6), None, "no such slot");
    }

    #[test]
    fn a_drop_names_the_row_it_goes_in_front_of() {
        assert_eq!(plan_move(5, 3, 1), Some(MovePlan { before: Some(1) }));
        assert_eq!(plan_move(5, 0, 3), Some(MovePlan { before: Some(3) }));
        assert_eq!(plan_move(5, 0, 5), Some(MovePlan { before: None }), "the end");
        assert_eq!(plan_move(5, 3, 0), Some(MovePlan { before: Some(0) }));
    }

    #[test]
    fn moving_reorders_exactly_as_the_insertion_line_showed() {
        let v = ["a", "b", "c", "d", "e"];
        assert_eq!(moved(&v, 3, 1), ["a", "d", "b", "c", "e"]);
        assert_eq!(moved(&v, 0, 3), ["b", "c", "a", "d", "e"]);
        assert_eq!(moved(&v, 0, 5), ["b", "c", "d", "e", "a"]);
        assert_eq!(moved(&v, 4, 0), ["e", "a", "b", "c", "d"]);
        assert_eq!(moved(&v, 2, 2), v);
        assert_eq!(moved(&v, 2, 3), v);
    }

    #[test]
    fn only_an_unknown_outcome_needs_reading_back() {
        assert!(WriteFailure::Unknown.outcome_unknown());
        for f in [
            WriteFailure::Session,
            WriteFailure::Throttled,
            WriteFailure::Rejected,
            WriteFailure::Unsupported,
        ] {
            assert!(!f.outcome_unknown(), "{f:?}");
        }
    }
}
