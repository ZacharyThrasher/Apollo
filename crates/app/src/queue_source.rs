//! Where a queue came from, as the pages that can start one name it, and the way back: the
//! page a "Playing from" line opens.

use apollo_domain::library::{Collection, CollectionKind, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::model::BrowseId;
use apollo_domain::queue::{QueueSource, SourceKind};

use crate::library::{ArtistTarget, Target};

/// A playlist, an album or Liked Music, played from its page.
pub fn of_collection(c: &Collection) -> QueueSource {
    let (kind, name) = if c.is_liked_music() {
        (SourceKind::LikedSongs, "Liked songs".to_owned())
    } else {
        let kind = match c.kind {
            CollectionKind::Playlist => SourceKind::Playlist,
            CollectionKind::Album => SourceKind::Album,
        };
        (kind, c.title.clone())
    };
    QueueSource::new(kind, name)
        .with_target(c.id.as_str())
        .with_thumbnail(c.thumbnail.clone())
}

/// An artist's songs or mix, played from their page.
pub fn of_artist(a: &ArtistTarget) -> QueueSource {
    QueueSource::new(SourceKind::Artist, a.name.clone())
        .with_target(a.id.as_str())
        .with_thumbnail(a.thumbnail.clone())
}

/// A radio seeded by `seed` (a song, an artist or a collection's name). It has no page.
pub fn radio(seed: impl Into<String>) -> QueueSource {
    QueueSource::new(SourceKind::Radio, seed)
}

/// Search results for `query`.
pub fn search(query: impl Into<String>) -> QueueSource {
    QueueSource::new(SourceKind::Search, query)
}

/// A named place with no page of its own (a Home shelf, a chart).
pub fn named(name: impl Into<String>) -> QueueSource {
    QueueSource::new(SourceKind::Other, name)
}

/// The page the source's name opens, when it has one.
pub fn page_of(source: &QueueSource) -> Option<Target> {
    let target = source.target.as_deref()?;
    match source.kind {
        SourceKind::Artist => Some(Target::Artist(ArtistTarget {
            id: BrowseId::new(target),
            name: source.name.clone(),
            thumbnail: source.thumbnail.clone(),
        })),
        SourceKind::Playlist | SourceKind::Album | SourceKind::LikedSongs => {
            let kind = if source.kind == SourceKind::Album {
                CollectionKind::Album
            } else {
                CollectionKind::Playlist
            };
            let id = if source.kind == SourceKind::LikedSongs {
                LIKED_MUSIC_BROWSE_ID
            } else {
                target
            };
            Some(Target::Collection(Collection {
                kind,
                id: BrowseId::new(id),
                title: source.label(),
                subtitle: None,
                thumbnail: source.thumbnail.clone(),
            }))
        }
        SourceKind::Radio | SourceKind::Search | SourceKind::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album() -> Collection {
        Collection {
            kind: CollectionKind::Album,
            id: BrowseId::new("MPRE1"),
            title: "Blue".into(),
            subtitle: Some("Joni Mitchell".into()),
            thumbnail: Some("https://img/1".into()),
        }
    }

    #[test]
    fn a_collection_opens_again_from_its_source() {
        let source = of_collection(&album());
        assert_eq!(source.label(), "Blue");
        let Some(Target::Collection(back)) = page_of(&source) else {
            panic!("an album has a page");
        };
        assert_eq!(back.kind, CollectionKind::Album);
        assert_eq!(back.id.as_str(), "MPRE1");
        assert_eq!(back.title, "Blue");
    }

    #[test]
    fn liked_music_reads_liked_songs_and_opens_liked_music() {
        let liked = Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new(LIKED_MUSIC_BROWSE_ID),
            title: "Liked Music".into(),
            subtitle: None,
            thumbnail: None,
        };
        let source = of_collection(&liked);
        assert_eq!(source.label(), "Liked songs");
        let Some(Target::Collection(back)) = page_of(&source) else {
            panic!("liked music has a page");
        };
        assert!(back.is_liked_music());
    }

    #[test]
    fn an_artist_opens_and_radios_searches_and_shelves_do_not() {
        let artist = ArtistTarget {
            id: BrowseId::new("UC1"),
            name: "Daft Punk".into(),
            thumbnail: None,
        };
        assert!(matches!(page_of(&of_artist(&artist)), Some(Target::Artist(_))));
        assert!(page_of(&radio("Daft Punk")).is_none());
        assert!(page_of(&search("daft")).is_none());
        assert!(page_of(&named("Quick picks")).is_none());
        assert_eq!(radio("Daft Punk").label(), "Daft Punk radio");
        assert_eq!(search("daft").label(), "Search: daft");
    }
}
