use std::time::Duration;

use apollo_domain::account::{IdentityId, Validation, ValidationError};
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track, VideoId};
use innertube::{
    AccountIdentity, AccountInfo, AlbumPage, PlaylistContinuation, PlaylistPage, SongItem,
};

/// A track as it appears in a listing: the playable identity plus the row's own occurrence token,
/// which is what repeated pages are deduplicated on (the same video may sit in a playlist twice).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedTrack {
    pub track: Track,
    pub occurrence: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

impl ListItem for ListedTrack {
    fn dedup_key(&self) -> Option<String> {
        self.occurrence.clone()
    }
}

/// `"3:47"` -> 227 s, `"1:02:03"` -> 3723 s. Anything else is unknown, never zero.
fn parse_duration(text: &str) -> Option<Duration> {
    let mut secs: u64 = 0;
    let mut parts = 0;
    for part in text.trim().split(':') {
        let n: u64 = part.parse().ok()?;
        secs = secs.checked_mul(60)?.checked_add(n)?;
        parts += 1;
    }
    (1..=3)
        .contains(&parts)
        .then(|| Duration::from_secs(secs))
        .filter(|d| !d.is_zero())
}

/// `None` when the row has no valid video id or no title: such a row cannot be played or shown.
pub fn track_from_song(song: &SongItem) -> Option<ListedTrack> {
    let video_id = VideoId::parse(&song.video_id)?;
    if song.title.trim().is_empty() {
        return None;
    }
    let artists = if song.artist_runs.is_empty() {
        let name = song.artists.trim();
        if name.is_empty() {
            Vec::new()
        } else {
            vec![ArtistRef {
                name: name.to_owned(),
                id: song.artist_id.clone().map(BrowseId::new),
            }]
        }
    } else {
        // Separators like " & " carry no id; only linked runs are artists.
        song.artist_runs
            .iter()
            .filter(|r| r.id.is_some())
            .map(|r| ArtistRef {
                name: r.text.clone(),
                id: r.id.clone().map(BrowseId::new),
            })
            .collect()
    };
    let album = song
        .album
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map(|title| AlbumRef {
            title: title.clone(),
            id: song.album_id.clone().map(BrowseId::new),
        });
    Some(ListedTrack {
        track: Track {
            video_id,
            title: song.title.clone(),
            artists,
            album,
            duration: song.duration.as_deref().and_then(parse_duration),
        },
        occurrence: song.occurrence_id.clone(),
        thumbnail: song.thumbnail.clone(),
        explicit: song.explicit,
    })
}

/// Maps rows and folds mapping failures into the page's `skipped` count, so a bad row is never
/// silently invisible.
fn page(
    songs: &[SongItem],
    continuation: Option<String>,
    parser_skipped: u32,
) -> Page<ListedTrack> {
    let mut items = Vec::with_capacity(songs.len());
    let mut skipped = parser_skipped;
    for song in songs {
        match track_from_song(song) {
            Some(t) => items.push(t),
            None => skipped = skipped.saturating_add(1),
        }
    }
    Page {
        items,
        continuation,
        skipped,
    }
}

pub fn page_from_playlist(p: &PlaylistPage) -> Page<ListedTrack> {
    page(&p.items, p.continuation.clone(), p.skipped)
}

pub fn page_from_album(p: &AlbumPage) -> Page<ListedTrack> {
    page(&p.items, p.continuation.clone(), p.skipped)
}

pub fn page_from_continuation(p: &PlaylistContinuation) -> Page<ListedTrack> {
    page(&p.items, p.continuation.clone(), p.skipped)
}

/// What a transport error means to a listing. Only a service-stated rejection is `Session`;
/// an outage, a throttle or an unreadable body is never a logout.
pub fn classify(err: &innertube::Error) -> LoadFailure {
    match err {
        innertube::Error::SessionExpired => LoadFailure::Session,
        innertube::Error::Json(_) => LoadFailure::Parse,
        innertube::Error::Http(e) => match e.status().map(|s| s.as_u16()) {
            Some(401 | 403) => LoadFailure::Session,
            Some(429) => LoadFailure::Throttled,
            Some(_) => LoadFailure::Network,
            None if e.is_decode() => LoadFailure::Parse,
            None => LoadFailure::Network,
        },
        innertube::Error::VisitorDataNotFound | innertube::Error::Other(_) => LoadFailure::Other,
    }
}

/// Turn the authenticated account probes into the account model's validation result.
///
/// `menu` is the `account_menu` answer and `identities` the `accounts_list` answer. Premium is
/// never inferred here: no maintained account signal is read, so `premium_signal` stays `None`.
/// An account menu with no identifying field at all gives no verdict (`Unreachable`) rather than
/// a logout: an unparsed response must not sign the owner out.
pub fn account_validation(
    menu: &AccountInfo,
    identities: &[AccountIdentity],
) -> Result<Validation, ValidationError> {
    let mut ids: Vec<IdentityId> = identities
        .iter()
        .map(|i| IdentityId::new(i.data_sync_id.clone()))
        .collect();
    ids.dedup();
    if ids.is_empty() {
        let identified = menu.name.is_some()
            || menu.handle.is_some()
            || menu.email.is_some()
            || menu.channel_id.is_some();
        if !identified {
            return Err(ValidationError::Unreachable);
        }
        // Single-account sessions carry no delegation token; the channel is the identity.
        if let Some(id) = menu.data_sync_id.as_ref().or(menu.channel_id.as_ref()) {
            ids.push(IdentityId::new(id.clone()));
        }
    }
    Ok(Validation {
        identities: ids,
        premium_signal: None,
    })
}

#[cfg(test)]
mod tests;
