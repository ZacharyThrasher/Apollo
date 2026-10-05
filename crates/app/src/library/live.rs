//! The library source backed by the real account: InnerTube (through `apollo-provider`) for the
//! lists and track pages, plain HTTP for cover art.
//!
//! Credentials stay with the account service. This adapter asks it for a snapshot, builds a
//! signed-in InnerTube session from the cookies, and hands rotated cookies back (the service
//! rotates `__Secure-*PSIDTS` on these very requests and invalidates the old value, so dropping
//! the new one would eventually sign the account out). Nothing here logs cookies, ids or titles.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_domain::library::{
    AddedRow, Collection, CollectionKind, PageMeta, PlaylistEdit, PlaylistTarget, Privacy, WriteFailure,
};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::BrowseId;
use apollo_provider::{
    ExplorePage, HistoryDay, HomePage, HomeShelf, Hit, InnerTubeSource, LibraryMarks, ListedTrack,
};
use apollo_services_common::{unix_now, SecretString, SessionProvider};
use innertube::{Locale, Session};

use super::source::{ArtistContent, LibrarySource, TrackRow, PROBE_VIDEO};
use crate::art::DecodedArt;

const ART_TIMEOUT: Duration = Duration::from_secs(10);
const ART_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The channel a multi-channel login acts as (`None` for a single-account login).
pub type DataSyncId = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// A signed-in InnerTube session and what it was built from. A new login, logout or channel
/// choice changes either, and the next request builds a fresh one.
struct Built {
    epoch: u64,
    data_sync_id: Option<String>,
    source: InnerTubeSource,
}

pub struct InnerTubeLibrary {
    rt: tokio::runtime::Runtime,
    session: Arc<dyn SessionProvider>,
    data_sync_id: DataSyncId,
    built: Mutex<Option<Built>>,
    agent: ureq::Agent,
    /// What rows said about saving songs to the library: shared with the Search source, and
    /// emptied when the account changes (a token belongs to the session that was given it).
    marks: Arc<LibraryMarks>,
}

impl InnerTubeLibrary {
    /// Nothing is read from the account until the first request, so this works signed out.
    pub fn new(
        session: Arc<dyn SessionProvider>,
        data_sync_id: DataSyncId,
        marks: Arc<LibraryMarks>,
    ) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("apollo-library")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(ART_TIMEOUT))
            .build()
            .into();
        Ok(Self {
            rt,
            session,
            data_sync_id,
            built: Mutex::new(None),
            agent,
            marks,
        })
    }

    /// The session for the current account, built on first use and after the account changes.
    /// `Session` failure when nobody is signed in.
    fn source(&self) -> Result<InnerTubeSource, LoadFailure> {
        let snapshot = self.session.snapshot().ok_or(LoadFailure::Session)?;
        let data_sync_id = (self.data_sync_id)();
        let mut built = self.built.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = built.as_ref() {
            if b.epoch == snapshot.epoch && b.data_sync_id == data_sync_id {
                return Ok(b.source.clone());
            }
        }
        let cookie = cookie_header(snapshot.cookies_netscape.expose(), unix_now())
            .ok_or(LoadFailure::Session)?;
        // A different session or channel: nothing noted for the old one applies.
        self.marks.clear();
        let source = InnerTubeSource::new(
            Session {
                locale: Locale::default(),
                visitor_data: None,
                data_sync_id: data_sync_id.clone(),
                cookie: Some(cookie.clone()),
            },
            None,
        )
        .map_err(|_| LoadFailure::Other)?
        .with_marks(self.marks.clone());
        self.spawn_rotation(&source, cookie, snapshot.epoch);
        *built = Some(Built {
            epoch: snapshot.epoch,
            data_sync_id,
            source: source.clone(),
        });
        Ok(source)
    }

    /// Rotated cookies go back to the account service, as long as the session they came from
    /// is still current (the service rotates `__Secure-*PSIDTS` on these very requests and
    /// invalidates the old value, so dropping the new one would eventually sign the account out).
    fn spawn_rotation(&self, source: &InnerTubeSource, sent: String, epoch: u64) {
        let rotated = source.cookie_changed();
        let reader = source.clone();
        let session = self.session.clone();
        self.rt.spawn(async move {
            loop {
                rotated.notified().await;
                let (Some(header), Some(now)) = (reader.cookie(), session.snapshot()) else {
                    continue;
                };
                if now.epoch != epoch {
                    break;
                }
                let merged =
                    merge_header_into_netscape(now.cookies_netscape.expose(), &sent, &header);
                if !session.offer_rotation(epoch, SecretString::new(merged)) {
                    break;
                }
            }
        });
    }
}

/// A write that could not even start: no session means sign in again; anything else (the client
/// would not build) never reached the service, so it is a definite no.
fn write_failure(failure: LoadFailure) -> WriteFailure {
    match failure {
        LoadFailure::Session => WriteFailure::Session,
        LoadFailure::Throttled => WriteFailure::Throttled,
        // The account is signed in but the service has not confirmed it yet: nothing was sent.
        LoadFailure::Network | LoadFailure::Parse | LoadFailure::Other => WriteFailure::Rejected,
    }
}

fn row(listed: ListedTrack) -> TrackRow {
    crate::explicit_marks::note(listed.track.video_id.as_str(), listed.explicit);
    TrackRow {
        track: listed.track,
        occurrence: listed.occurrence,
        thumbnail: listed.thumbnail,
        explicit: listed.explicit,
        liked: listed.liked,
    }
}

fn rows(page: Page<ListedTrack>) -> Page<TrackRow> {
    Page {
        items: page.items.into_iter().map(row).collect(),
        continuation: page.continuation,
        skipped: page.skipped,
    }
}

impl LibrarySource for InnerTubeLibrary {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            let mut all = source.library_playlists().await?;
            all.extend(source.library_albums().await?);
            Ok(all)
        })
    }

    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        use apollo_domain::library::CollectionKind;
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            let page = match collection.kind {
                CollectionKind::Playlist => source.playlist(collection.id.as_str()).await,
                CollectionKind::Album => source.album(collection.id.as_str()).await,
            }?;
            Ok(rows(page))
        })
    }

    fn page(&self, collection: &Collection) -> Result<(Page<TrackRow>, PageMeta), LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            let (page, meta) = match collection.kind {
                CollectionKind::Playlist => {
                    source.playlist_with_meta(collection.id.as_str()).await
                }
                CollectionKind::Album => source.album_with_meta(collection.id.as_str()).await,
            }?;
            Ok((rows(page), meta))
        })
    }

    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        let source = self.source()?;
        self.rt
            .block_on(async { Ok(rows(source.continuation(token).await?)) })
    }

    fn library_songs(&self) -> Result<Page<TrackRow>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            Ok(rows(source.library_songs().await?))
        })
    }

    fn library_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.library_artists().await
        })
    }

    fn subscribed_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.subscribed_artists().await
        })
    }

    fn history(&self) -> Result<Vec<HistoryDay>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.history().await
        })
    }

    fn artist(&self, id: &BrowseId) -> Result<ArtistContent, LoadFailure> {
        let source = self.source()?;
        let page = self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.artist(id.as_str()).await
        })?;
        Ok(ArtistContent {
            name: page.name,
            thumbnail: page.thumbnail,
            top_songs: page.top_songs.into_iter().map(row).collect(),
            top_songs_id: page.top_songs_id,
            sections: page.sections,
            related: page.related,
            description: page.description,
            subscribers: page.subscribers,
            monthly_listeners: page.monthly_listeners,
            channel_id: page.channel_id,
            subscribed: page.subscribed,
            radio_playlist_id: page.radio_playlist_id,
            shuffle_playlist_id: page.shuffle_playlist_id,
            skipped: page.skipped,
        })
    }

    fn home(&self) -> Result<HomePage, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.home().await
        })
    }

    fn home_chip(&self, params: &str) -> Result<HomePage, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.home_chip(params).await
        })
    }

    fn home_more(&self, token: &str) -> Result<HomePage, LoadFailure> {
        let source = self.source()?;
        self.rt
            .block_on(async { source.home_continuation(token).await })
    }

    fn related(&self, video_id: &str) -> Result<Vec<HomeShelf>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            Ok(source.related(video_id).await?.shelves)
        })
    }

    fn explore(&self) -> Result<ExplorePage, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.explore().await
        })
    }

    fn mood_playlists(&self, params: &str) -> Result<Vec<Collection>, LoadFailure> {
        let source = self.source()?;
        self.rt
            .block_on(async { source.mood_playlists(params).await })
    }

    fn radio(&self, video_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            Ok(source.radio(video_id).await?.into_iter().map(row).collect())
        })
    }

    fn mix(&self, playlist_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            Ok(source.mix(playlist_id).await?.into_iter().map(row).collect())
        })
    }

    fn set_subscribed(&self, channel_id: &str, subscribed: bool) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.set_subscribed(channel_id, subscribed).await
        })
    }

    fn set_liked(&self, video_id: &str, liked: bool) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.set_liked(video_id, liked).await
        })
    }

    fn set_disliked(&self, video_id: &str) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.set_disliked(video_id).await
        })
    }

    fn disliked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.disliked(video_id).await
        })
    }

    fn liked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.liked(video_id).await
        })
    }

    fn song_saved(&self, video_id: &str) -> Option<bool> {
        self.marks.state(video_id)
    }

    fn set_song_saved(&self, video_id: &str, saved: bool) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.set_song_saved(video_id, saved).await
        })
    }

    fn note_song_saved(&self, video_id: &str, saved: bool) {
        if saved {
            self.marks.listed(video_id);
        } else {
            self.marks.set(video_id, false);
        }
    }

    fn playlists_for(&self, video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.playlists_for(video_id).await
        })
    }

    fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.add_to_playlist(playlist_id, video_id).await.map(drop)
        })
    }

    fn add_many_to_playlist(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.add_many_to_playlist(playlist_id, video_ids).await.map(drop)
        })
    }

    fn add_to_playlist_rows(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            match video_ids {
                [video] => source.add_to_playlist(playlist_id, video).await,
                _ => source.add_many_to_playlist(playlist_id, video_ids).await,
            }
        })
    }

    fn add_again_to_playlist(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.add_again_to_playlist(playlist_id, video_ids).await
        })
    }

    fn account_token(&self) -> Option<String> {
        let snapshot = self.session.snapshot()?;
        let channel = (self.data_sync_id)().unwrap_or_default();
        Some(format!("{}:{channel}", snapshot.epoch))
    }

    fn editable_playlists(&self) -> Result<Vec<String>, LoadFailure> {
        let source = self.source()?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.editable_playlists(PROBE_VIDEO).await
        })
    }

    fn save_target(&self, collection: &Collection) -> Result<String, LoadFailure> {
        match collection.kind {
            CollectionKind::Playlist => Ok(apollo_domain::library::bare_playlist_id(
                collection.id.as_str(),
            )
            .to_owned()),
            // An album is saved on its audio playlist, which only its page names.
            CollectionKind::Album => {
                let source = self.source()?;
                let (_, meta) = self.rt.block_on(async {
                    source.ensure_visitor_data().await;
                    source.album_with_meta(collection.id.as_str()).await
                })?;
                meta.save_id.ok_or(LoadFailure::Parse)
            }
        }
    }

    fn create_playlist(&self, title: &str) -> Result<String, WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.create_playlist(title).await
        })
    }

    fn delete_playlist(&self, playlist_id: &str) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.delete_playlist(playlist_id).await
        })
    }

    fn create_playlist_with(
        &self,
        title: &str,
        description: &str,
        privacy: Privacy,
        video_ids: &[String],
    ) -> Result<String, WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source
                .create_playlist_with(title, description, privacy, video_ids)
                .await
        })
    }

    fn edit_playlist(&self, playlist_id: &str, edit: &PlaylistEdit) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.edit_playlist(playlist_id, edit).await
        })
    }

    fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        occurrence: &str,
    ) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source
                .remove_from_playlist(playlist_id, video_id, occurrence)
                .await
        })
    }

    fn move_in_playlist(
        &self,
        playlist_id: &str,
        occurrence: &str,
        before: Option<&str>,
    ) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source
                .move_in_playlist(playlist_id, occurrence, before)
                .await
        })
    }

    fn set_saved(&self, save_id: &str, saved: bool) -> Result<(), WriteFailure> {
        let source = self.source().map_err(write_failure)?;
        self.rt.block_on(async {
            source.ensure_visitor_data().await;
            source.set_saved(save_id, saved).await
        })
    }

    fn artwork(&self, url: &str) -> Option<DecodedArt> {
        // Cover art is public: an unauthenticated GET, never carrying the session.
        if !url.starts_with("https://") {
            return None;
        }
        let mut resp = self.agent.get(url).call().ok()?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(ART_MAX_BYTES)
            .read_to_vec()
            .ok()?;
        DecodedArt::from_encoded(&bytes)
    }
}

// ---- cookies ---------------------------------------------------------------------------------

/// One Netscape line: `domain, include-subdomains, path, secure, expiry, name, value`, with the
/// optional `#HttpOnly_` prefix on the domain.
struct Line<'a> {
    domain: &'a str,
    expiry: i64,
    name: &'a str,
    value: &'a str,
}

fn parse_line(raw: &str) -> Option<Line<'_>> {
    let line = raw.strip_prefix("#HttpOnly_").unwrap_or(raw);
    if line.trim().is_empty() || line.starts_with('#') {
        return None;
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    Some(Line {
        domain: f[0],
        expiry: f[4].trim().parse().unwrap_or(0),
        name: f[5],
        value: f[6].trim_end_matches('\r'),
    })
}

fn is_youtube(domain: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d == "youtube.com" || d.ends_with(".youtube.com")
}

/// A `Cookie:` header for requests to `music.youtube.com`: the unexpired YouTube-domain cookies
/// of the Netscape text, the more specific domain winning when a name appears twice. `None`
/// when there are none.
pub fn cookie_header(netscape: &str, now: u64) -> Option<String> {
    let mut pairs: Vec<(&str, &str, usize)> = Vec::new();
    for line in netscape.lines().filter_map(parse_line) {
        let alive = line.expiry <= 0 || line.expiry as u64 > now;
        if !alive || !is_youtube(line.domain) {
            continue;
        }
        let specificity = line.domain.len();
        match pairs.iter_mut().find(|(n, _, _)| *n == line.name) {
            Some(existing) if existing.2 >= specificity => {}
            Some(existing) => *existing = (line.name, line.value, specificity),
            None => pairs.push((line.name, line.value, specificity)),
        }
    }
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(n, v, _)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Netscape text with the cookies that rotated written back. `sent` is the header the session
/// started with and `now` the one it holds after a response rotated cookies: only names whose
/// value differs (or that are new) are touched, so a same-named cookie on a different YouTube
/// subdomain keeps its own value. A rotated name replaces the value on each YouTube-domain line
/// of that name; a new name is appended as a session cookie. Everything else is kept as it was.
pub fn merge_header_into_netscape(netscape: &str, sent: &str, now: &str) -> String {
    fn split(h: &str) -> Vec<(&str, &str)> {
        h.split(';')
            .filter_map(|p| p.trim().split_once('='))
            .collect()
    }
    let sent = split(sent);
    let rotated: Vec<(&str, &str)> = split(now)
        .into_iter()
        .filter(|(n, v)| {
            sent.iter()
                .find(|(sn, _)| sn == n)
                .is_none_or(|(_, sv)| sv != v)
        })
        .collect();
    let mut seen = vec![false; rotated.len()];
    let mut out = String::with_capacity(netscape.len() + 64);
    for raw in netscape.lines() {
        let hit = parse_line(raw)
            .filter(|l| is_youtube(l.domain))
            .and_then(|l| rotated.iter().position(|(n, _)| *n == l.name));
        match hit {
            Some(i) => {
                seen[i] = true;
                let (head, _) = raw.rsplit_once('\t').expect("a parsed line has tabs");
                out.push_str(head);
                out.push('\t');
                out.push_str(rotated[i].1);
            }
            None => out.push_str(raw),
        }
        out.push('\n');
    }
    for ((name, value), seen) in rotated.iter().zip(seen) {
        if !seen {
            out.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETSCAPE: &str = "# Netscape HTTP Cookie File\n\
        #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSID\tsid-value\n\
        .youtube.com\tTRUE\t/\tTRUE\t4102444800\tSAPISID\tsapisid-value\n\
        .google.com\tTRUE\t/\tTRUE\t4102444800\tNID\tgoogle-only\n\
        .youtube.com\tTRUE\t/\tTRUE\t1\tOLD\texpired\n\
        music.youtube.com\tFALSE\t/\tTRUE\t0\tSID\tmusic-sid\n";

    #[test]
    fn the_header_keeps_live_youtube_cookies_and_prefers_the_specific_domain() {
        let header = cookie_header(NETSCAPE, 1_000_000).unwrap();
        let mut parts: Vec<&str> = header.split("; ").collect();
        parts.sort();
        assert_eq!(parts, ["SAPISID=sapisid-value", "SID=music-sid"]);
    }

    #[test]
    fn no_youtube_cookies_means_no_header() {
        assert_eq!(cookie_header("# nothing\n", 0), None);
        assert_eq!(
            cookie_header(".google.com\tTRUE\t/\tTRUE\t0\tNID\tx\n", 0),
            None
        );
    }

    #[test]
    fn rotated_values_replace_in_place_and_new_names_are_appended() {
        let sent = cookie_header(NETSCAPE, 1_000_000).unwrap();
        let now = sent.replace("sapisid-value", "sapisid-rotated") + "; __Secure-1PSIDTS=fresh";
        let merged = merge_header_into_netscape(NETSCAPE, &sent, &now);
        assert!(merged.contains("\tSAPISID\tsapisid-rotated\n"));
        assert!(!merged.contains("sapisid-value"));
        assert!(
            merged.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSID\tsid-value\n"),
            "an unrotated cookie keeps its line, flags and expiry (even with a same-named twin)"
        );
        assert!(merged.contains(".google.com\tTRUE\t/\tTRUE\t4102444800\tNID\tgoogle-only\n"));
        assert!(merged.ends_with(".youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSIDTS\tfresh\n"));
    }

    #[test]
    fn a_google_domain_cookie_is_never_overwritten_by_a_youtube_header() {
        let merged = merge_header_into_netscape(NETSCAPE, "SID=x", "NID=from-header");
        assert!(merged.contains("\tNID\tgoogle-only\n"));
        // Appended as a new YouTube-domain cookie instead.
        assert!(merged.ends_with(".youtube.com\tTRUE\t/\tTRUE\t0\tNID\tfrom-header\n"));
    }

    #[test]
    fn an_unchanged_header_changes_nothing() {
        let sent = cookie_header(NETSCAPE, 1_000_000).unwrap();
        assert_eq!(
            merge_header_into_netscape(NETSCAPE, &sent, &sent),
            NETSCAPE.to_string()
        );
    }
}
