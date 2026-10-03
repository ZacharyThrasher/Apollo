//! Where on screen an element sits, for things that open next to it (the playlist picker).
//!
//! An element places a [`probe`] inside itself; painting records its window-space bounds under
//! a key. Whoever opens the popover looks the key up. Keys name a place, not a row's content
//! (`track-playlist-3`), so a stale entry is only ever read for something that was just drawn.

use std::collections::HashMap;

use gpui::{
    canvas, div, prelude::*, App, Bounds, Global, IntoElement, Pixels, SharedString, Styled,
};

#[derive(Default)]
struct Anchors(HashMap<SharedString, Bounds<Pixels>>);

impl Global for Anchors {}

/// A zero-cost overlay that fills its (positioned) parent and records its bounds under `key`.
pub fn probe(key: impl Into<SharedString>) -> impl IntoElement {
    let key = key.into();
    canvas(
        move |bounds, _, cx| {
            cx.default_global::<Anchors>().0.insert(key.clone(), bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// The bounds last recorded under `key`.
pub fn bounds_of(key: &str, cx: &App) -> Option<Bounds<Pixels>> {
    cx.try_global::<Anchors>()?.0.get(key).copied()
}

/// A positioned wrapper around `child` that records the child's bounds under `key`.
pub fn anchored(key: impl Into<SharedString>, child: impl IntoElement) -> gpui::Div {
    div().relative().flex_none().child(child).child(probe(key))
}
