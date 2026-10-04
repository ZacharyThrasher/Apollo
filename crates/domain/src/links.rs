//! External link parsing (BUILD_SPEC DATA-04).
//!
//! Exact host rules on a real URL parse. The result carries only validated IDs;
//! pasted text never reaches a shell or the filesystem through this entry point.

use url::Url;

use crate::library::{Collection, CollectionKind};
use crate::model::{BrowseId, PlaylistId, VideoId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalLink {
    /// A specific track, optionally within a playlist context.
    Track {
        video: VideoId,
        playlist: Option<PlaylistId>,
    },
    Playlist(PlaylistId),
    /// An album page (`/browse/MPREb_…`).
    Album(BrowseId),
    /// An artist page (`/channel/UC…` or `/browse/UC…`).
    Artist(BrowseId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    NotAUrl,
    Scheme,
    Host,
    UnsupportedPath,
    BadId,
}

const HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
];

pub fn parse_external_link(input: &str) -> Result<ExternalLink, LinkError> {
    let url = Url::parse(input.trim()).map_err(|_| LinkError::NotAUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(LinkError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(LinkError::Host);
    }
    let host = url.host_str().ok_or(LinkError::Host)?;
    if !HOSTS.contains(&host) {
        return Err(LinkError::Host);
    }
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let playlist = match query("list") {
        Some(l) => Some(playlist_id(&l)?),
        None => None,
    };

    if host == "youtu.be" {
        let id = url.path_segments().and_then(|mut s| s.next()).unwrap_or("");
        return Ok(ExternalLink::Track {
            video: video_id(id)?,
            playlist,
        });
    }
    match url.path() {
        "/watch" => match query("v") {
            Some(v) => Ok(ExternalLink::Track {
                video: video_id(&v)?,
                playlist,
            }),
            None => playlist
                .map(ExternalLink::Playlist)
                .ok_or(LinkError::UnsupportedPath),
        },
        "/playlist" => playlist
            .map(ExternalLink::Playlist)
            .ok_or(LinkError::UnsupportedPath),
        path => {
            let mut segments = path.trim_matches('/').split('/');
            let (kind, id) = (segments.next(), segments.next());
            if segments.next().is_some() {
                return Err(LinkError::UnsupportedPath);
            }
            match (kind, id) {
                (Some("channel"), Some(id)) if id.starts_with("UC") => {
                    browse_id(id).map(ExternalLink::Artist)
                }
                (Some("browse"), Some(id)) if id.starts_with("MPRE") => {
                    browse_id(id).map(ExternalLink::Album)
                }
                (Some("browse"), Some(id)) if id.starts_with("UC") => {
                    browse_id(id).map(ExternalLink::Artist)
                }
                // A playlist's browse id is its list id behind a `VL`.
                (Some("browse"), Some(id)) if id.starts_with("VL") => {
                    playlist_id(&id[2..]).map(ExternalLink::Playlist)
                }
                _ => Err(LinkError::UnsupportedPath),
            }
        }
    }
}

/// The shareable link of a track on YouTube Music.
pub fn track_url(video: &VideoId) -> String {
    format!("https://music.youtube.com/watch?v={}", video.as_str())
}

/// The shareable link of an album or playlist: a playlist by its list id (the `VL` browse prefix
/// dropped), an album by its browse id.
pub fn collection_url(collection: &Collection) -> String {
    let id = collection.id.as_str();
    match collection.kind {
        CollectionKind::Playlist => format!(
            "https://music.youtube.com/playlist?list={}",
            id.strip_prefix("VL").unwrap_or(id)
        ),
        CollectionKind::Album => format!("https://music.youtube.com/browse/{id}"),
    }
}

fn id_chars_ok(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn video_id(s: &str) -> Result<VideoId, LinkError> {
    VideoId::parse(s).ok_or(LinkError::BadId)
}

fn browse_id(s: &str) -> Result<BrowseId, LinkError> {
    if (2..=64).contains(&s.len()) && id_chars_ok(s) {
        Ok(BrowseId::new(s))
    } else {
        Err(LinkError::BadId)
    }
}

fn playlist_id(s: &str) -> Result<PlaylistId, LinkError> {
    if (2..=64).contains(&s.len()) && id_chars_ok(s) {
        Ok(PlaylistId::new(s))
    } else {
        Err(LinkError::BadId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "dQw4w9WgXcQ";

    fn track(v: &str, list: Option<&str>) -> ExternalLink {
        ExternalLink::Track {
            video: VideoId::new(v),
            playlist: list.map(PlaylistId::new),
        }
    }

    #[test]
    fn parses_the_ordinary_forms() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc",
            "https://m.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "  https://youtube.com/watch?v=dQw4w9WgXcQ  ",
            "HTTPS://MUSIC.YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(parse_external_link(u), Ok(track(V, None)), "{u}");
        }
    }

    #[test]
    fn a_track_link_is_the_music_watch_url_and_parses_back() {
        let video = VideoId::new(V);
        let url = track_url(&video);
        assert_eq!(url, "https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(parse_external_link(&url), Ok(track(V, None)));
    }

    #[test]
    fn collection_links_name_the_playlist_or_the_album() {
        let collection = |kind, id: &str| Collection {
            kind,
            id: crate::model::BrowseId::new(id),
            title: String::new(),
            subtitle: None,
            thumbnail: None,
        };
        let playlist = collection_url(&collection(CollectionKind::Playlist, "VLPL123"));
        assert_eq!(playlist, "https://music.youtube.com/playlist?list=PL123");
        assert_eq!(
            parse_external_link(&playlist),
            Ok(ExternalLink::Playlist(PlaylistId::new("PL123")))
        );
        assert_eq!(
            collection_url(&collection(CollectionKind::Album, "MPREb_abc")),
            "https://music.youtube.com/browse/MPREb_abc"
        );
    }

    #[test]
    fn keeps_playlist_context_and_plain_playlists() {
        assert_eq!(
            parse_external_link(
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=OLAK5uy_abc-DEF"
            ),
            Ok(track(V, Some("OLAK5uy_abc-DEF")))
        );
        assert_eq!(
            parse_external_link("https://music.youtube.com/playlist?list=PL1234567890"),
            Ok(ExternalLink::Playlist(PlaylistId::new("PL1234567890")))
        );
        assert_eq!(
            parse_external_link("https://www.youtube.com/watch?list=PL1234567890"),
            Ok(ExternalLink::Playlist(PlaylistId::new("PL1234567890")))
        );
    }

    #[test]
    fn rejects_lookalike_and_userinfo_hosts() {
        for u in [
            "https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ",
            "https://evilyoutube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com@evil.example/watch?v=dQw4w9WgXcQ",
            "https://evil.example/watch?v=dQw4w9WgXcQ&u=youtube.com",
            "https://youtu.be.evil.example/dQw4w9WgXcQ",
            "https://studio.youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(parse_external_link(u), Err(LinkError::Host), "{u}");
        }
    }

    #[test]
    fn rejects_non_web_schemes_and_non_urls() {
        for u in [
            "file:///C:/a.m4a",
            "javascript:alert(1)",
            "ftp://youtube.com/watch?v=dQw4w9WgXcQ",
            "C:\\media\\a.m4a",
        ] {
            assert_eq!(parse_external_link(u), Err(LinkError::Scheme), "{u}");
        }
        for u in [
            "",
            "dQw4w9WgXcQ",
            "not a url",
            "youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(parse_external_link(u), Err(LinkError::NotAUrl), "{u}");
        }
    }

    #[test]
    fn rejects_malformed_ids_and_shell_metacharacters() {
        for u in [
            "https://youtu.be/short",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQtoolong",
            "https://www.youtube.com/watch?v=dQw4w9WgX%3Bq",
            "https://www.youtube.com/watch?v=$(reboot)",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=a%20b",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
        ] {
            assert_eq!(parse_external_link(u), Err(LinkError::BadId), "{u}");
        }
    }

    #[test]
    fn rejects_unsupported_paths() {
        for u in [
            "https://www.youtube.com/",
            "https://www.youtube.com/channel/PL1234567890",
            "https://www.youtube.com/channel/UC1234567890/videos",
            "https://www.youtube.com/watch",
            "https://music.youtube.com/browse/FEmusic_home",
            "https://www.youtube.com/@someone",
        ] {
            assert_eq!(
                parse_external_link(u),
                Err(LinkError::UnsupportedPath),
                "{u}"
            );
        }
    }

    #[test]
    fn album_artist_and_browse_playlist_pages_parse() {
        assert_eq!(
            parse_external_link("https://music.youtube.com/browse/MPREb_abc123"),
            Ok(ExternalLink::Album(BrowseId::new("MPREb_abc123")))
        );
        for u in [
            "https://music.youtube.com/channel/UCabc123?si=x",
            "https://www.youtube.com/channel/UCabc123",
            "https://music.youtube.com/browse/UCabc123",
        ] {
            assert_eq!(
                parse_external_link(u),
                Ok(ExternalLink::Artist(BrowseId::new("UCabc123"))),
                "{u}"
            );
        }
        assert_eq!(
            parse_external_link("https://music.youtube.com/browse/VLPL123"),
            Ok(ExternalLink::Playlist(PlaylistId::new("PL123")))
        );
        assert_eq!(
            parse_external_link("https://music.youtube.com/channel/UCabc%3B123"),
            Err(LinkError::BadId)
        );
    }
}
