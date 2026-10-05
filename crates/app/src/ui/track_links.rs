//! The "Artist, Artist · Album" line of the player bar and Now Playing, with each name a link to
//! its page. A name without a browse id stays plain text.

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::model::{AlbumRef, ArtistRef};
use apollo_theme::tokens::typography;
use gpui::{
    div, prelude::*, px, App, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};

use crate::library::model::{ArtistTarget, Target};
use crate::style::{self, color};
use crate::ui::search_page::OpenTarget;
use crate::ui::tooltip::Tooltip;

/// One piece of the line: a separator, or a name that may open a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Sep(&'static str),
    Name {
        text: String,
        target: Option<Target>,
    },
}

/// The line as parts: artists joined by ", ", then " · " and the album when it is shown.
pub fn parts(artists: &[ArtistRef], album: Option<&AlbumRef>) -> Vec<Part> {
    let mut out = Vec::new();
    for (i, artist) in artists.iter().enumerate() {
        if i > 0 {
            out.push(Part::Sep(", "));
        }
        out.push(Part::Name {
            text: artist.name.clone(),
            target: artist.id.clone().map(|id| {
                Target::Artist(ArtistTarget {
                    id,
                    name: artist.name.clone(),
                    thumbnail: None,
                })
            }),
        });
    }
    if let Some(album) = album.filter(|a| !a.title.is_empty()) {
        if !out.is_empty() {
            out.push(Part::Sep(" · "));
        }
        out.push(Part::Name {
            text: album.title.clone(),
            target: album.id.clone().map(|id| {
                Target::Collection(Collection {
                    kind: CollectionKind::Album,
                    id,
                    title: album.title.clone(),
                    subtitle: None,
                    thumbnail: None,
                })
            }),
        });
    }
    out
}

/// One truncated line. Names that have a page are links: text.secondary, text.primary and
/// underlined on hover, with a pointer cursor. Ids are `<id>-<n>` for the headless tests.
pub fn track_links(
    id: &'static str,
    artists: &[ArtistRef],
    album: Option<&AlbumRef>,
    style_: typography::TextStyle,
    on_open: &OpenTarget,
    cx: &App,
) -> impl IntoElement {
    let parts = parts(artists, album);
    let full: SharedString = parts
        .iter()
        .map(|p| match p {
            Part::Sep(s) => s,
            Part::Name { text, .. } => text.as_str(),
        })
        .collect::<String>()
        .into();
    let children = parts.into_iter().enumerate().map(|(ix, part)| match part {
        Part::Sep(text) => div().flex_none().child(text).into_any_element(),
        Part::Name { text, target: None } => div().min_w_0().truncate().child(text).into_any_element(),
        Part::Name {
            text,
            target: Some(target),
        } => {
            let open = on_open.clone();
            div()
                .id((id, ix))
                .debug_selector(move || format!("{id}-{ix}"))
                .min_w_0()
                .truncate()
                .cursor_pointer()
                .hover(|s| s.text_color(color::text_primary()).underline())
                // The click is the link's own, not the block's behind it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, window, cx| {
                    // Space is the global play/pause key.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == gpui::KeyboardButton::Space {
                            return;
                        }
                    }
                    cx.stop_propagation();
                    open(target.clone(), window, cx);
                })
                .child(text)
                .into_any_element()
        }
    });
    style::text_style(div(), style_, cx)
        .id(id)
        .flex()
        .flex_row()
        .min_w_0()
        .overflow_hidden()
        .text_color(color::text_secondary())
        .when(!full.is_empty(), move |d| {
            d.tooltip(move |_, cx| Tooltip::view(full.clone(), cx))
        })
        .children(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::model::BrowseId;

    fn artist(name: &str, id: Option<&str>) -> ArtistRef {
        ArtistRef {
            name: name.into(),
            id: id.map(|i| BrowseId::new(i.to_owned())),
        }
    }

    #[test]
    fn each_artist_is_its_own_link_and_the_album_follows() {
        let album = AlbumRef {
            title: "CHROMAKOPIA".into(),
            id: Some(BrowseId::new("MPREb_1".to_owned())),
        };
        let line = parts(
            &[artist("A", Some("UC1")), artist("B", Some("UC2"))],
            Some(&album),
        );
        assert_eq!(line.len(), 5);
        assert!(matches!(&line[0], Part::Name { text, target: Some(Target::Artist(_)) } if text == "A"));
        assert_eq!(line[1], Part::Sep(", "));
        assert!(matches!(&line[2], Part::Name { text, target: Some(Target::Artist(_)) } if text == "B"));
        assert_eq!(line[3], Part::Sep(" · "));
        assert!(
            matches!(&line[4], Part::Name { text, target: Some(Target::Collection(c)) } if text == "CHROMAKOPIA" && c.kind == CollectionKind::Album)
        );
    }

    #[test]
    fn a_name_without_an_id_is_plain_text() {
        let album = AlbumRef {
            title: "Album".into(),
            id: None,
        };
        let line = parts(&[artist("A", None)], Some(&album));
        assert!(matches!(&line[0], Part::Name { target: None, .. }));
        assert!(matches!(&line[2], Part::Name { target: None, .. }));
    }

    #[test]
    fn a_missing_artist_or_album_leaves_out_its_separator() {
        let album = AlbumRef {
            title: "Album".into(),
            id: None,
        };
        assert_eq!(parts(&[], Some(&album)).len(), 1);
        assert_eq!(parts(&[artist("A", None)], None).len(), 1);
        assert!(parts(&[], None).is_empty());
    }
}
