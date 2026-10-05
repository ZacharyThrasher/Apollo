//! Playlist editing: create (empty, or holding a queue's songs), edit the title, description and
//! visibility of and delete playlists, remove and reorder the rows of one you own, save or unsave
//! playlists and albums to the library, and add one song to the library or take it out.
//!
//! These write to the real account, so every one follows the same rules:
//!
//! * **Never the wrong account.** The account the write was asked for in is recorded when it is
//!   asked ([`LibrarySource::account_token`]) and checked again on the worker thread right before
//!   anything is sent; a changed account sends nothing. Answers for an account that has since
//!   been replaced are dropped (`generation`).
//! * **Never blind.** The transport sends each of these once. A write that fails with an unknown
//!   outcome (a timeout, a dropped connection, an unreadable reply) is settled by reading the
//!   service back, never by sending it again: [`create`], [`delete`], [`rename`], [`remove`],
//!   [`move_row`], [`set_saved`] and [`set_song_saved`] each know what the service must hold if
//!   the write landed.
//!   When the read-back cannot be made either, the write is reported as failed (unknown).
//! * **Only what is certainly editable.** Rename, delete and row edits are refused locally for
//!   anything but a `PL…` playlist ([`is_user_playlist_id`]); Liked Music, albums and YouTube's
//!   own lists never reach the service as an edit.
//! * **One write per object at a time.** A second request for something with a write out is
//!   ignored.
//!
//! The functions at the top are plain blocking code over [`LibrarySource`], tested against the
//! simulated source. [`LibraryEdits`] below is the entity the dialogs and menus talk to: it keeps
//! what the library holds (so a menu knows what is saved and what is editable), runs the
//! functions off the UI thread, and says what happened as [`EditEvent`]s.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use apollo_domain::library::{
    is_user_playlist_id, Collection, CollectionKind, PlaylistEdit, PlaylistTarget, Privacy,
    WriteFailure,
};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::BrowseId;
use gpui::{Context, Entity, EventEmitter};

use super::model::Collections;
use super::source::{LibrarySource, TrackRow};
use super::writes::LibraryWrites;
use crate::player::PlayerModel;

/// The most pages a read-back follows. A playlist longer than this cannot be read back, and the
/// write is reported as unknown.
const MAX_PAGES: usize = 100;

/// What a write makes stale in the page cache, dropped when the write goes out and again when it
/// settles (a read between the two must not put the old page back).
#[derive(Clone, Debug)]
pub enum Stale {
    /// The set of playlists and albums (one was created).
    Library,
    /// One collection's page, and the library list that carries its title.
    Collection(String),
    /// Library > Songs (a song was added or taken out).
    Songs,
}

impl Stale {
    pub fn invalidate(&self, cx: &gpui::App) {
        let Some(cache) = crate::page_cache::PageCache::try_global(cx) else {
            return;
        };
        match self {
            Stale::Library => cache.invalidate_library(),
            Stale::Collection(id) => cache.invalidate_collection(id),
            Stale::Songs => cache.invalidate_songs(),
        }
    }
}

/// What was being done, for the words of a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Create,
    /// Create a playlist from the queue.
    SaveQueue,
    /// Create a playlist from the Crate.
    SaveCrate,
    /// Change a playlist's title, description or visibility.
    Update,
    Delete,
    RemoveSong,
    MoveSong,
    Save,
    Unsave,
}

/// What a failed edit says: what happened in six words or fewer.
pub fn failure_text(edit: Edit, failure: WriteFailure) -> &'static str {
    match failure {
        WriteFailure::Session => "Sign-in expired",
        WriteFailure::Throttled => "Too many requests",
        _ => match edit {
            Edit::Create => "Couldn't create playlist",
            Edit::SaveQueue => "Couldn't save queue",
            Edit::SaveCrate => "Couldn't save Crate",
            Edit::Update => "Couldn't update playlist",
            Edit::Delete => "Couldn't delete playlist",
            Edit::RemoveSong => "Couldn't remove song",
            Edit::MoveSong => "Couldn't move song",
            Edit::Save => "Couldn't save to library",
            Edit::Unsave => "Couldn't remove from library",
        },
    }
}

// ---- the writes, each settled by reading back ------------------------------------------------

/// Sends nothing unless the account is still the one the write was asked for in.
fn guard(source: &dyn LibrarySource, expect: &Option<String>) -> Result<(), WriteFailure> {
    if source.account_token() == *expect {
        Ok(())
    } else {
        Err(WriteFailure::Rejected)
    }
}

/// Playlists people can save to their library: users' own (`PL…`) and YouTube Music's curated
/// mixes (`RDCLAK…`). Radios and other generated lists (`RDAMVM…`, `RDEM…`) are not offered.
pub fn is_saveable_playlist_id(bare: &str) -> bool {
    is_user_playlist_id(bare) || bare.starts_with("RDCLAK")
}

/// Playlist rows that can be edited need an id Apollo is sure is a user playlist.
fn editable(bare_id: &str) -> Result<(), WriteFailure> {
    if is_user_playlist_id(bare_id) {
        Ok(())
    } else {
        Err(WriteFailure::Rejected)
    }
}

/// Every row of a collection, or an error when any page could not be read: a partial list cannot
/// say that a row is gone.
pub fn all_rows(
    source: &dyn LibrarySource,
    collection: &Collection,
) -> Result<Vec<TrackRow>, LoadFailure> {
    let first = source.tracks(collection)?;
    let mut rows = first.items;
    let mut next = first.continuation;
    let mut pages = 1;
    while let Some(token) = next.take() {
        if pages >= MAX_PAGES {
            return Err(LoadFailure::Other);
        }
        let page = source.more_tracks(&token)?;
        pages += 1;
        rows.extend(page.items);
        next = page.continuation;
    }
    Ok(rows)
}

/// The library's playlists and albums as the service lists them now.
fn library(source: &dyn LibrarySource) -> Option<Vec<Collection>> {
    source.collections().ok()
}

/// Create a private, empty playlist and return its bare id. `known` is every playlist id the
/// library held before: an unknown outcome is settled by looking for a playlist with this title
/// that was not there.
pub fn create(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    known: &HashSet<String>,
    title: &str,
) -> Result<String, WriteFailure> {
    guard(source, expect)?;
    match source.create_playlist(title) {
        Ok(id) => Ok(id),
        Err(failure) if failure.outcome_unknown() => found_new(source, known, title).ok_or(failure),
        Err(failure) => Err(failure),
    }
}

/// Create a playlist that already holds `video_ids`, in order, and return its bare id. One write
/// carries the playlist and its songs, so a playlist that is there is whole. Settled like
/// [`create`]: by finding a playlist with this title that was not in the library before.
pub fn create_with(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    known: &HashSet<String>,
    title: &str,
    privacy: Privacy,
    video_ids: &[String],
) -> Result<String, WriteFailure> {
    guard(source, expect)?;
    match source.create_playlist_with(title, "", privacy, video_ids) {
        Ok(id) => Ok(id),
        Err(failure) if failure.outcome_unknown() => found_new(source, known, title).ok_or(failure),
        Err(failure) => Err(failure),
    }
}

/// The id of a playlist titled `title` that the library did not hold before (`known`).
fn found_new(source: &dyn LibrarySource, known: &HashSet<String>, title: &str) -> Option<String> {
    library(source).and_then(|list| {
        list.into_iter()
            .filter(|c| c.kind == CollectionKind::Playlist)
            .find(|c| c.title == title && !known.contains(c.library_key()))
            .map(|c| c.library_key().to_owned())
    })
}

/// Delete a playlist the account owns. Settled by the playlist being gone from the library.
pub fn delete(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    playlist_id: &str,
) -> Result<(), WriteFailure> {
    editable(playlist_id)?;
    guard(source, expect)?;
    match source.delete_playlist(playlist_id) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => match library(source) {
            Some(list) if !list.iter().any(|c| c.library_key() == playlist_id) => Ok(()),
            _ => Err(failure),
        },
        Err(failure) => Err(failure),
    }
}

/// Change the title, description and visibility of a playlist the account owns: only what
/// `change` carries is sent, in one request. Settled by the service holding all of it: the
/// library listing the new title, the playlist's page saying the new description and visibility.
pub fn edit(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    collection: &Collection,
    change: &PlaylistEdit,
) -> Result<(), WriteFailure> {
    let playlist_id = collection.library_key();
    editable(playlist_id)?;
    if change.is_empty() {
        return Ok(());
    }
    guard(source, expect)?;
    match source.edit_playlist(playlist_id, change) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => {
            if edit_applied(source, collection, change) {
                Ok(())
            } else {
                Err(failure)
            }
        }
        Err(failure) => Err(failure),
    }
}

/// Whether the service holds every part of `change` now. `false` when it does not, and also when
/// it could not be asked: an unproven edit is a failed one.
fn edit_applied(source: &dyn LibrarySource, collection: &Collection, change: &PlaylistEdit) -> bool {
    let key = collection.library_key();
    if let Some(title) = &change.title {
        let titled = library(source)
            .is_some_and(|list| list.iter().any(|c| c.library_key() == key && c.title == *title));
        if !titled {
            return false;
        }
    }
    if change.description.is_none() && change.privacy.is_none() {
        return true;
    }
    let Ok((_, meta)) = source.page(collection) else {
        return false;
    };
    change
        .description
        .as_ref()
        .is_none_or(|d| meta.description.clone().unwrap_or_default() == *d)
        && change.privacy.is_none_or(|p| meta.privacy == Some(p))
}

/// Remove one row of a playlist the account owns. `occurrence` is the row's own token, so a song
/// listed twice loses only that row. Settled by the page no longer holding the token.
pub fn remove(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    collection: &Collection,
    video_id: &str,
    occurrence: &str,
) -> Result<(), WriteFailure> {
    let playlist_id = collection.library_key();
    editable(playlist_id)?;
    guard(source, expect)?;
    match source.remove_from_playlist(playlist_id, video_id, occurrence) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => match all_rows(source, collection) {
            Ok(rows) if !rows.iter().any(|r| r.occurrence.as_deref() == Some(occurrence)) => Ok(()),
            _ => Err(failure),
        },
        Err(failure) => Err(failure),
    }
}

/// Move one row of a playlist the account owns in front of another (`before` `None`: to the
/// end). `desired` is every row's token in the order the playlist should now have; an unknown
/// outcome is settled by the page holding exactly that order.
pub fn move_row(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    collection: &Collection,
    occurrence: &str,
    before: Option<&str>,
    desired: &[String],
) -> Result<(), WriteFailure> {
    let playlist_id = collection.library_key();
    editable(playlist_id)?;
    guard(source, expect)?;
    match source.move_in_playlist(playlist_id, occurrence, before) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => match all_rows(source, collection) {
            Ok(rows) => {
                let held: Vec<&str> =
                    rows.iter().filter_map(|r| r.occurrence.as_deref()).collect();
                let wanted: Vec<&str> = desired.iter().map(String::as_str).collect();
                if held == wanted {
                    Ok(())
                } else {
                    Err(failure)
                }
            }
            Err(_) => Err(failure),
        },
        Err(failure) => Err(failure),
    }
}

/// Save a playlist or album to the library (`saved`) or take it out. `save_id` is what the save
/// is made on (see [`LibrarySource::save_target`]). Settled by the library holding the collection
/// or not.
pub fn set_saved(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    collection: &Collection,
    save_id: &str,
    saved: bool,
) -> Result<(), WriteFailure> {
    // Liked Music is the heart's, and an own playlist is in the library by being yours.
    if collection.is_liked_music() || save_id.is_empty() || save_id == "LM" {
        return Err(WriteFailure::Rejected);
    }
    guard(source, expect)?;
    match source.set_saved(save_id, saved) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => match library(source) {
            Some(list)
                if list
                    .iter()
                    .any(|c| c.kind == collection.kind && c.library_key() == collection.library_key())
                    == saved =>
            {
                Ok(())
            }
            _ => Err(failure),
        },
        Err(failure) => Err(failure),
    }
}

/// Every song of Library > Songs, or `None` when any page could not be read: a partial list
/// cannot say that a song is not there.
pub fn all_song_rows(source: &dyn LibrarySource) -> Option<Vec<TrackRow>> {
    let first = source.library_songs().ok()?;
    let mut rows = first.items;
    let mut next = first.continuation;
    let mut pages = 1;
    while let Some(token) = next.take() {
        if pages >= MAX_PAGES {
            return None;
        }
        let page = source.more_tracks(&token).ok()?;
        pages += 1;
        rows.extend(page.items);
        next = page.continuation;
    }
    Some(rows)
}

/// Add one song to the library (`saved`) or take it out. Settled by Library > Songs holding the
/// song or not.
pub fn set_song_saved(
    source: &dyn LibrarySource,
    expect: &Option<String>,
    video_id: &str,
    saved: bool,
) -> Result<(), WriteFailure> {
    if video_id.is_empty() {
        return Err(WriteFailure::Rejected);
    }
    guard(source, expect)?;
    match source.set_song_saved(video_id, saved) {
        Ok(()) => Ok(()),
        Err(failure) if failure.outcome_unknown() => {
            let held = all_song_rows(source)
                .map(|rows| rows.iter().any(|r| r.track.video_id.as_str() == video_id));
            if held == Some(saved) {
                source.note_song_saved(video_id, saved);
                Ok(())
            } else {
                Err(failure)
            }
        }
        Err(failure) => Err(failure),
    }
}

// ---- the entity -------------------------------------------------------------------------------

/// What the library holds, as the menus need it.
#[derive(Default)]
struct Membership {
    /// The lists have been loaded: until they have, nothing is known to be saved or not.
    loaded: bool,
    /// Bare ids of the library's playlists.
    playlists: HashSet<String>,
    /// Titles of the library's playlists (what an instant playlist's default name steps around).
    playlist_titles: HashSet<String>,
    /// Browse ids of the library's albums.
    albums: HashSet<String>,
    /// Bare ids of the playlists the account can edit; `None` until the service has said.
    editable: Option<HashSet<String>>,
}

/// What the screens learn from an edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditEvent {
    /// A playlist was created; it is in the library now.
    Created(Collection),
    /// A playlist now has this title (the collection carries it). Raised only when the title
    /// changed, along with [`EditEvent::Edited`].
    Renamed(Collection),
    /// An edit of a playlist's title, description or visibility went through (the collection
    /// carries the new title).
    Edited(Collection),
    /// A song is in the library (`saved`) or no longer is.
    SongSaved { video_id: String, saved: bool },
    Deleted(Collection),
    /// The collection is in the library (`saved`) or no longer is.
    Saved { collection: Collection, saved: bool },
    /// The edit did not happen (a notice has been raised).
    Failed(Edit),
}

pub struct LibraryEdits {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    membership: Membership,
    /// Bumped when the account changes; answers for an older one are dropped.
    generation: u64,
    /// Objects with a write out: `"create"`, `"playlist:<id>"`, `"save:<key>"`.
    busy: HashSet<String>,
    /// The editable list has been asked for in this generation.
    editable_asked: bool,
    /// What the account's own playlists say about themselves, by bare id, once read (the Edit
    /// playlist dialog prefills from it).
    details: HashMap<String, PlaylistDetails>,
    /// Playlists whose details are being read.
    details_asked: HashSet<String>,
}

/// What a playlist of the account's own says about itself, beyond its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDetails {
    /// The blurb, empty when there is none.
    pub description: String,
    /// Who can see it; `None` when the page did not say (nothing is then offered to change).
    pub privacy: Option<Privacy>,
}

impl EventEmitter<EditEvent> for LibraryEdits {}

impl LibraryEdits {
    pub fn new(player: Entity<PlayerModel>, writes: Entity<LibraryWrites>) -> Self {
        Self {
            player,
            writes,
            membership: Membership::default(),
            generation: 0,
            busy: HashSet::new(),
            editable_asked: false,
            details: HashMap::new(),
            details_asked: HashSet::new(),
        }
    }

    fn source(&self, cx: &gpui::App) -> Option<Arc<dyn LibrarySource>> {
        self.player.read(cx).library_source()
    }

    // ---- reading -----------------------------------------------------------------------

    /// The account owns this playlist and can rename and delete it: the service listed it among
    /// the playlists the account can edit, and it is a user playlist.
    pub fn is_editable(&self, collection: &Collection) -> bool {
        collection.kind == CollectionKind::Playlist
            && is_user_playlist_id(collection.library_key())
            && self
                .membership
                .editable
                .as_ref()
                .is_some_and(|set| set.contains(collection.library_key()))
    }

    /// Whether the library holds an album or someone else's playlist: `None` where saving makes
    /// no sense (Liked Music, a playlist of your own) or the lists are not loaded yet.
    pub fn saved_state(&self, collection: &Collection) -> Option<bool> {
        if !self.membership.loaded || collection.is_liked_music() || self.is_editable(collection) {
            return None;
        }
        let key = collection.library_key();
        match collection.kind {
            CollectionKind::Playlist => {
                let held = self.membership.playlists.contains(key);
                // What is in the library can always be taken out; a list is offered for saving
                // only when it is one the service lets people save (not a radio or a mix made
                // for one song).
                (held || is_saveable_playlist_id(key)).then_some(held)
            }
            CollectionKind::Album => Some(self.membership.albums.contains(key)),
        }
    }

    /// A write for this object is out.
    pub fn is_busy(&self, key: &str) -> bool {
        self.busy.contains(key)
    }

    /// Whether the library holds a playlist with this title.
    pub fn has_playlist_titled(&self, title: &str) -> bool {
        self.membership.playlist_titles.contains(title)
    }

    /// Every playlist id the library holds.
    pub fn known_playlists(&self) -> &HashSet<String> {
        &self.membership.playlists
    }

    /// Whether the library holds a song, with a way to change that: `None` for a song no row has
    /// offered the change on (the menu then has no item for it).
    pub fn song_saved(&self, video_id: &str, cx: &gpui::App) -> Option<bool> {
        self.source(cx)?.song_saved(video_id)
    }

    /// What a playlist of the account's own says about itself, once [`Self::load_details`] has
    /// read it (or an edit has set it).
    pub fn details(&self, collection: &Collection) -> Option<&PlaylistDetails> {
        self.details.get(collection.library_key())
    }

    /// Read a playlist's description and visibility off its page, for the Edit playlist dialog.
    /// Asked once per playlist until an edit or a reset; a failed read leaves nothing known, and
    /// the dialog then offers only the title.
    pub fn load_details(&mut self, collection: &Collection, cx: &mut Context<Self>) {
        let key = collection.library_key().to_owned();
        if !self.is_editable(collection) || self.details.contains_key(&key) {
            return;
        }
        if !self.details_asked.insert(key.clone()) {
            return;
        }
        let Some(source) = self.source(cx) else {
            self.details_asked.remove(&key);
            return;
        };
        let (generation, asked) = (self.generation, collection.clone());
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move { source.page(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                m.details_asked.remove(&key);
                if let Ok((_, meta)) = answer {
                    // The owner's header is what carries both: a page without one says nothing.
                    if meta.owned {
                        m.details.entry(key).or_insert(PlaylistDetails {
                            description: meta.description.unwrap_or_default(),
                            privacy: meta.privacy,
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- what the library holds ----------------------------------------------------------

    /// The lists were (re)loaded: take them as what the library holds, and ask which playlists
    /// are editable.
    pub fn set_library(&mut self, collections: &Collections, cx: &mut Context<Self>) {
        self.membership.playlists = collections
            .playlists
            .iter()
            .map(|c| c.library_key().to_owned())
            .collect();
        self.membership.playlist_titles =
            collections.playlists.iter().map(|c| c.title.clone()).collect();
        self.membership.albums = collections
            .albums
            .iter()
            .map(|c| c.library_key().to_owned())
            .collect();
        self.membership.loaded = true;
        self.load_editable(cx);
        cx.notify();
    }

    fn load_editable(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source(cx) else {
            return;
        };
        let generation = self.generation;
        self.editable_asked = true;
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move { source.editable_playlists() })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                // A failed answer leaves nothing editable: no rename or delete is offered rather
                // than offered for a playlist that might not be the account's.
                m.membership.editable = Some(answer.unwrap_or_default().into_iter().collect());
                cx.notify();
            });
        })
        .detach();
    }

    /// Forget everything: the account changed, and nothing known about the old one applies.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.membership = Membership::default();
        self.busy.clear();
        self.editable_asked = false;
        self.details.clear();
        self.details_asked.clear();
        cx.notify();
    }

    // ---- the edits -----------------------------------------------------------------------

    /// Run `work` off the UI thread for the object `key`, then `done` with its result. Ignored
    /// (returns false) while a write for the same object is out.
    fn run<T: Send + 'static>(
        &mut self,
        key: String,
        stale: Stale,
        cx: &mut Context<Self>,
        work: impl FnOnce(&dyn LibrarySource, &Option<String>) -> Result<T, WriteFailure>
            + Send
            + 'static,
        done: impl FnOnce(&mut Self, Result<T, WriteFailure>, &mut Context<Self>) + 'static,
    ) -> bool {
        if self.busy.contains(&key) {
            return false;
        }
        let Some(source) = self.source(cx) else {
            // Nothing to write through: a definite no.
            done(self, Err(WriteFailure::Unsupported), cx);
            return true;
        };
        let expect = source.account_token();
        self.busy.insert(key.clone());
        stale.invalidate(cx);
        cx.notify();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&*source, &expect) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                m.busy.remove(&key);
                stale.invalidate(cx);
                done(m, result, cx);
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// A playlist of the account's was made: it is in the library and the account can edit it.
    /// Before the editable list has been read, the new playlist is the one thing known to be
    /// editable (so it can be renamed at once) and the full list is asked for.
    fn note_created(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        self.membership.playlists.insert(id.to_owned());
        self.membership.playlist_titles.insert(title.to_owned());
        match self.membership.editable.as_mut() {
            Some(set) => {
                set.insert(id.to_owned());
            }
            None => {
                self.membership.editable = Some(HashSet::from([id.to_owned()]));
                self.load_editable(cx);
            }
        }
    }

    fn fail(&mut self, edit: Edit, failure: WriteFailure, cx: &mut Context<Self>) {
        let text = failure_text(edit, failure);
        self.writes.update(cx, |w, cx| w.raise(text, cx));
        cx.emit(EditEvent::Failed(edit));
    }

    /// Create a private playlist titled `title` (already cleaned). With `add_video`, the song is
    /// added to it once it exists (the add-to-playlist popover's "New playlist…").
    pub fn create(&mut self, title: String, add_video: Option<String>, cx: &mut Context<Self>) {
        let known = self.membership.playlists.clone();
        let asked = title.clone();
        self.run(
            "create".into(),
            Stale::Library,
            cx,
            move |source, expect| create(source, expect, &known, &asked),
            move |m, result, cx| match result {
                Ok(id) => {
                    let collection = Collection {
                        kind: CollectionKind::Playlist,
                        id: BrowseId::new(format!("VL{id}")),
                        title,
                        subtitle: Some("Playlist".into()),
                        thumbnail: None,
                    };
                    m.note_created(&id, &collection.title, cx);
                    if let Some(video) = add_video {
                        let target = PlaylistTarget {
                            id,
                            title: collection.title.clone(),
                            thumbnail: None,
                            holds_track: false,
                        };
                        m.writes
                            .update(cx, |w, cx| w.add_to_playlist(&target, &video, cx));
                    }
                    cx.emit(EditEvent::Created(collection));
                }
                Err(failure) => m.fail(Edit::Create, failure, cx),
            },
        );
    }

    /// Create a private playlist titled `title` (already cleaned) that holds `video_ids`, in
    /// order: the queue saved as a playlist. One write; the playlist is whole or not there.
    pub fn create_with_songs(
        &mut self,
        title: String,
        video_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.create_songs(Edit::SaveQueue, title, video_ids, cx);
    }

    /// [`Self::create_with_songs`] for the Crate: the same write, its failure worded for the Crate.
    pub fn create_with_crate(
        &mut self,
        title: String,
        video_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.create_songs(Edit::SaveCrate, title, video_ids, cx);
    }

    /// The same write for a playlist made from the add-to-playlist picker (a typed name and the
    /// songs it was opened for), with `edit` choosing the words of a failure.
    pub fn create_songs(
        &mut self,
        edit: Edit,
        title: String,
        video_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let known = self.membership.playlists.clone();
        let asked = title.clone();
        self.run(
            "create".into(),
            Stale::Library,
            cx,
            move |source, expect| {
                create_with(source, expect, &known, &asked, Privacy::Private, &video_ids)
            },
            move |m, result, cx| match result {
                Ok(id) => {
                    let collection = Collection {
                        kind: CollectionKind::Playlist,
                        id: BrowseId::new(format!("VL{id}")),
                        title,
                        subtitle: Some("Playlist".into()),
                        thumbnail: None,
                    };
                    m.note_created(&id, &collection.title, cx);
                    cx.emit(EditEvent::Created(collection));
                }
                Err(failure) => m.fail(edit, failure, cx),
            },
        );
    }

    /// Change a playlist the account owns: only what `change` carries (its title already cleaned)
    /// is sent. A change that is nothing is not sent and is still a success.
    pub fn edit(&mut self, collection: Collection, change: PlaylistEdit, cx: &mut Context<Self>) {
        if !self.is_editable(&collection) {
            return;
        }
        let key = collection.library_key().to_owned();
        let (id, asked, sent) = (key.clone(), collection.clone(), change.clone());
        self.run(
            format!("playlist:{key}"),
            Stale::Collection(key.clone()),
            cx,
            move |source, expect| edit(source, expect, &asked, &sent),
            move |m, result, cx| match result {
                Ok(()) => {
                    if let Some(details) = m.details.get_mut(&id) {
                        if let Some(description) = &change.description {
                            details.description = description.clone();
                        }
                        if let Some(privacy) = change.privacy {
                            details.privacy = Some(privacy);
                        }
                    }
                    let mut edited = collection;
                    if let Some(title) = change.title {
                        m.membership.playlist_titles.remove(&edited.title);
                        m.membership.playlist_titles.insert(title.clone());
                        edited.title = title;
                        cx.emit(EditEvent::Renamed(edited.clone()));
                    }
                    cx.emit(EditEvent::Edited(edited));
                }
                Err(failure) => m.fail(Edit::Update, failure, cx),
            },
        );
    }

    /// Rename a playlist the account owns to `title` (already cleaned).
    pub fn rename(&mut self, collection: Collection, title: String, cx: &mut Context<Self>) {
        let change = PlaylistEdit { title: Some(title), ..PlaylistEdit::default() };
        self.edit(collection, change, cx);
    }

    /// Add a song to the library (`saved`) or take it out. A song nothing offers the change on is
    /// left alone.
    pub fn set_song_saved(&mut self, video_id: String, saved: bool, cx: &mut Context<Self>) {
        if self.song_saved(&video_id, cx).is_none() {
            return;
        }
        let edit = if saved { Edit::Save } else { Edit::Unsave };
        let asked = video_id.clone();
        self.run(
            format!("song:{video_id}"),
            Stale::Songs,
            cx,
            move |source, expect| set_song_saved(source, expect, &asked, saved),
            move |m, result, cx| match result {
                Ok(()) => cx.emit(EditEvent::SongSaved { video_id, saved }),
                Err(failure) => m.fail(edit, failure, cx),
            },
        );
    }

    /// Delete a playlist the account owns.
    pub fn delete(&mut self, collection: Collection, cx: &mut Context<Self>) {
        if !self.is_editable(&collection) {
            return;
        }
        let key = collection.library_key().to_owned();
        let id = key.clone();
        self.run(
            format!("playlist:{key}"),
            Stale::Collection(key.clone()),
            cx,
            move |source, expect| delete(source, expect, &id),
            move |m, result, cx| match result {
                Ok(()) => {
                    m.membership.playlists.remove(&key);
                    m.membership.playlist_titles.remove(&collection.title);
                    if let Some(set) = m.membership.editable.as_mut() {
                        set.remove(&key);
                    }
                    cx.emit(EditEvent::Deleted(collection));
                }
                Err(failure) => m.fail(Edit::Delete, failure, cx),
            },
        );
    }

    /// Save an album or someone else's playlist to the library (`saved`), or take it out.
    pub fn set_saved(&mut self, collection: Collection, saved: bool, cx: &mut Context<Self>) {
        if self.saved_state(&collection).is_none() {
            return;
        }
        let key = collection.library_key().to_owned();
        let edit = if saved { Edit::Save } else { Edit::Unsave };
        let asked = collection.clone();
        self.run(
            format!("save:{key}"),
            Stale::Collection(key.clone()),
            cx,
            move |source, expect| {
                // An album is saved on its audio playlist, which takes reading its page.
                let save_id = source
                    .save_target(&asked)
                    .map_err(|_| WriteFailure::Rejected)?;
                set_saved(source, expect, &asked, &save_id, saved)
            },
            move |m, result, cx| match result {
                Ok(()) => {
                    let set = match collection.kind {
                        CollectionKind::Playlist => &mut m.membership.playlists,
                        CollectionKind::Album => &mut m.membership.albums,
                    };
                    if saved {
                        set.insert(key);
                    } else {
                        set.remove(&key);
                    }
                    cx.emit(EditEvent::Saved { collection, saved });
                }
                Err(failure) => m.fail(edit, failure, cx),
            },
        );
    }
}

#[cfg(test)]
mod tests;
