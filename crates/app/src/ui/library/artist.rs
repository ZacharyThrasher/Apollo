//! The artist page: a wide banner with the name over it, a button row (Play, Shuffle, Radio,
//! Subscribe), the top songs, the albums and singles as sideways card shelves, similar artists
//! as round cards, and the bio with the subscriber count. See DIRECTION.md, "Artist page".
//!
//! A child of the library view: it draws from the model's open artist page and shares the
//! view's keyboard selection (the top songs are the list rows) and tab stops.

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{img, linear_color_stop, linear_gradient, ImageSource, ObjectFit, RenderImage};
use std::sync::Arc;

use super::*;
use crate::library::model::MixKind;

/// The banner's height; its width is the page's.
const BANNER_HEIGHT: f32 = 280.0;
/// Children of the scrolling column before the first top song: the banner, the button row and
/// the "Top songs" heading. Keyboard selection scrolls to `selected + SONGS_OFFSET`.
pub(super) const SONGS_OFFSET: usize = 3;
/// Lines of the bio shown until it is opened.
const BIO_LINES: usize = 3;
/// The Subscribe button keeps this width whether it reads "Subscribe" or "Subscribed".
const SUBSCRIBE_WIDTH: f32 = 116.0;
/// The bio reads best at a measure, not across a wide window.
const BIO_MAX_WIDTH: f32 = 720.0;
/// Room inside a scrolling shelf for the focus ring, which sits outside what it rings.
const RING_ROOM: f32 = layout::FOCUS_RING_OFFSET + layout::FOCUS_RING_WIDTH;

/// What the page needs from the model, copied out so nothing stays borrowed while drawing.
struct Snapshot {
    name: SharedString,
    /// The card the page was opened from (usually square): the banner's stand-in.
    target_thumbnail: Option<String>,
    state: Load<Arc<ArtistContent>>,
    pending: Option<PlayRequest>,
    subscribed: bool,
    subscribing: bool,
    mix: Option<MixKind>,
    bio_open: bool,
}

impl LibraryView {
    pub(super) fn render_artist(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let animations = !cx.reduce_motion();
        let snap = {
            let a: &ArtistDetail = self.model.read(cx).artist().expect("artist page is open");
            Snapshot {
                name: SharedString::from(a.name().to_owned()),
                target_thumbnail: a.target.thumbnail.clone(),
                state: a.state.clone(),
                pending: a.pending,
                subscribed: a.subscribed(),
                subscribing: a.subscribing.is_some(),
                mix: a.mix,
                bio_open: a.bio_open,
            }
        };
        let this = cx.entity();

        let back = div().flex().flex_row().flex_none().mb(px(spacing::S2)).child(
            IconButton::new(
                "library-back",
                &self.focus.back,
                Icon::ArrowLeft,
                "Back",
                accent,
            )
            .on_click({
                let this = this.clone();
                move |window, cx| this.update(cx, |t, cx| t.back(window, cx))
            }),
        );

        let (banner_url, loading) = match &snap.state {
            Load::Ready(c) => (
                c.thumbnail.clone().or_else(|| snap.target_thumbnail.clone()),
                false,
            ),
            Load::Idle | Load::Loading => (None, true),
            Load::Failed(_) => (snap.target_thumbnail.clone(), false),
        };
        let banner = self.artist_banner(snap.name.clone(), banner_url.as_deref(), loading, cx);
        let buttons = self.artist_buttons(&snap, accent);

        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(back);
        match snap.state.clone() {
            Load::Idle | Load::Loading => root
                .child(banner)
                .child(buttons)
                .child(
                    div().flex_1().min_h_0().mt(px(spacing::S4)).child(
                        Self::skeleton_rows(
                            ARTIST_SKELETON_ROWS,
                            TRACK_ROW,
                            TRACK_ART,
                            animations,
                        ),
                    ),
                )
                .into_any_element(),
            Load::Failed(failure) => {
                let model = self.model.clone();
                let notice = self.notice(
                    Self::failure_line(failure, "this artist"),
                    true,
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.reload_artist(cx)),
                    cx,
                );
                root.child(banner)
                    .child(buttons)
                    .child(div().mt(px(spacing::S4)).child(notice))
                    .into_any_element()
            }
            Load::Ready(content) if content.is_empty() => {
                let notice = self.notice(
                    "Nothing to show",
                    false,
                    "Back",
                    accent,
                    move |window, cx| this.update(cx, |t, cx| t.back(window, cx)),
                    cx,
                );
                root.child(banner)
                    .child(buttons)
                    .child(div().mt(px(spacing::S4)).child(notice))
                    .into_any_element()
            }
            Load::Ready(content) => {
                let body = self.artist_columns(
                    vec![banner, buttons],
                    &content,
                    snap.bio_open,
                    accent,
                    window,
                    cx,
                );
                root.child(div().flex_1().min_h_0().child(body))
                    .into_any_element()
            }
        }
    }

    /// The wide picture, cropped to its frame, fading into the page at the bottom with the name
    /// over it. No blur: the ambient blur belongs to Now Playing alone.
    fn artist_banner(
        &mut self,
        name: SharedString,
        url: Option<&str>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let image: Option<Arc<RenderImage>> = url.and_then(|url| {
            if !self.model.read(cx).art_requested(url, ArtSize::Banner) {
                let url = url.to_owned();
                self.model
                    .update(cx, |m, cx| m.want_art(&url, ArtSize::Banner, cx));
                return None;
            }
            self.model.read(cx).art(url, ArtSize::Banner)
        });
        // The opaque page color: the Mica alpha would leave the picture showing through.
        let page = style::hsla(apollo_theme::tokens::color::BG_BASE);
        let corners = px(radius::MEDIUM);
        div()
            .id("artist-banner")
            .debug_selector(|| "artist-banner".into())
            .relative()
            .flex_none()
            .w_full()
            .h(px(BANNER_HEIGHT))
            .rounded(corners)
            .overflow_hidden()
            .bg(if loading {
                color::skeleton()
            } else {
                color::bg_raised()
            })
            .when_some(image, |d, image| {
                d.child(
                    img(ImageSource::Render(image))
                        .absolute()
                        .size_full()
                        .rounded(corners)
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded(corners)
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(page.opacity(0.0), 0.35),
                        linear_color_stop(page, 1.0),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(px(spacing::S6))
                    .right(px(spacing::S6))
                    .bottom(px(spacing::S6))
                    .flex()
                    .flex_row()
                    .child(Self::line(
                        "artist-name",
                        name,
                        typography::ARTIST_NAME,
                        color::text_primary(),
                        false,
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// Play (accent filled), Shuffle, Radio and the Subscribe toggle.
    fn artist_buttons(&self, snap: &Snapshot, accent: Accent) -> AnyElement {
        let ready = matches!(&snap.state, Load::Ready(c) if !c.is_empty());
        let loading = matches!(snap.state, Load::Idle | Load::Loading);
        let can_play = matches!(&snap.state, Load::Ready(c) if !c.top_songs.is_empty());
        let can_mix = matches!(&snap.state, Load::Ready(c) if
            c.radio_playlist_id.is_some() || !c.top_songs.is_empty());
        let can_subscribe =
            matches!(&snap.state, Load::Ready(c) if !c.channel_id.is_empty());
        let play_busy = snap.pending.is_some_and(|p| p.shuffle != Some(true));
        let shuffle_busy =
            snap.pending.is_some_and(|p| p.shuffle == Some(true)) || snap.mix == Some(MixKind::Shuffle);
        let radio_busy = snap.mix == Some(MixKind::Radio);
        let (model_play, model_shuffle, model_radio, model_subscribe) = (
            self.model.clone(),
            self.model.clone(),
            self.model.clone(),
            self.model.clone(),
        );
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S2))
            .mt(px(spacing::S4))
            .child(selector(
                "artist-play",
                TextButton::new("library-play", &self.focus.play, "Play", accent)
                    .accent_fill()
                    .enabled(can_play || loading)
                    .busy(play_busy)
                    .on_click(move |_, cx| {
                        model_play.update(cx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx))
                    }),
            ))
            .child(selector(
                "artist-shuffle",
                TextButton::new("library-shuffle", &self.focus.shuffle, "Shuffle", accent)
                    .outlined(false)
                    .enabled(can_play || loading || can_mix)
                    .busy(shuffle_busy)
                    .on_click(move |_, cx| {
                        model_shuffle.update(cx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx))
                    }),
            ))
            .child(selector(
                "artist-radio",
                TextButton::new("library-radio", &self.focus.radio, "Radio", accent)
                    .outlined(false)
                    .enabled(ready && can_mix)
                    .busy(radio_busy)
                    .on_click(move |_, cx| {
                        model_radio.update(cx, |m, cx| m.start_artist_radio(cx))
                    }),
            ))
            .child(selector(
                "artist-subscribe",
                TextButton::new(
                    "library-subscribe",
                    &self.focus.subscribe,
                    if snap.subscribed {
                        "Subscribed"
                    } else {
                        "Subscribe"
                    },
                    accent,
                )
                .outlined(snap.subscribed)
                .min_width(SUBSCRIBE_WIDTH)
                .enabled(ready && can_subscribe && !snap.subscribing)
                .on_click(move |_, cx| model_subscribe.update(cx, |m, cx| m.toggle_subscribe(cx))),
            ))
            .into_any_element()
    }

    /// The artist's scrolling body: the banner and buttons, the top songs as rows, a shelf per
    /// album/single/playlist section, similar artists, then the bio.
    fn artist_columns(
        &mut self,
        top: Vec<AnyElement>,
        content: &ArtistContent,
        bio_open: bool,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let animations = !cx.reduce_motion();
        let cards: usize =
            content.sections.iter().map(|s| s.items.len()).sum::<usize>() + content.related.len();
        while self.card_focus.len() < cards {
            self.card_focus
                .push(cx.focus_handle().tab_index(10).tab_stop(true));
        }
        let shelves = content.sections.len() + usize::from(!content.related.is_empty());
        let (column, shelf_scroll) = {
            let Some(page) = self.pages.last_mut() else {
                return div().into_any_element();
            };
            page.shelves.resize_with(shelves, ScrollHandle::new);
            (page.column.clone(), page.shelves.clone())
        };

        // Direct children of the scroll container are what keyboard selection scrolls to: the
        // banner and the buttons are children 0 and 1, the "Top songs" heading is child 2, then
        // one child per song (`SONGS_OFFSET`).
        let mut children: Vec<AnyElement> = top;
        let songs = content.top_songs.len();
        if songs > 0 {
            children.push(self.top_songs_heading(content, accent, cx));
            let list_focused = keyboard_focused(&self.focus.list, window);
            self.hover.prune();
            let mut moving = false;
            for ix in 0..songs {
                let frame = RowFrame {
                    ix,
                    real: ix,
                    level: self.hover.level(ix, &mut moving),
                    selected: list_focused && ix == self.selected,
                    accent,
                    animations,
                };
                children.push(self.track_row(frame, window, cx));
            }
            if moving {
                window.request_animation_frame();
            }
            self.rendered = 0..songs;
        }

        let this = cx.entity();
        let mut stop = 0;
        // The card that holds keyboard focus: its stop, its shelf, its place and its child.
        let mut focused: Option<(usize, usize, usize, usize)> = None;
        for (section_ix, section) in content.sections.iter().enumerate() {
            children.push(heading(section.title.clone(), !children.is_empty(), cx));
            let mut cards = Vec::with_capacity(section.items.len());
            for (ix, item) in section.items.iter().enumerate() {
                let thumb = self.card_thumb(item.thumbnail.as_deref(), cx);
                if keyboard_focused(&self.card_focus[stop], window) {
                    focused = Some((stop, section_ix, ix, children.len()));
                }
                let (this, item_open) = (this.clone(), item.clone());
                cards.push(
                    Card::new(
                        format!("artist-card-{section_ix}-{ix}"),
                        item.title.clone(),
                        item.subtitle.clone().map(Into::into),
                        thumb,
                        &self.card_focus[stop],
                        accent,
                        move |window, cx| {
                            this.update(cx, |t, cx| t.open(item_open.clone(), window, cx))
                        },
                    )
                    .menu(Subject::Collection(item.clone())),
                );
                stop += 1;
            }
            children.push(shelf(
                ("artist-shelf", section_ix),
                &shelf_scroll[section_ix],
                cards,
            ));
        }
        if !content.related.is_empty() {
            let section_ix = content.sections.len();
            children.push(heading("Fans also like", !children.is_empty(), cx));
            let mut cards = Vec::with_capacity(content.related.len());
            for (ix, artist) in content.related.iter().enumerate() {
                let thumb = self.card_thumb(artist.thumbnail.as_deref(), cx);
                if keyboard_focused(&self.card_focus[stop], window) {
                    focused = Some((stop, section_ix, ix, children.len()));
                }
                let (this, target) = (
                    this.clone(),
                    ArtistTarget {
                        id: artist.id.clone(),
                        name: artist.name.clone(),
                        thumbnail: artist.thumbnail.clone(),
                    },
                );
                cards.push(
                    Card::new(
                        format!("artist-fan-{ix}"),
                        artist.name.clone(),
                        None,
                        thumb,
                        &self.card_focus[stop],
                        accent,
                        move |window, cx| {
                            this.update(cx, |t, cx| t.open_artist(target.clone(), window, cx))
                        },
                    )
                    .circular(true),
                );
                stop += 1;
            }
            children.push(shelf(
                ("artist-shelf", section_ix),
                &shelf_scroll[section_ix],
                cards,
            ));
        }

        // Keyboard focus brings its card into view, sideways in its shelf and down the page.
        if let Some(page) = self.pages.last_mut() {
            match focused {
                Some((stop, shelf_ix, ix, child)) if page.scrolled_to != Some(stop) => {
                    page.scrolled_to = Some(stop);
                    shelf_scroll[shelf_ix].scroll_to_item(ix);
                    page.column.scroll_to_item(child);
                }
                Some(_) => {}
                None => page.scrolled_to = None,
            }
        }

        if let Some(about) = self.artist_about(content, bio_open, accent, window, cx) {
            children.push(heading("About", !children.is_empty(), cx));
            children.push(about);
        }
        if content.skipped > 0 {
            children.push(
                style::text_style(div(), typography::BODY, cx)
                    .mt(px(spacing::S4))
                    .text_color(color::text_secondary())
                    .child(if content.skipped == 1 {
                        SharedString::from("1 item can't be shown")
                    } else {
                        format!("{} items can't be shown", content.skipped).into()
                    })
                    .into_any_element(),
            );
        }

        let list = div()
            .id("artist-page")
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus.list)
            .role(gpui::Role::List)
            .aria_label("Artist")
            .size_full()
            .min_h_0();
        Self::list_actions(list, cx)
            .child(
                div()
                    .id("artist-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&column)
                    .pb(px(spacing::S6))
                    .children(children),
            )
            .into_any_element()
    }

    /// "Top songs", with "Show all" at the right when the service has a page of every top song.
    fn top_songs_heading(
        &mut self,
        content: &ArtistContent,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_between()
            .mt(px(spacing::S6))
            .mb(px(spacing::S2))
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("Top songs"),
            )
            .when(content.top_songs_id.is_some(), |d| {
                d.child(selector(
                    "artist-show-all",
                    TextButton::new("artist-show-all", &self.focus.show_all, "Show all", accent)
                        .on_click(move |window, cx| {
                            this.update(cx, |t, cx| {
                                if let Some(all) = t.model.read(cx).top_songs_collection() {
                                    t.open(all, window, cx);
                                }
                            })
                        }),
                ))
            })
            .into_any_element()
    }

    /// The artwork of a card, asking for it the first time it is drawn.
    fn card_thumb(&mut self, url: Option<&str>, cx: &mut Context<Self>) -> crate::url_thumbs::Thumb {
        if let Some(url) = url {
            if !self.model.read(cx).art_requested(url, ArtSize::Card) {
                let url = url.to_owned();
                self.model
                    .update(cx, |m, cx| m.want_art(&url, ArtSize::Card, cx));
            }
        }
        self.model.read(cx).thumb(url, ArtSize::Card)
    }

    /// The bio, clamped to a few lines until it is clicked, and the counts under it. `None` when
    /// the page has neither.
    fn artist_about(
        &mut self,
        content: &ArtistContent,
        bio_open: bool,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let counts: Vec<&str> = [
            content.subscribers.as_deref(),
            content.monthly_listeners.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        if content.description.is_none() && counts.is_empty() {
            return None;
        }
        let model = self.model.clone();
        let focus = self.focus.bio.clone();
        let ring = keyboard_focused(&focus, window);
        let ring_color = color::accent(accent);
        let bio = content.description.clone().map(|text| {
            div()
                .id("artist-bio")
                .debug_selector(|| "artist-bio".into())
                .relative()
                .w_full()
                .max_w(px(BIO_MAX_WIDTH))
                .rounded(px(radius::SMALL))
                .cursor_pointer()
                .track_focus(&focus)
                .role(gpui::Role::Button)
                .aria_label("Bio")
                .on_click(move |event, _, cx| {
                    // Space is the global play/pause key.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == gpui::KeyboardButton::Space {
                            return;
                        }
                    }
                    model.update(cx, |m, cx| m.toggle_bio(cx));
                })
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .text_color(color::text_primary())
                        .when(!bio_open, |d| d.line_clamp(BIO_LINES))
                        .child(SharedString::from(if bio_open {
                            text
                        } else {
                            one_paragraph(&text)
                        })),
                )
                .when(ring, |d| d.child(focus_ring(ring_color, radius::SMALL)))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(spacing::S2))
                .children(bio)
                .when(!counts.is_empty(), |d| {
                    d.child(
                        style::text_style(div(), typography::BODY, cx)
                            .id("artist-counts")
                            .text_color(color::text_secondary())
                            .child(SharedString::from(counts.join(" · "))),
                    )
                })
                .into_any_element(),
        )
    }
}

/// The bio as one paragraph. GPUI's line clamp counts wrapped lines inside a paragraph only: the
/// paragraphs after the first (the service's bios have several, and a "From Wikipedia" credit) are
/// drawn whole, unwrapped and cut off at the edge, so the clamped bio flattens its line breaks.
fn one_paragraph(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::one_paragraph;

    #[test]
    fn a_clamped_bio_is_a_single_paragraph() {
        assert_eq!(
            one_paragraph("First paragraph.\nSecond one.\n\nFrom Wikipedia (https://x)"),
            "First paragraph. Second one. From Wikipedia (https://x)"
        );
        assert_eq!(one_paragraph("  already   one line "), "already one line");
        assert_eq!(one_paragraph(""), "");
    }
}

/// Names a button for the headless tests.
fn selector(name: &'static str, button: TextButton) -> impl IntoElement {
    div().debug_selector(move || name.into()).child(button)
}

/// A shelf of cards in a row that scrolls sideways (wheel or swipe). The ring room and the
/// negative margin let the focus ring and hover tint sit outside the cards while the artwork
/// still lines up with the heading above.
fn shelf(
    id: (&'static str, usize),
    scroll: &ScrollHandle,
    cards: Vec<Card>,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_none()
        .gap(px(spacing::S4))
        .px(px(RING_ROOM))
        .py(px(RING_ROOM))
        .mx(px(-(crate::ui::card::INSET + RING_ROOM)))
        .my(px(-RING_ROOM))
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .track_scroll(scroll)
        .children(cards)
        .into_any_element()
}
