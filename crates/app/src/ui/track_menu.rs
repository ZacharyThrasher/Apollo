//! The track menu: a popover of what can be done with a song (or, shorter, an album or playlist
//! card), opened by right-clicking it or by the More button a row shows on hover.
//!
//! It is one entity hosted by the shell and reached through [`open`], so a row only has to say
//! what it is (a [`Subject`]) and where the menu goes (an [`Anchor`]). The look is the
//! add-to-playlist popover's: bg.overlay, 8px radius, border.subtle, no scrim, 32px rows with a
//! 16px icon and a body-text label. The menu opens at the pointer or under the button and flips
//! to stay inside the window. Up and Down move, Enter chooses, Esc or a click outside closes.
//!
//! Choosing closes the menu at once and the action runs after it. Success is silent; only a
//! failure leaves the usual error toast. Start radio plays the song now and swaps the queue's
//! up-next list for the radio when it arrives, so a radio that cannot be fetched costs nothing.

use std::rc::Rc;

use apollo_domain::arrange::Edge;
use apollo_domain::blocklist::BlockedArtist;
use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::links;
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, QueueEntryId, Track};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, point, App, Bounds, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, Global, IntoElement, KeyBinding, MouseButton, ParentElement, Pixels, Point, Render,
    SharedString, Size, Styled, Window,
};

use crate::assets::Icon;
use crate::library::edits::LibraryEdits;
use crate::library::model::{ArtistTarget, Target};
use crate::queue_source;
use crate::library::writes::LibraryWrites;
use crate::player::PlayerModel;
use crate::shortcuts::{self, Scope};
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::duplicates_dialog;
use crate::ui::playlist_dialog::{self, Mode};
use crate::ui::playlist_picker::{PlaylistPicker, PANEL_WIDTH};

actions!(
    apollo_track_menu,
    [MenuUp, MenuDown, MenuFirst, MenuLast, MenuChoose, MenuClose, MenuStay]
);

/// Key context of the menu.
pub const KEY_CONTEXT: &str = "TrackMenu";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", MenuUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", MenuDown, Some(KEY_CONTEXT)),
        KeyBinding::new("home", MenuFirst, Some(KEY_CONTEXT)),
        KeyBinding::new("end", MenuLast, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", MenuChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("space", MenuChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", MenuClose, Some(KEY_CONTEXT)),
        // Focus stays in the menu while it is open.
        KeyBinding::new("tab", MenuStay, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", MenuStay, Some(KEY_CONTEXT)),
    ]);
}

/// Whether the playlist picker can take a list of songs (`PlaylistPicker::open_many`). A
/// selection's menu has Add to playlist\u{2026} while it can.
pub const PICKER_TAKES_MANY: bool = true;

pub const MENU_WIDTH: f32 = 220.0;
pub const ITEM_HEIGHT: f32 = layout::MIN_HIT_TARGET;
const ICON: f32 = 16.0;
/// Space between the panel's edge and its rows, and above and below a separator.
const PAD: f32 = spacing::S1;
const SEPARATOR: f32 = 1.0;
const BORDER: f32 = 1.0;
/// Gap between the button a menu hangs from and the menu, and the least it keeps from the edge.
const ANCHOR_GAP: f32 = spacing::S1;
const EDGE_MARGIN: f32 = spacing::S2;

// ---- what the menu is about ------------------------------------------------------------------

/// A song, with what a row knows about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackSubject {
    pub track: Track,
    /// The row's cover, handed to the player so a queued song has art.
    pub thumbnail: Option<String>,
    /// Set for a row of the queue panel: the entry it can be removed by.
    pub entry: Option<QueueEntryId>,
    /// Set for a row of a playlist the account can edit: where Remove from playlist takes it out.
    pub playlist_row: Option<PlaylistRow>,
}

/// A row of an editable playlist: the playlist and the row's own token in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistRow {
    pub collection: Collection,
    pub occurrence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Track(TrackSubject),
    /// An album or playlist card.
    Collection(Collection),
    /// The queue, from the More button of its header.
    Queue,
    /// An artist, from the artist page's More button.
    Artist(ArtistSubject),
    /// Two songs or more picked in a list (see `ui::selection`): the verbs act on all of them,
    /// in the list's order.
    Tracks(Vec<TrackSubject>),
}

/// An artist the menu is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistSubject {
    pub id: BrowseId,
    pub name: String,
}

impl ArtistSubject {
    /// The entry the block list keeps for this artist.
    pub fn blocked(&self) -> BlockedArtist {
        BlockedArtist {
            id: Some(self.id.as_str().to_owned()),
            name: self.name.clone(),
        }
    }
}

impl TrackSubject {
    /// A song of a playlist the account can edit, or a plain song where `playlist` is `None`.
    pub fn of_row(
        track: Track,
        thumbnail: Option<String>,
        playlist: Option<(Collection, String)>,
    ) -> Self {
        Self {
            track,
            thumbnail,
            entry: None,
            playlist_row: playlist.map(|(collection, occurrence)| PlaylistRow {
                collection,
                occurrence,
            }),
        }
    }
}

impl Subject {
    pub fn track(track: Track, thumbnail: Option<String>) -> Self {
        Subject::Track(TrackSubject {
            track,
            thumbnail,
            entry: None,
            playlist_row: None,
        })
    }

    /// A song in a playlist the account can edit.
    pub fn playlist_track(
        track: Track,
        thumbnail: Option<String>,
        collection: Collection,
        occurrence: String,
    ) -> Self {
        Subject::Track(TrackSubject {
            track,
            thumbnail,
            entry: None,
            playlist_row: Some(PlaylistRow {
                collection,
                occurrence,
            }),
        })
    }

    /// A song in the queue panel.
    pub fn queued(track: Track, entry: QueueEntryId) -> Self {
        Subject::Track(TrackSubject {
            track,
            thumbnail: None,
            entry: Some(entry),
            playlist_row: None,
        })
    }

    /// The menu of a selection. Fewer than two songs is not a selection: it falls back to the
    /// song's own menu (or none).
    pub fn tracks(mut songs: Vec<TrackSubject>) -> Option<Self> {
        match songs.len() {
            0 => None,
            1 => songs.pop().map(Subject::Track),
            _ => Some(Subject::Tracks(songs)),
        }
    }

    /// The menu of a card: only albums and playlists have one.
    pub fn of_target(target: &Target) -> Option<Self> {
        match target {
            Target::Collection(c) => Some(Subject::Collection(c.clone())),
            Target::Artist(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Shuffle,
    PlayNext,
    AddToQueue,
    StartRadio,
    AddToPlaylist,
    ToggleLike,
    GoToArtist,
    GoToAlbum,
    CopyLink,
    RemoveFromQueue,
    AddToCrate,
    RemoveFromCrate,
    SaveToLibrary,
    RemoveFromLibrary,
    /// Opens the Edit playlist dialog (name, description, privacy).
    EditPlaylist,
    DeletePlaylist,
    RemoveFromPlaylist,
    /// Send the row (or every picked row) to the top of a playlist of your own.
    MoveToTop,
    /// ... and to the bottom.
    MoveToBottom,
    /// Opens the Remove duplicates dialog: a preview of the repeated songs of a playlist of your
    /// own.
    RemoveDuplicates,
    /// Save the queue as a new playlist.
    SaveQueue,
    /// The thumbs-down: tell YouTube Music this song is not wanted.
    NotInterested,
    /// "Title - Artist" on the clipboard.
    CopyTitleArtist,
    OpenOnYoutubeMusic,
    /// Keep the artist's songs out of radio, autoplay and the personal shelves (or let them back).
    BlockArtist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub command: Command,
    pub icon: Icon,
    pub label: &'static str,
}

const fn item(command: Command, icon: Icon, label: &'static str) -> Item {
    Item {
        command,
        icon,
        label,
    }
}

/// The artist a song's menu goes to: the first one that has a page.
pub fn artist_of(track: &Track) -> Option<&ArtistRef> {
    track.artists.iter().find(|a| a.id.is_some())
}

/// The artist a song's block item acts on: the first credited artist that has a page.
pub fn blocked_artist_of(track: &Track) -> Option<BlockedArtist> {
    artist_of(track).map(|a| BlockedArtist {
        id: a.id.as_ref().map(|id| id.as_str().to_owned()),
        name: a.name.clone(),
    })
}

/// The album a song's menu goes to, when the row knew it.
pub fn album_of(track: &Track) -> Option<&AlbumRef> {
    track.album.as_ref().filter(|a| a.id.is_some())
}

/// What the menu knows about a card beyond what it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A playlist of the account's own: it can be renamed and deleted.
    pub own_playlist: bool,
    /// The playlist's own page is open and its songs can be arranged: Remove duplicates reads
    /// that page, so a card elsewhere does not offer it.
    pub page_open: bool,
    /// Whether the library holds the album or playlist; `None` where saving does not apply.
    pub saved: Option<bool>,
    /// Whether the song is in the Crate (its item then takes it out).
    pub in_crate: bool,
    /// Whether the library holds the song; `None` where nothing has offered a way to change that
    /// (the item is then left out).
    pub song_saved: Option<bool>,
    /// Whether the artist the block item acts on is blocked now (its item then lets them back).
    pub artist_blocked: bool,
}

/// The menu's items in groups (a separator sits between groups). `liked` is the song's heart.
pub fn groups(subject: &Subject, liked: bool) -> Vec<Vec<Item>> {
    groups_with(subject, liked, Facts::default())
}

/// [`groups`] with what is known about a card's ownership and place in the library.
pub fn groups_with(subject: &Subject, liked: bool, facts: Facts) -> Vec<Vec<Item>> {
    match subject {
        Subject::Artist(_) => vec![vec![block_item(facts.artist_blocked)]],
        Subject::Collection(_) => {
            let mut out = vec![
                vec![
                    item(Command::Play, Icon::Play, "Play"),
                    item(Command::Shuffle, Icon::ShuffleRegular, "Shuffle"),
                ],
                vec![
                    item(Command::AddToQueue, Icon::QueueAdd, "Add to queue"),
                    item(Command::StartRadio, Icon::Radio, "Start radio"),
                ],
            ];
            match facts.saved {
                Some(false) => out.push(vec![item(
                    Command::SaveToLibrary,
                    Icon::LibraryRegular,
                    "Save to library",
                )]),
                Some(true) => out.push(vec![item(
                    Command::RemoveFromLibrary,
                    Icon::LibraryFilled,
                    "Remove from library",
                )]),
                None => {}
            }
            out.push(vec![item(Command::CopyLink, Icon::Link, "Copy link")]);
            if facts.own_playlist {
                let mut own = vec![item(Command::EditPlaylist, Icon::Edit, "Edit playlist\u{2026}")];
                if facts.page_open {
                    own.push(item(
                        Command::RemoveDuplicates,
                        Icon::Dismiss,
                        "Remove duplicates\u{2026}",
                    ));
                }
                own.push(item(Command::DeletePlaylist, Icon::Delete, "Delete playlist\u{2026}"));
                out.push(own);
            }
            out
        }
        Subject::Track(t) => {
            let mut out = vec![
                vec![
                    item(Command::PlayNext, Icon::PlayNext, "Play next"),
                    item(Command::AddToQueue, Icon::QueueAdd, "Add to queue"),
                ],
                vec![
                    item(Command::StartRadio, Icon::Radio, "Start radio"),
                    item(Command::AddToPlaylist, Icon::PlaylistAdd, "Add to playlist\u{2026}"),
                    if facts.in_crate {
                        item(Command::RemoveFromCrate, Icon::BoxFilled, "Remove from Crate")
                    } else {
                        item(Command::AddToCrate, Icon::BoxRegular, "Add to Crate")
                    },
                ],
                {
                    let mut group = vec![if liked {
                        item(Command::ToggleLike, Icon::HeartFilled, "Remove from Liked")
                    } else {
                        item(Command::ToggleLike, Icon::HeartRegular, "Like")
                    }];
                    match facts.song_saved {
                        Some(false) => group.push(item(
                            Command::SaveToLibrary,
                            Icon::LibraryRegular,
                            "Save to library",
                        )),
                        Some(true) => group.push(item(
                            Command::RemoveFromLibrary,
                            Icon::LibraryFilled,
                            "Remove from library",
                        )),
                        None => {}
                    }
                    group
                },
            ];
            let mut places = Vec::new();
            if artist_of(&t.track).is_some() {
                places.push(item(Command::GoToArtist, Icon::Person, "Go to artist"));
            }
            if album_of(&t.track).is_some() {
                places.push(item(Command::GoToAlbum, Icon::Album, "Go to album"));
            }
            if artist_of(&t.track).is_some() {
                places.push(block_item(facts.artist_blocked));
            }
            if !places.is_empty() {
                out.push(places);
            }
            out.push(vec![item(
                Command::NotInterested,
                Icon::ThumbDislike,
                "Not interested",
            )]);
            out.push(vec![
                item(Command::CopyLink, Icon::Link, "Copy link"),
                item(Command::CopyTitleArtist, Icon::Copy, "Copy title and artist"),
                item(Command::OpenOnYoutubeMusic, Icon::Open, "Open on YouTube Music"),
            ]);
            if t.entry.is_some() {
                out.push(vec![item(
                    Command::RemoveFromQueue,
                    Icon::Delete,
                    "Remove from queue",
                )]);
            }
            if t.playlist_row.is_some() {
                out.push(move_items());
                out.push(vec![item(
                    Command::RemoveFromPlaylist,
                    Icon::Delete,
                    "Remove from playlist",
                )]);
            }
            out
        }
        Subject::Tracks(songs) => {
            let mut out = vec![vec![
                item(Command::PlayNext, Icon::PlayNext, "Play next"),
                item(Command::AddToQueue, Icon::QueueAdd, "Add to queue"),
            ]];
            let mut keep = Vec::new();
            if PICKER_TAKES_MANY {
                keep.push(item(
                    Command::AddToPlaylist,
                    Icon::PlaylistAdd,
                    "Add to playlist\u{2026}",
                ));
            }
            // `in_crate` is true when every song is in the Crate: the item then takes them all out.
            keep.push(if facts.in_crate {
                item(Command::RemoveFromCrate, Icon::BoxFilled, "Remove from Crate")
            } else {
                item(Command::AddToCrate, Icon::BoxRegular, "Add to Crate")
            });
            out.push(keep);
            // `liked` is true when every song is liked: the item then takes them all out.
            out.push(vec![if liked {
                item(Command::ToggleLike, Icon::HeartFilled, "Remove from Liked")
            } else {
                item(Command::ToggleLike, Icon::HeartRegular, "Like")
            }]);
            if songs.iter().all(|s| s.playlist_row.is_some()) {
                out.push(move_items());
                out.push(vec![item(
                    Command::RemoveFromPlaylist,
                    Icon::Delete,
                    "Remove from playlist",
                )]);
            }
            // In the queue panel: the picked rows that are not playing can be taken out, in one
            // step Ctrl+Z puts back.
            if songs.iter().any(|s| s.entry.is_some()) {
                out.push(vec![item(
                    Command::RemoveFromQueue,
                    Icon::Delete,
                    "Remove from queue",
                )]);
            }
            out
        }
        Subject::Queue => vec![vec![item(
            Command::SaveQueue,
            Icon::PlaylistAdd,
            "Save as playlist\u{2026}",
        )]],
    }
}

/// The two ends a row of your own playlist can be sent to.
fn move_items() -> Vec<Item> {
    vec![
        item(Command::MoveToTop, Icon::MoveTop, "Move to top"),
        item(Command::MoveToBottom, Icon::MoveBottom, "Move to bottom"),
    ]
}

/// The block item: "Don't play this artist", or the way back once they are blocked.
fn block_item(blocked: bool) -> Item {
    if blocked {
        item(
            Command::BlockArtist,
            Icon::PersonProhibited,
            "Play this artist again",
        )
    } else {
        item(
            Command::BlockArtist,
            Icon::PersonProhibited,
            "Don't play this artist",
        )
    }
}

/// What Copy title and artist puts on the clipboard: "Title - Artist, Artist".
pub fn title_and_artist(track: &Track) -> String {
    let artists = track
        .artists
        .iter()
        .map(|a| a.name.trim())
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if artists.is_empty() {
        track.title.clone()
    } else {
        format!("{} - {artists}", track.title)
    }
}

fn flat(groups: &[Vec<Item>]) -> Vec<Item> {
    groups.iter().flatten().copied().collect()
}

/// The link Copy link puts on the clipboard.
pub fn link_of(subject: &Subject) -> String {
    match subject {
        Subject::Track(t) => links::track_url(&t.track.video_id),
        Subject::Collection(c) => links::collection_url(c),
        Subject::Queue | Subject::Tracks(_) => String::new(),
        Subject::Artist(a) => format!("https://music.youtube.com/channel/{}", a.id.as_str()),
    }
}

/// The name the Save as playlist dialog starts with: the place the queue plays from, else a
/// generic one.
pub fn queue_playlist_name(heading: Option<&crate::player::QueueHeading>) -> String {
    use apollo_domain::queue::SourceKind;
    match heading {
        Some(crate::player::QueueHeading::Source(source)) => match source.kind {
            SourceKind::Radio | SourceKind::LikedSongs => source.label(),
            _ => source.name.clone(),
        },
        _ => "My queue".to_owned(),
    }
}

/// The songs of a queue as a playlist would hold them: everything the panel lists, in its order,
/// each song once.
pub fn queue_video_ids(view: &crate::player::QueueView) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    view.history
        .iter()
        .chain(view.current.iter())
        .chain(view.upcoming.iter())
        .map(|row| row.video_id.clone())
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

// ---- where it goes ---------------------------------------------------------------------------

/// What a menu is placed against.
#[derive(Clone, Debug)]
pub enum Anchor {
    /// The pointer (a right-click), in window coordinates.
    Point(Point<Pixels>),
    /// An element that recorded its bounds under this key (see `ui::anchor`): the More button.
    Key(SharedString),
}

/// The anchor with its place on screen worked out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placed {
    Point(Point<Pixels>),
    Below(Bounds<Pixels>),
}

/// The menu's top-left corner in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct At {
    pub left: f32,
    pub top: f32,
}

/// How tall a menu with `rows` rows in `groups` groups is.
pub fn menu_height(rows: usize, groups: usize) -> f32 {
    2.0 * (BORDER + PAD)
        + rows as f32 * ITEM_HEIGHT
        + groups.saturating_sub(1) as f32 * (SEPARATOR + 2.0 * PAD)
}

/// Where a menu of `size` goes. At the pointer it opens down and to the right; under a button it
/// lines its right edge up with the button's (the More button sits at a row's right end). When it
/// would run off the window it flips to the other side of the pointer or button, and it always
/// stays inside the window, clear of the title bar.
pub fn menu_position(placed: Placed, size: Size<Pixels>, window: Size<Pixels>) -> At {
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let (window_w, window_h) = (f32::from(window.width), f32::from(window.height));
    let min_top = layout::TITLE_BAR_HEIGHT + EDGE_MARGIN;
    let (left, top) = match placed {
        Placed::Point(p) => {
            let (x, y) = (f32::from(p.x), f32::from(p.y));
            let left = if x + w + EDGE_MARGIN <= window_w { x } else { x - w };
            let top = if y + h + EDGE_MARGIN <= window_h { y } else { y - h };
            (left, top)
        }
        Placed::Below(b) => {
            let right = f32::from(b.origin.x + b.size.width);
            let (anchor_top, anchor_bottom) = (f32::from(b.origin.y), f32::from(b.origin.y + b.size.height));
            let below = anchor_bottom + ANCHOR_GAP;
            let top = if below + h + EDGE_MARGIN <= window_h {
                below
            } else {
                anchor_top - ANCHOR_GAP - h
            };
            (right - w, top)
        }
    };
    At {
        left: left.clamp(EDGE_MARGIN, (window_w - w - EDGE_MARGIN).max(EDGE_MARGIN)),
        top: top.clamp(min_top, (window_h - h - EDGE_MARGIN).max(min_top)),
    }
}

// ---- opening it ------------------------------------------------------------------------------

/// How much of a collection a card command loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pages {
    /// Just the first page (Start radio only needs a seed).
    First,
    /// Every page, up to [`MAX_PAGES`] (Play, Shuffle and Add to queue).
    All,
}

/// The most pages one command follows; a collection longer than this is cut short.
const MAX_PAGES: usize = 30;

/// A collection's songs: the first page, and with [`Pages::All`] the ones after it. A later page
/// that fails ends the list there; only a failing first page is an error.
fn load_tracks(
    source: &dyn crate::library::LibrarySource,
    collection: &Collection,
    pages: Pages,
) -> Result<Vec<crate::library::source::TrackRow>, apollo_domain::listing::LoadFailure> {
    let first = source.tracks(collection)?;
    let mut rows = first.items;
    let mut next = first.continuation;
    let mut followed = 1;
    while let (Pages::All, Some(token)) = (pages, next.take()) {
        if followed >= MAX_PAGES {
            break;
        }
        let Ok(page) = source.more_tracks(&token) else {
            break;
        };
        followed += 1;
        rows.extend(page.items);
        next = page.continuation;
    }
    Ok(rows)
}

/// The shell's menu, reachable from any row.
#[derive(Clone)]
pub struct MenuHandle(pub Entity<TrackMenu>);

impl Global for MenuHandle {}

/// Open the menu for `subject`. Does nothing where there is no shell (a view on its own).
pub fn open(subject: Subject, anchor: Anchor, window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<MenuHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |menu, cx| menu.open(subject, anchor, window, cx));
}

/// Run one of the menu's commands on `subject` without opening it (a card's own Play button).
/// Does nothing where there is no shell.
pub fn run_now(command: Command, subject: Subject, window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<MenuHandle>().map(|h| h.0.clone()) else {
        return;
    };
    // Only Add to playlist reads the place; the commands run directly have no use for it.
    let nowhere = At { left: 0.0, top: 0.0 };
    handle.update(cx, |menu, cx| menu.run(command, subject, nowhere, window, cx));
}

/// Make right-clicking `el` open the menu at the pointer.
pub fn on_context_menu<E: InteractiveElement>(el: E, subject: Subject) -> E {
    el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
        cx.stop_propagation();
        open(subject.clone(), Anchor::Point(event.position), window, cx);
    })
}

/// Where the menu goes once a target is chosen (Go to artist, Go to album).
pub type OpenTarget = Rc<dyn Fn(Target, &mut Window, &mut App)>;

/// What Remove from playlist calls: the playlist's collection and the row's token.
pub type RemoveRow = Rc<dyn Fn(&Collection, &str, &mut App)>;

/// What Move to top / Move to bottom call: the playlist's collection, the tokens of the rows to
/// move (in list order) and the end they go to.
pub type ArrangeRows = Rc<dyn Fn(&Collection, &[String], Edge, &mut App)>;

struct Open {
    subject: Subject,
    placed: Placed,
    cursor: usize,
    /// Where keyboard focus returns to when the menu closes.
    return_focus: Option<FocusHandle>,
}

pub struct TrackMenu {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    edits: Entity<LibraryEdits>,
    on_open: OpenTarget,
    on_remove_row: RemoveRow,
    on_arrange: ArrangeRows,
    focus: FocusHandle,
    open: Option<Open>,
}

/// What a failed radio or collection load says: what happened in six words or fewer.
pub const RADIO_FAILED: &str = "Couldn't start radio";
pub const LOAD_FAILED: &str = "Couldn't load tracks";

impl TrackMenu {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        edits: Entity<LibraryEdits>,
        on_open: impl Fn(Target, &mut Window, &mut App) + 'static,
        on_remove_row: impl Fn(&Collection, &str, &mut App) + 'static,
        on_arrange: impl Fn(&Collection, &[String], Edge, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.observe(&edits, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            writes,
            picker,
            edits,
            on_open: Rc::new(on_open),
            on_remove_row: Rc::new(on_remove_row),
            on_arrange: Rc::new(on_arrange),
            focus: cx.focus_handle(),
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn subject(&self) -> Option<&Subject> {
        self.open.as_ref().map(|o| &o.subject)
    }

    pub fn cursor(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.cursor)
    }

    /// The commands on offer, in order.
    pub fn commands(&self, cx: &App) -> Vec<Command> {
        self.items(cx).iter().map(|i| i.command).collect()
    }

    /// The commands on offer with the words each is shown with, in order.
    pub fn labels(&self, cx: &App) -> Vec<(Command, &'static str)> {
        self.items(cx).iter().map(|i| (i.command, i.label)).collect()
    }

    fn groups(&self, cx: &App) -> Vec<Vec<Item>> {
        let Some(open) = self.open.as_ref() else {
            return Vec::new();
        };
        let blocked = |artist: Option<BlockedArtist>| {
            artist.is_some_and(|a| {
                crate::blocked_artists::handle(cx).is_some_and(|b| b.read(cx).is_blocked(&a))
            })
        };
        let (liked, facts) = match &open.subject {
            Subject::Track(t) => (
                self.writes.read(cx).is_liked(t.track.video_id.as_str()),
                Facts {
                    in_crate: crate::crate_list::handle(cx)
                        .is_some_and(|c| c.read(cx).contains(t.track.video_id.as_str())),
                    song_saved: self
                        .edits
                        .read(cx)
                        .song_saved(t.track.video_id.as_str(), cx),
                    artist_blocked: blocked(blocked_artist_of(&t.track)),
                    ..Facts::default()
                },
            ),
            Subject::Artist(a) => (
                false,
                Facts {
                    artist_blocked: blocked(Some(a.blocked())),
                    ..Facts::default()
                },
            ),
            Subject::Collection(c) => {
                let edits = self.edits.read(cx);
                (
                    false,
                    Facts {
                        own_playlist: edits.is_editable(c),
                        page_open: cx
                            .try_global::<duplicates_dialog::DuplicatesDialogHandle>()
                            .is_some_and(|h| h.0.read(cx).can_open(c, cx)),
                        saved: edits.saved_state(c),
                        ..Facts::default()
                    },
                )
            }
            Subject::Tracks(songs) => (
                songs
                    .iter()
                    .all(|t| self.writes.read(cx).is_liked(t.track.video_id.as_str())),
                Facts {
                    in_crate: crate::crate_list::handle(cx).is_some_and(|c| {
                        let crate_ = c.read(cx);
                        songs.iter().all(|t| crate_.contains(t.track.video_id.as_str()))
                    }),
                    ..Facts::default()
                },
            ),
            Subject::Queue => (false, Facts::default()),
        };
        groups_with(&open.subject, liked, facts)
    }

    fn items(&self, cx: &App) -> Vec<Item> {
        flat(&self.groups(cx))
    }

    pub fn open(
        &mut self,
        subject: Subject,
        anchor: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placed = match anchor {
            Anchor::Point(p) => Placed::Point(p),
            Anchor::Key(key) => match anchor::bounds_of(&key, cx) {
                Some(bounds) => Placed::Below(bounds),
                None => {
                    let size = window.viewport_size();
                    Placed::Point(point(size.width / 2.0, size.height / 2.0))
                }
            },
        };
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        self.open = Some(Open {
            subject,
            placed,
            cursor: 0,
            return_focus,
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Close without touching focus: the account changed under the menu.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.take_closed(window, cx);
    }

    fn take_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Open> {
        let open = self.open.take()?;
        match &open.return_focus {
            Some(handle) => window.focus(handle, cx),
            None => window.focus(&self.focus, cx),
        }
        cx.notify();
        Some(open)
    }

    fn move_cursor(&mut self, to: isize, cx: &mut Context<Self>) {
        let len = self.items(cx).len() as isize;
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if len == 0 {
            return;
        }
        open.cursor = to.clamp(0, len - 1) as usize;
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let at = self.open.as_ref().map(|o| o.cursor as isize);
        if let Some(at) = at {
            self.move_cursor(at + delta, cx);
        }
    }

    /// Run the command at `index` and close.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.items(cx);
        let Some(item) = items.get(index).copied() else {
            return;
        };
        let size = self.size(cx);
        let Some(open) = self.take_closed(window, cx) else {
            return;
        };
        let at = menu_position(open.placed, size, window.viewport_size());
        self.run(item.command, open.subject, at, window, cx);
    }

    fn size(&self, cx: &App) -> Size<Pixels> {
        let groups = self.groups(cx);
        let rows = groups.iter().map(Vec::len).sum();
        gpui::size(
            px(MENU_WIDTH),
            px(menu_height(rows, groups.len())),
        )
    }

    fn run(
        &mut self,
        command: Command,
        subject: Subject,
        at: At,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (command, subject) {
            (Command::PlayNext, Subject::Track(t)) => {
                self.player.update(cx, |p, cx| {
                    p.hint_art(t.track.video_id.as_str(), t.thumbnail.as_deref());
                    p.play_next(t.track.clone(), cx);
                });
            }
            (Command::AddToQueue, Subject::Track(t)) => {
                self.player.update(cx, |p, cx| {
                    p.hint_art(t.track.video_id.as_str(), t.thumbnail.as_deref());
                    p.add_to_queue(t.track.clone(), cx);
                });
            }
            (Command::AddToQueue, Subject::Collection(c)) => {
                self.with_tracks(c, Pages::All, |menu, rows, cx| {
                    let tracks = menu.hinted(rows, cx);
                    menu.player.update(cx, |p, cx| p.add_tracks_to_queue(tracks, cx));
                }, cx);
            }
            (Command::Play, Subject::Collection(c)) => {
                let source = Some(queue_source::of_collection(&c));
                self.with_tracks(c, Pages::All, move |menu, rows, cx| {
                    let tracks = menu.hinted(rows, cx);
                    menu.player
                        .update(cx, |p, cx| p.play_tracks_from(tracks, Some(0), None, source, cx));
                }, cx);
            }
            (Command::Shuffle, Subject::Collection(c)) => {
                let source = Some(queue_source::of_collection(&c));
                self.with_tracks(c, Pages::All, move |menu, rows, cx| {
                    let tracks = menu.hinted(rows, cx);
                    menu.player
                        .update(cx, |p, cx| p.play_tracks_from(tracks, None, Some(true), source, cx));
                }, cx);
            }
            (Command::PlayNext, Subject::Tracks(songs)) => {
                let tracks = self.hinted_songs(songs, cx);
                self.player.update(cx, |p, cx| p.play_next_tracks(tracks, cx));
            }
            (Command::AddToQueue, Subject::Tracks(songs)) => {
                let tracks = self.hinted_songs(songs, cx);
                self.player.update(cx, |p, cx| p.add_tracks_to_queue(tracks, cx));
            }
            (Command::AddToPlaylist, Subject::Tracks(songs)) => {
                // The picker opens where the menu was, for all of them, in list order.
                let anchor = Bounds::new(
                    point(px(at.left), px(at.top - ANCHOR_GAP)),
                    gpui::size(px(PANEL_WIDTH), px(0.0)),
                );
                let mut seen = std::collections::HashSet::new();
                let video_ids: Vec<String> = songs
                    .iter()
                    .map(|s| s.track.video_id.as_str().to_owned())
                    .filter(|id| seen.insert(id.clone()))
                    .collect();
                let title = format!("{} songs", video_ids.len());
                self.picker.update(cx, |p, cx| {
                    p.open_many(video_ids, title, Some(anchor), window, cx)
                });
            }
            (Command::AddToCrate, Subject::Tracks(songs)) => {
                for song in songs {
                    crate::crate_list::add_track(&self.player, song.track, song.thumbnail, cx);
                }
            }
            (Command::ToggleLike, Subject::Tracks(songs)) => {
                // Like them all, or (when they all are liked already) take them all out. The
                // writes go out one after another, not all at once.
                let all_liked = songs
                    .iter()
                    .all(|t| self.writes.read(cx).is_liked(t.track.video_id.as_str()));
                let ids = songs
                    .iter()
                    .map(|t| t.track.video_id.as_str().to_owned())
                    .collect();
                self.writes
                    .update(cx, |w, cx| w.set_liked_many(ids, !all_liked, cx));
            }
            (Command::RemoveFromCrate, Subject::Tracks(songs)) => {
                if let Some(model) = crate::crate_list::handle(cx) {
                    model.update(cx, |m, cx| {
                        for song in &songs {
                            m.remove(song.track.video_id.as_str(), cx);
                        }
                    });
                }
            }
            (Command::RemoveFromQueue, Subject::Tracks(songs)) => {
                // One step for Ctrl+Z, whatever the count. The playing row has no entry here.
                let entries: Vec<_> = songs.iter().filter_map(|s| s.entry).collect();
                self.player.update(cx, |p, cx| p.remove_entries(entries, cx));
            }
            (Command::RemoveFromPlaylist, Subject::Tracks(songs)) => {
                // The playlist page's own edit queue sends the removals one at a time.
                for row in songs.into_iter().filter_map(|s| s.playlist_row) {
                    (self.on_remove_row.clone())(&row.collection, &row.occurrence, cx);
                }
            }
            (Command::MoveToTop | Command::MoveToBottom, Subject::Tracks(songs)) => {
                let edge = if command == Command::MoveToTop { Edge::Top } else { Edge::Bottom };
                // One playlist page: the rows' own tokens, in list order.
                let rows: Vec<PlaylistRow> = songs.into_iter().filter_map(|s| s.playlist_row).collect();
                if let Some(first) = rows.first() {
                    let tokens: Vec<String> = rows.iter().map(|r| r.occurrence.clone()).collect();
                    (self.on_arrange.clone())(&first.collection, &tokens, edge, cx);
                }
            }
            (Command::MoveToTop | Command::MoveToBottom, Subject::Track(t)) => {
                let edge = if command == Command::MoveToTop { Edge::Top } else { Edge::Bottom };
                if let Some(row) = t.playlist_row {
                    (self.on_arrange.clone())(&row.collection, &[row.occurrence], edge, cx);
                }
            }
            (Command::RemoveDuplicates, Subject::Collection(c)) => {
                duplicates_dialog::open(c, window, cx);
            }
            (Command::StartRadio, Subject::Track(t)) => {
                let name = t.track.title.clone();
                self.start_radio(t.track, t.thumbnail, name, cx);
            }
            (Command::StartRadio, Subject::Collection(c)) => {
                // A collection's radio is seeded from its first song.
                let name = c.title.clone();
                self.with_tracks(c, Pages::First, move |menu, rows, cx| {
                    if let Some(first) = rows.into_iter().next() {
                        menu.start_radio(first.track, first.thumbnail, name, cx);
                    }
                }, cx);
            }
            (Command::AddToPlaylist, Subject::Track(t)) => {
                // The picker opens where the menu was.
                let anchor = Bounds::new(
                    point(px(at.left), px(at.top - ANCHOR_GAP)),
                    gpui::size(px(PANEL_WIDTH), px(0.0)),
                );
                let (video, title) = (t.track.video_id.as_str().to_owned(), t.track.title);
                self.picker.update(cx, |p, cx| {
                    p.open_with(&video, title, Some(anchor), window, cx)
                });
            }
            (Command::ToggleLike, Subject::Track(t)) => {
                let video = t.track.video_id.as_str().to_owned();
                self.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
            }
            (Command::GoToArtist, Subject::Track(t)) => {
                if let Some(ArtistRef {
                    name,
                    id: Some(id),
                }) = artist_of(&t.track).cloned()
                {
                    let target = Target::Artist(ArtistTarget {
                        id,
                        name,
                        thumbnail: None,
                    });
                    (self.on_open.clone())(target, window, cx);
                }
            }
            (Command::GoToAlbum, Subject::Track(t)) => {
                if let Some(AlbumRef {
                    title,
                    id: Some(id),
                }) = album_of(&t.track).cloned()
                {
                    let target = Target::Collection(Collection {
                        kind: CollectionKind::Album,
                        id,
                        title,
                        subtitle: None,
                        thumbnail: t.thumbnail.clone(),
                    });
                    (self.on_open.clone())(target, window, cx);
                }
            }
            (Command::CopyLink, subject) => {
                cx.write_to_clipboard(ClipboardItem::new_string(link_of(&subject)));
            }
            (Command::NotInterested, Subject::Track(t)) => {
                let video = t.track.video_id.as_str().to_owned();
                self.writes.update(cx, |w, cx| w.not_interested(&video, cx));
            }
            (Command::CopyTitleArtist, Subject::Track(t)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(title_and_artist(&t.track)));
            }
            (Command::OpenOnYoutubeMusic, Subject::Track(t)) => {
                cx.open_url(&links::track_url(&t.track.video_id));
            }
            (Command::BlockArtist, subject) => {
                let artist = match &subject {
                    Subject::Track(t) => blocked_artist_of(&t.track),
                    Subject::Artist(a) => Some(a.blocked()),
                    Subject::Collection(_) | Subject::Queue | Subject::Tracks(_) => None,
                };
                if let (Some(artist), Some(model)) = (artist, crate::blocked_artists::handle(cx)) {
                    model.update(cx, |m, cx| {
                        // The same item blocks or lets back, whichever the artist needs.
                        if !m.unblock(&artist, cx) {
                            m.block(artist, cx);
                        }
                    });
                }
            }
            (Command::AddToCrate, Subject::Track(t)) => {
                crate::crate_list::add_track(&self.player, t.track, t.thumbnail, cx);
            }
            (Command::RemoveFromCrate, Subject::Track(t)) => {
                if let Some(model) = crate::crate_list::handle(cx) {
                    model.update(cx, |m, cx| {
                        m.remove(t.track.video_id.as_str(), cx);
                    });
                }
            }
            (Command::RemoveFromQueue, Subject::Track(t)) => {
                if let Some(entry) = t.entry {
                    self.player.update(cx, |p, cx| p.remove_entry(entry, cx));
                }
            }
            (Command::RemoveFromPlaylist, Subject::Track(t)) => {
                if let Some(row) = t.playlist_row {
                    (self.on_remove_row.clone())(&row.collection, &row.occurrence, cx);
                }
            }
            (Command::SaveToLibrary, Subject::Collection(c)) => {
                self.edits.update(cx, |e, cx| e.set_saved(c, true, cx));
            }
            (Command::RemoveFromLibrary, Subject::Collection(c)) => {
                self.edits.update(cx, |e, cx| e.set_saved(c, false, cx));
            }
            (Command::SaveToLibrary, Subject::Track(t)) => {
                let video = t.track.video_id.as_str().to_owned();
                self.edits.update(cx, |e, cx| e.set_song_saved(video, true, cx));
            }
            (Command::RemoveFromLibrary, Subject::Track(t)) => {
                let video = t.track.video_id.as_str().to_owned();
                self.edits.update(cx, |e, cx| e.set_song_saved(video, false, cx));
            }
            (Command::SaveQueue, Subject::Queue) => {
                let view = self.player.read(cx).queue_view();
                let video_ids = queue_video_ids(&view);
                if !video_ids.is_empty() {
                    let name = queue_playlist_name(view.heading.as_ref());
                    playlist_dialog::open(Mode::FromQueue { name, video_ids }, window, cx);
                }
            }
            (Command::EditPlaylist, Subject::Collection(c)) => {
                playlist_dialog::open(Mode::Edit(c), window, cx);
            }
            (Command::DeletePlaylist, Subject::Collection(c)) => {
                playlist_dialog::open(Mode::Delete(c), window, cx);
            }
            _ => {}
        }
    }

    /// Hand a selection's covers to the player and keep the tracks, in the selection's order.
    fn hinted_songs(&self, songs: Vec<TrackSubject>, cx: &mut Context<Self>) -> Vec<Track> {
        self.player.update(cx, |p, _| {
            for song in &songs {
                p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
            }
        });
        songs.into_iter().map(|s| s.track).collect()
    }

    /// Hand the rows' covers to the player and keep the tracks.
    fn hinted(&self, rows: Vec<crate::library::source::TrackRow>, cx: &mut Context<Self>) -> Vec<Track> {
        self.player.update(cx, |p, _| {
            for row in &rows {
                p.hint_art(row.track.video_id.as_str(), row.thumbnail.as_deref());
            }
        });
        rows.into_iter().map(|r| r.track).collect()
    }

    /// Load the first page of a collection's tracks off the UI thread, then hand it to `then`.
    /// A collection that cannot be read leaves the error toast.
    fn with_tracks(
        &mut self,
        collection: Collection,
        pages: Pages,
        then: impl FnOnce(&mut Self, Vec<crate::library::source::TrackRow>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let source = self.player.read(cx).library_source();
        cx.spawn(async move |this, cx| {
            let result = match source {
                Some(source) => {
                    cx.background_executor()
                        .spawn(async move { load_tracks(&*source, &collection, pages) })
                        .await
                }
                None => Err(apollo_domain::listing::LoadFailure::Other),
            };
            let _ = this.update(cx, |menu, cx| match result {
                Ok(rows) if !rows.is_empty() => then(menu, rows, cx),
                _ => menu.writes.update(cx, |w, cx| w.raise(LOAD_FAILED, cx)),
            });
        })
        .detach();
    }

    /// Play `seed` now; its radio replaces the up-next list when it arrives.
    fn start_radio(
        &mut self,
        seed: Track,
        thumbnail: Option<String>,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let video = seed.video_id.as_str().to_owned();
        self.player.update(cx, |p, cx| {
            p.hint_art(&video, thumbnail.as_deref());
            p.start_radio(seed, cx);
        });
        let source = self.player.read(cx).library_source();
        cx.spawn(async move |this, cx| {
            let result = match source {
                Some(source) => {
                    let video = video.clone();
                    cx.background_executor()
                        .spawn(async move { source.radio(&video) })
                        .await
                }
                None => Err(apollo_domain::listing::LoadFailure::Other),
            };
            let _ = this.update(cx, |menu, cx| match result {
                Ok(rows) if !rows.is_empty() => {
                    let tracks = menu.hinted(rows, cx);
                    menu.player
                        .update(cx, |p, cx| p.finish_radio(tracks, queue_source::radio(name), cx));
                }
                _ => menu.writes.update(cx, |w, cx| w.raise(RADIO_FAILED, cx)),
            });
        })
        .detach();
    }
}

impl Focusable for TrackMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TrackMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let groups = self.groups(cx);
        let cursor = open.cursor;
        let size = self.size(cx);
        let at = menu_position(open.placed, size, window.viewport_size());

        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut ix = 0usize;
        for (g, group) in groups.iter().enumerate() {
            if g > 0 {
                body.push(
                    div()
                        .flex_none()
                        .h(px(SEPARATOR))
                        .mx(px(PAD))
                        .my(px(PAD))
                        .bg(color::border_subtle())
                        .into_any_element(),
                );
            }
            for it in group {
                body.push(menu_item(ix, *it, ix == cursor, cx));
                ix += 1;
            }
        }

        let panel = div()
            .id("track-menu")
            .debug_selector(|| "track-menu".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Menu)
            .flex()
            .flex_col()
            .w(px(MENU_WIDTH))
            .p(px(PAD))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the menu are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|m, _: &MenuUp, _, cx| m.step(-1, cx)))
            .on_action(cx.listener(|m, _: &MenuDown, _, cx| m.step(1, cx)))
            .on_action(cx.listener(|m, _: &MenuFirst, _, cx| m.move_cursor(0, cx)))
            .on_action(cx.listener(|m, _: &MenuLast, _, cx| m.move_cursor(isize::MAX, cx)))
            .on_action(cx.listener(|m, _: &MenuChoose, window, cx| {
                if let Some(ix) = m.cursor() {
                    m.choose(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|m, _: &MenuClose, window, cx| m.close(window, cx)))
            .on_action(cx.listener(|_, _: &MenuStay, _, _| {}))
            .children(body);

        // A transparent layer under the menu catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("track-menu-layer")
            .debug_selector(|| "track-menu-layer".into())
            .absolute()
            .top(px(layout::TITLE_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(at.left))
                    .top(px(at.top - layout::TITLE_BAR_HEIGHT))
                    .child(panel),
            )
            .into_any_element()
    }
}

/// The command's entry in `shortcuts`, when it has a key.
fn shortcut_of(command: Command) -> Option<&'static str> {
    match command {
        Command::ToggleLike => Some("like"),
        Command::AddToCrate => Some("crate"),
        Command::AddToPlaylist => Some("playlist"),
        Command::RemoveFromQueue | Command::RemoveFromCrate => Some("remove"),
        _ => None,
    }
}

fn menu_item(ix: usize, it: Item, cursor: bool, cx: &mut Context<TrackMenu>) -> gpui::AnyElement {
    let shortcut = shortcut_of(it.command);
    let key = shortcut.and_then(|id| shortcuts::menu_key(id, cx));
    style::text_style(div(), typography::BODY, cx)
        .id(("track-menu-item", ix))
        .debug_selector(move || format!("track-menu-item-{ix}"))
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ITEM_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .role(gpui::Role::MenuItem)
        .aria_label(it.label)
        .text_color(color::text_primary())
        .bg(if cursor {
            color::state_hover()
        } else {
            gpui::transparent_black()
        })
        .on_hover(cx.listener(move |m, hovered: &bool, _, cx| {
            if *hovered {
                m.move_cursor(ix as isize, cx);
            }
        }))
        .on_click(cx.listener(move |m, event: &gpui::ClickEvent, window, cx| {
            m.choose(ix, window, cx);
            if let (Some(id), gpui::ClickEvent::Mouse(_)) = (shortcut, event) {
                shortcuts::note_mouse_use(id, Scope::Any, cx);
            }
        }))
        .child(
            gpui::svg()
                .path(it.icon.path())
                .size(px(ICON))
                .flex_none()
                .text_color(color::text_secondary()),
        )
        .child(div().flex_1().min_w_0().truncate().child(it.label))
        .children(key.map(|key| {
            style::text_style(div(), typography::CAPTION, cx)
                .debug_selector(move || format!("track-menu-key-{ix}"))
                .flex_none()
                .text_color(color::text_secondary())
                .child(key)
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::model::BrowseId;
    use gpui::size;

    fn song(artist: bool, album: bool) -> Track {
        let mut t = Track::new("dQw4w9WgXcQ", "Song");
        if artist {
            t.artists = vec![ArtistRef {
                name: "Artist".into(),
                id: Some(BrowseId::new("UCx")),
            }];
        }
        if album {
            t.album = Some(AlbumRef {
                title: "Album".into(),
                id: Some(BrowseId::new("MPREx")),
            });
        }
        t
    }

    fn commands(subject: &Subject, liked: bool) -> Vec<Vec<Command>> {
        groups(subject, liked)
            .iter()
            .map(|g| g.iter().map(|i| i.command).collect())
            .collect()
    }

    #[test]
    fn a_song_menu_has_the_six_groups_in_order() {
        use Command::*;
        let subject = Subject::track(song(true, true), None);
        assert_eq!(
            commands(&subject, false),
            vec![
                vec![PlayNext, AddToQueue],
                vec![StartRadio, AddToPlaylist, AddToCrate],
                vec![ToggleLike],
                vec![GoToArtist, GoToAlbum, BlockArtist],
                vec![NotInterested],
                vec![CopyLink, CopyTitleArtist, OpenOnYoutubeMusic],
            ]
        );
    }

    #[test]
    fn not_interested_sits_alone_just_above_the_final_group_of_copies_and_open() {
        let groups = commands(&Subject::track(song(true, true), None), false);
        let [.., not_interested, last] = &groups[..] else {
            panic!("a song menu has groups");
        };
        assert_eq!(not_interested, &vec![Command::NotInterested]);
        assert_eq!(
            last,
            &vec![
                Command::CopyLink,
                Command::CopyTitleArtist,
                Command::OpenOnYoutubeMusic
            ]
        );
    }

    #[test]
    fn the_block_item_names_what_it_will_do() {
        let subject = Subject::track(song(true, false), None);
        let label = |blocked| {
            groups_with(&subject, false, Facts { artist_blocked: blocked, ..Facts::default() })[3]
                .iter()
                .find(|i| i.command == Command::BlockArtist)
                .map(|i| i.label)
        };
        assert_eq!(label(false), Some("Don't play this artist"));
        assert_eq!(label(true), Some("Play this artist again"));
        // No page to block, no item.
        let nameless = commands(&Subject::track(song(false, true), None), false);
        assert!(!nameless.iter().flatten().any(|c| *c == Command::BlockArtist));
    }

    #[test]
    fn an_artist_has_a_menu_of_just_the_block_item() {
        let artist = Subject::Artist(ArtistSubject {
            id: BrowseId::new("UCx"),
            name: "Artist".into(),
        });
        assert_eq!(commands(&artist, false), vec![vec![Command::BlockArtist]]);
        assert_eq!(link_of(&artist), "https://music.youtube.com/channel/UCx");
    }

    #[test]
    fn copy_title_and_artist_reads_title_dash_artists() {
        let mut t = song(true, false);
        t.title = "Song".into();
        t.artists.push(ArtistRef {
            name: "Guest".into(),
            id: None,
        });
        assert_eq!(title_and_artist(&t), "Song - Artist, Guest");
        t.artists.clear();
        assert_eq!(title_and_artist(&t), "Song", "no artist, no dash");
    }

    #[test]
    fn go_to_artist_and_album_hide_when_they_are_not_known() {
        use Command::*;
        let only_album = commands(&Subject::track(song(false, true), None), false);
        assert_eq!(only_album[3], vec![GoToAlbum]);
        let only_artist = commands(&Subject::track(song(true, false), None), false);
        assert_eq!(only_artist[3], vec![GoToArtist, BlockArtist]);
        // Neither known: the group is gone, not empty.
        let neither = commands(&Subject::track(song(false, false), None), false);
        assert_eq!(neither.len(), 5);
        assert_eq!(neither[3], vec![NotInterested]);
        // A name without an id has no page to go to.
        let mut t = song(false, false);
        t.artists = vec![ArtistRef {
            name: "Nameless".into(),
            id: None,
        }];
        assert_eq!(commands(&Subject::track(t, None), false).len(), 5);
    }

    #[test]
    fn the_heart_item_follows_the_like_state() {
        let subject = Subject::track(song(false, false), None);
        let label = |liked| groups(&subject, liked)[2][0].label;
        assert_eq!(label(false), "Like");
        assert_eq!(label(true), "Remove from Liked");
    }

    #[test]
    fn only_a_queue_row_can_be_removed_from_the_queue() {
        use Command::*;
        let plain = commands(&Subject::track(song(true, true), None), false);
        assert!(!plain.iter().flatten().any(|c| *c == RemoveFromQueue));
        let queued = commands(&Subject::queued(song(true, true), QueueEntryId::from_raw(3)), false);
        assert_eq!(queued.last().unwrap(), &vec![RemoveFromQueue]);
    }

    #[test]
    fn a_card_menu_is_the_shorter_one() {
        use Command::*;
        let c = Collection {
            kind: CollectionKind::Album,
            id: BrowseId::new("MPREx"),
            title: "Album".into(),
            subtitle: None,
            thumbnail: None,
        };
        assert_eq!(
            commands(&Subject::Collection(c), false),
            vec![
                vec![Play, Shuffle],
                vec![AddToQueue, StartRadio],
                vec![CopyLink],
            ]
        );
    }

    fn card() -> Subject {
        Subject::Collection(Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new("VLPLx"),
            title: "Mine".into(),
            subtitle: None,
            thumbnail: None,
        })
    }

    fn commands_with(subject: &Subject, facts: Facts) -> Vec<Vec<Command>> {
        groups_with(subject, false, facts)
            .iter()
            .map(|g| g.iter().map(|i| i.command).collect())
            .collect()
    }

    #[test]
    fn your_own_playlist_card_ends_with_rename_duplicates_and_delete() {
        use Command::*;
        let own = Facts {
            own_playlist: true,
            saved: None,
            ..Facts::default()
        };
        assert_eq!(
            commands_with(&card(), own)[3],
            vec![EditPlaylist, DeletePlaylist],
            "a card that is not the open page has no Remove duplicates"
        );
        let on_its_page = Facts { page_open: true, ..own };
        assert_eq!(
            commands_with(&card(), on_its_page),
            vec![
                vec![Play, Shuffle],
                vec![AddToQueue, StartRadio],
                vec![CopyLink],
                vec![EditPlaylist, RemoveDuplicates, DeletePlaylist],
            ]
        );
    }

    #[test]
    fn a_card_offers_the_library_toggle_that_matches_where_it_is() {
        use Command::*;
        let saved = |saved| Facts {
            own_playlist: false,
            saved: Some(saved),
            ..Facts::default()
        };
        assert_eq!(
            commands_with(&card(), saved(false))[2],
            vec![SaveToLibrary],
            "not in the library: save it"
        );
        assert_eq!(
            commands_with(&card(), saved(true))[2],
            vec![RemoveFromLibrary],
            "in the library: take it out"
        );
        // Never both, and never Rename or Delete for what is not yours.
        let all: Vec<Command> = commands_with(&card(), saved(true)).into_iter().flatten().collect();
        assert!(!all.contains(&SaveToLibrary) && !all.contains(&EditPlaylist) && !all.contains(&DeletePlaylist));
    }

    #[test]
    fn a_song_of_your_own_playlist_can_be_removed_from_it_and_others_cannot() {
        use Command::*;
        let Subject::Collection(playlist) = card() else {
            unreachable!()
        };
        let in_playlist =
            Subject::playlist_track(song(true, true), None, playlist, "setVideoId1".into());
        let groups = commands(&in_playlist, false);
        assert_eq!(groups.last().unwrap(), &vec![RemoveFromPlaylist]);
        // ... and sent to either end of it, in a group of their own just above.
        assert_eq!(groups[groups.len() - 2], vec![MoveToTop, MoveToBottom]);
        let plain = commands(&Subject::track(song(true, true), None), false);
        assert!(!plain.iter().flatten().any(|c| *c == RemoveFromPlaylist));
        assert!(!plain.iter().flatten().any(|c| matches!(c, MoveToTop | MoveToBottom)));
        // A queue row is removed from the queue, not from a playlist.
        let queued = commands(&Subject::queued(song(true, true), QueueEntryId::from_raw(3)), false);
        assert!(!queued.iter().flatten().any(|c| *c == RemoveFromPlaylist));
    }

    #[test]
    fn play_and_queue_follow_every_page_but_a_radio_needs_only_the_first() {
        use crate::library::sim::{SimLibrary, LONG_ID, LONG_LEN};
        let sim = SimLibrary::instant();
        let long = Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new(LONG_ID),
            title: "Long Haul".into(),
            subtitle: None,
            thumbnail: None,
        };
        let all = load_tracks(&sim, &long, Pages::All).unwrap();
        assert_eq!(all.len(), LONG_LEN);
        let first = load_tracks(&sim, &long, Pages::First).unwrap();
        assert!(!first.is_empty() && first.len() < LONG_LEN);
    }

    #[test]
    fn an_artist_card_has_no_menu() {
        let artist = Target::Artist(ArtistTarget {
            id: BrowseId::new("UCx"),
            name: "A".into(),
            thumbnail: None,
        });
        assert_eq!(Subject::of_target(&artist), None);
    }

    #[test]
    fn a_song_links_to_its_watch_page() {
        assert_eq!(
            link_of(&Subject::track(song(false, false), None)),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn the_height_counts_rows_separators_padding_and_border() {
        assert_eq!(menu_height(2, 1), 2.0 * (BORDER + PAD) + 2.0 * ITEM_HEIGHT);
        assert_eq!(
            menu_height(5, 3),
            2.0 * (BORDER + PAD) + 5.0 * ITEM_HEIGHT + 2.0 * (SEPARATOR + 2.0 * PAD)
        );
    }

    fn pt(x: f32, y: f32) -> Placed {
        Placed::Point(point(px(x), px(y)))
    }

    fn menu() -> Size<Pixels> {
        size(px(MENU_WIDTH), px(menu_height(8, 5)))
    }

    #[test]
    fn at_the_pointer_the_menu_opens_down_and_right() {
        let at = menu_position(pt(300.0, 200.0), menu(), size(px(1200.0), px(800.0)));
        assert_eq!(at, At { left: 300.0, top: 200.0 });
    }

    #[test]
    fn near_the_right_and_bottom_edges_it_flips_to_the_other_side_of_the_pointer() {
        let window = size(px(1200.0), px(800.0));
        let h = f32::from(menu().height);
        let at = menu_position(pt(1150.0, 780.0), menu(), window);
        assert_eq!(at.left, 1150.0 - MENU_WIDTH);
        assert_eq!(at.top, 780.0 - h);
    }

    #[test]
    fn it_never_leaves_the_window_or_covers_the_title_bar() {
        let window = size(px(400.0), px(300.0));
        let at = menu_position(pt(5.0, 5.0), menu(), window);
        assert!(at.left >= EDGE_MARGIN);
        assert!(at.top >= layout::TITLE_BAR_HEIGHT);
        let at = menu_position(pt(390.0, 295.0), menu(), window);
        assert!(at.left + MENU_WIDTH <= 400.0);
        // Taller than the room: pinned under the title bar rather than pushed off the window.
        assert_eq!(at.top, layout::TITLE_BAR_HEIGHT + EDGE_MARGIN);
    }

    fn button(x: f32, y: f32) -> Placed {
        Placed::Below(Bounds::new(point(px(x), px(y)), size(px(32.0), px(32.0))))
    }

    #[test]
    fn under_a_button_it_lines_up_its_right_edge_and_sits_below() {
        let at = menu_position(button(900.0, 200.0), menu(), size(px(1200.0), px(800.0)));
        assert_eq!(at.left, 932.0 - MENU_WIDTH);
        assert_eq!(at.top, 232.0 + ANCHOR_GAP);
    }

    #[test]
    fn under_a_button_near_the_bottom_it_opens_above_it() {
        let window = size(px(1200.0), px(800.0));
        let h = f32::from(menu().height);
        let at = menu_position(button(900.0, 700.0), menu(), window);
        assert_eq!(at.top, 700.0 - ANCHOR_GAP - h);
    }

    fn picked(entry: bool) -> TrackSubject {
        TrackSubject {
            track: Track::new("v", "Song"),
            thumbnail: None,
            entry: entry.then(|| QueueEntryId::from_raw(1)),
            playlist_row: None,
        }
    }

    #[test]
    fn a_selection_menu_swaps_the_crate_item_when_all_are_in_it_and_adds_queue_removal_where_rows_have_entries() {
        let two = Subject::Tracks(vec![picked(false), picked(false)]);
        let flat: Vec<Command> = commands_with(&two, Facts::default()).concat();
        assert!(flat.contains(&Command::AddToCrate));
        assert!(!flat.contains(&Command::RemoveFromQueue));
        let in_crate = Facts {
            in_crate: true,
            ..Facts::default()
        };
        let flat: Vec<Command> = commands_with(&two, in_crate).concat();
        assert!(flat.contains(&Command::RemoveFromCrate) && !flat.contains(&Command::AddToCrate));

        // Queue rows carry an entry (the playing row has none): any one is enough to offer it.
        let queued = Subject::Tracks(vec![picked(true), picked(false)]);
        let groups = commands_with(&queued, Facts::default());
        assert_eq!(groups.last().unwrap(), &vec![Command::RemoveFromQueue]);
    }
}
