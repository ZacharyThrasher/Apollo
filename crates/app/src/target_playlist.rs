//! The target playlist: one playlist of your own that **A** adds songs to, no dialog. The name of
//! the target shows as a small pill in the player bar; the pill (or A with no target set) opens
//! the playlist picker in its "choose a target" mode.
//!
//! The target is remembered between runs as a settings value (`TARGET_PLAYLIST`), kept with the
//! account it was chosen on: another account's playlist id means nothing here, so the target
//! reads as unset until that account is chosen again. A playlist that is deleted stops being the
//! target; one that is renamed keeps being it under its new name.
//!
//! Adding goes through [`LibraryWrites`] like the picker's own adds: shown as held at once and
//! taken back with the usual "Couldn't add to playlist" notice if the service refuses. A song the
//! playlist already holds is skipped, not added twice (the service skips it too).

use apollo_domain::library::{bare_playlist_id, PlaylistTarget};
use gpui::{App, Context, Entity, EventEmitter, Global, SharedString, Window};

use crate::library::edits::{EditEvent, LibraryEdits};
use crate::library::writes::LibraryWrites;
use crate::settings::{SettingsModel, TARGET_KEY, TARGET_PILL, TARGET_PLAYLIST};
use crate::ui::playlist_picker::{self, PickerSong};

/// The playlist A adds to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The bare playlist id (`PL…`).
    pub id: String,
    pub name: String,
}

impl Target {
    /// The playlist as the add calls take it.
    pub fn as_playlist(&self) -> PlaylistTarget {
        PlaylistTarget {
            id: self.id.clone(),
            title: self.name.clone(),
            thumbnail: None,
            holds_track: false,
        }
    }
}

/// What the stored value holds: `account <TAB> id <TAB> name`.
fn encode(account: &str, target: &Target) -> String {
    format!("{account}\t{}\t{}", target.id, target.name)
}

/// The target stored for `account`; none when nothing is stored, the value is damaged, or it was
/// chosen on another account.
pub fn decode(text: &str, account: &str) -> Option<Target> {
    let mut parts = text.splitn(3, '\t');
    let (stored_account, id, name) = (parts.next()?, parts.next()?, parts.next()?);
    let id = id.trim();
    let name = name.trim();
    if stored_account != account || id.is_empty() || name.is_empty() {
        return None;
    }
    Some(Target {
        id: id.to_owned(),
        name: name.to_owned(),
    })
}

/// The target changed (set, renamed or cleared).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetChanged;

pub struct TargetModel {
    settings: Entity<SettingsModel>,
    writes: Entity<LibraryWrites>,
    /// The chosen account's opaque key; empty before one is chosen.
    account: String,
}

impl EventEmitter<TargetChanged> for TargetModel {}

/// The shell's target model, reachable from any screen.
#[derive(Clone)]
pub struct TargetHandle(pub Entity<TargetModel>);

impl Global for TargetHandle {}

impl TargetModel {
    pub fn new(
        settings: Entity<SettingsModel>,
        writes: Entity<LibraryWrites>,
        edits: &Entity<LibraryEdits>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Switches change what the pill and the key do.
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.subscribe(edits, |m, _, event: &EditEvent, cx| m.edited(event, cx))
            .detach();
        Self {
            settings,
            writes,
            account: String::new(),
        }
    }

    /// The account the target belongs to is now this one.
    pub fn set_account(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        let key = key.unwrap_or_default();
        if self.account != key {
            self.account = key;
            cx.emit(TargetChanged);
            cx.notify();
        }
    }

    /// The target of the chosen account.
    pub fn target(&self, cx: &App) -> Option<Target> {
        decode(
            &self.settings.read(cx).get_text(&TARGET_PLAYLIST),
            &self.account,
        )
    }

    /// Settings > Playlists > A adds to the target playlist.
    pub fn key_enabled(&self, cx: &App) -> bool {
        self.settings.read(cx).get(&TARGET_KEY)
    }

    /// Settings > Playlists > Show target playlist in player bar.
    pub fn pill_shown(&self, cx: &App) -> bool {
        self.settings.read(cx).get(&TARGET_PILL)
    }

    /// Make `id` (a bare playlist id) the target.
    pub fn set(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let target = Target {
            id: bare_playlist_id(id).to_owned(),
            name: name.trim().to_owned(),
        };
        if target.id.is_empty() || target.name.is_empty() {
            return;
        }
        self.store(&encode(&self.account, &target), cx);
    }

    /// No target any more.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.store("", cx);
    }

    fn store(&mut self, value: &str, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |s, cx| s.set_text(&TARGET_PLAYLIST, value, cx));
        cx.emit(TargetChanged);
        cx.notify();
    }

    /// A playlist was deleted or renamed: the target follows.
    fn edited(&mut self, event: &EditEvent, cx: &mut Context<Self>) {
        let Some(target) = self.target(cx) else {
            return;
        };
        match event {
            EditEvent::Deleted(collection) if collection.library_key() == target.id => {
                self.clear(cx);
            }
            EditEvent::Renamed(collection) if collection.library_key() == target.id => {
                self.set(&target.id, &collection.title, cx);
            }
            _ => {}
        }
    }

    /// Add `songs` to the target, in order. Songs the playlist is already known to hold (added
    /// here earlier) and repeats are left out; the service skips any other it holds. Nothing
    /// happens without a target.
    pub fn add(&mut self, songs: &[PickerSong], cx: &mut Context<Self>) {
        let Some(target) = self.target(cx) else {
            return;
        };
        self.add_to(&target, songs, cx);
    }

    /// Add `songs` to `target` (see [`Self::add`]).
    pub fn add_to(&mut self, target: &Target, songs: &[PickerSong], cx: &mut Context<Self>) {
        let playlist = target.as_playlist();
        let mut videos: Vec<String> = Vec::new();
        {
            let writes = self.writes.read(cx);
            for song in songs {
                if writes.added_here(&playlist, &song.video_id) || videos.contains(&song.video_id)
                {
                    continue;
                }
                videos.push(song.video_id.clone());
            }
        }
        match videos.as_slice() {
            [] => {}
            [video] => {
                let video = video.clone();
                self.writes
                    .update(cx, |w, cx| w.add_to_playlist(&playlist, &video, cx));
            }
            _ => self
                .writes
                .update(cx, |w, cx| w.add_many_to_playlist(&playlist, &videos, cx)),
        }
    }
}

/// The A key (and the pill's click): add `songs` to the target playlist; with no target, the
/// picker opens to choose one and the songs go into it as it is chosen. `anchor` names where the
/// picker opens (see `ui::anchor`).
pub fn add_songs(
    songs: Vec<PickerSong>,
    anchor: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    if songs.is_empty() {
        return;
    }
    let Some(handle) = cx.try_global::<TargetHandle>().map(|h| h.0.clone()) else {
        return;
    };
    if !handle.read(cx).key_enabled(cx) {
        return;
    }
    match handle.read(cx).target(cx) {
        Some(target) => handle.update(cx, |m, cx| m.add_to(&target, &songs, cx)),
        None => playlist_picker::open_target(songs, anchor, window, cx),
    }
}

/// The A key is on (Settings > Playlists), or there is no shell to say it is not.
pub fn key_enabled(cx: &App) -> bool {
    cx.try_global::<TargetHandle>()
        .is_none_or(|h| h.0.read(cx).key_enabled(cx))
}

/// `playlist_id` (bare or not) is the target, and its pill is on show.
pub fn is_target(playlist_id: &str, cx: &App) -> bool {
    let Some(handle) = cx.try_global::<TargetHandle>() else {
        return false;
    };
    let model = handle.0.read(cx);
    model.pill_shown(cx)
        && model
            .target(cx)
            .is_some_and(|t| t.id == bare_playlist_id(playlist_id))
}

/// What the player bar's pill says, when there is a target and the pill is switched on.
pub fn pill_label(cx: &App) -> Option<SharedString> {
    let handle = cx.try_global::<TargetHandle>()?;
    let model = handle.0.read(cx);
    if !model.pill_shown(cx) {
        return None;
    }
    model.target(cx).map(|t| SharedString::from(t.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_kept_with_its_account() {
        let target = Target {
            id: "PLroad".into(),
            name: "Road trip".into(),
        };
        let text = encode("acct-1", &target);
        assert_eq!(decode(&text, "acct-1"), Some(target));
        assert_eq!(decode(&text, "acct-2"), None);
        assert_eq!(decode(&text, ""), None);
    }

    #[test]
    fn a_name_may_hold_anything_but_a_line_break_is_not_part_of_it() {
        let target = Target {
            id: "PLx".into(),
            name: "Gym \u{2192} 2026\tPart 2".into(),
        };
        assert_eq!(decode(&encode("a", &target), "a"), Some(target));
    }

    #[test]
    fn damaged_values_read_as_no_target() {
        for text in ["", "a", "a\tPLx", "a\t\tName", "a\tPLx\t  ", "\t\t"] {
            assert_eq!(decode(text, "a"), None, "{text:?}");
        }
        assert_eq!(decode("\t\t", ""), None);
    }
}
