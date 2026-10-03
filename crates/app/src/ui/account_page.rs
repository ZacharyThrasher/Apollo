//! Settings > Account: sign in, who is signed in, sign out. The only place account and Premium
//! status show (never in the main chrome). Every action is a text button; the page has no
//! helper copy, only the state and what to do about it.

use std::sync::Arc;

use apollo_domain::playback::Event;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, img, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, ImageSource,
    IntoElement, ObjectFit, ParentElement, PathPromptOptions, Render, RenderImage, SharedString,
    Styled, Window,
};

use crate::account::{AccountModel, Method, Notice, Screen, SignInError, Who};
use crate::assets::Icon;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::tooltip::Tooltip;

/// Tab position of the page's controls: after the nav rail, before the player bar. Controls
/// share it and follow paint order.
pub const TAB_INDEX: isize = 6;
const TILE: f32 = 56.0;
const ROW_TILE: f32 = 40.0;

pub struct AccountPage {
    model: Entity<AccountModel>,
    player: Entity<PlayerModel>,
    focus: Vec<FocusHandle>,
}

impl AccountPage {
    pub fn new(
        model: Entity<AccountModel>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            player,
            focus: Vec::new(),
        }
    }

    /// The first `n` focus handles, created as needed.
    fn handles(&mut self, n: usize, cx: &mut Context<Self>) -> Vec<FocusHandle> {
        while self.focus.len() < n {
            self.focus
                .push(cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true));
        }
        self.focus[..n].to_vec()
    }
}

impl Render for AccountPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (screen, busy) = {
            let m = self.model.read(cx);
            (m.screen(), m.busy())
        };
        let accent = self.player.read(cx).view().accent;
        let model = self.model.clone();
        let player = self.player.clone();

        let body: AnyElement = match screen {
            Screen::Checking => skeleton(!cx.reduce_motion()),
            Screen::SignedOut { error } => {
                let h = self.handles(5, cx);
                signed_out(error, busy, &h, accent, &model, window, cx)
            }
            Screen::SignedIn {
                who,
                premium,
                notice,
            } => {
                let h = self.handles(2, cx);
                let avatar = avatar_of(&who, &model, cx);
                let mut col = div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing::S3))
                    .child(identity_row(&who, avatar, premium, cx));
                if let Some(notice) = notice {
                    col = col.child(notice_row(notice, &h[0], accent, &model, cx));
                }
                col.child(
                    div().flex().child(
                        TextButton::new("sign-out", &h[1], "Sign out", accent)
                            .on_click(sign_out_handler(&model, &player)),
                    ),
                )
                .into_any_element()
            }
            Screen::Choose(list) => {
                let h = self.handles(list.len() + 1, cx);
                let mut col = div().flex().flex_col().gap(px(spacing::S1)).child(
                    style::text_style(div(), typography::BODY_STRONG, cx)
                        .px(px(spacing::S2))
                        .pb(px(spacing::S2))
                        .text_color(color::text_primary())
                        .child("Choose account"),
                );
                for (i, who) in list.iter().enumerate() {
                    let avatar = avatar_of(who, &model, cx);
                    col = col.child(choice_row(
                        i, who, avatar, &h[i], accent, &model, window, cx,
                    ));
                }
                col.child(
                    div().flex().pt(px(spacing::S2)).child(
                        TextButton::new("sign-out", &h[list.len()], "Sign out", accent)
                            .on_click(sign_out_handler(&model, &player)),
                    ),
                )
                .into_any_element()
            }
            Screen::CantConfirm(notice) => {
                let h = self.handles(2, cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing::S3))
                    .child(notice_row(notice, &h[0], accent, &model, cx))
                    .child(
                        div().flex().child(
                            TextButton::new("sign-out", &h[1], "Sign out", accent)
                                .on_click(sign_out_handler(&model, &player)),
                        ),
                    )
                    .into_any_element()
            }
        };

        div()
            .id("account-page")
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .w_full()
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("Account"),
            )
            .child(
                div()
                    .p(px(spacing::S4))
                    .rounded(px(radius::MEDIUM))
                    .bg(color::bg_raised())
                    .child(body),
            )
    }
}

fn signed_out(
    error: Option<SignInError>,
    busy: Option<Method>,
    h: &[FocusHandle],
    accent: Accent,
    model: &Entity<AccountModel>,
    _window: &mut Window,
    cx: &mut Context<AccountPage>,
) -> AnyElement {
    let working = busy.is_some();
    let mut col = div().flex().flex_col().gap(px(spacing::S3));
    if let Some(error) = error {
        col = col.child(warning_row(error.text(), cx));
    }
    let browser = {
        let model = model.clone();
        move |_: &mut Window, cx: &mut App| model.update(cx, |m, cx| m.sign_in_browser(cx))
    };
    col = col.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .child(
                TextButton::new("sign-in", &h[0], "Sign in", accent)
                    .primary()
                    .busy(working)
                    .on_click(browser),
            )
            .when(busy == Some(Method::Browser), |d| {
                let model = model.clone();
                d.child(
                    TextButton::new("sign-in-cancel", &h[1], "Cancel", accent)
                        .on_click(move |_, cx| model.update(cx, |m, _| m.cancel())),
                )
            }),
    );
    let firefox = {
        let model = model.clone();
        move |_: &mut Window, cx: &mut App| model.update(cx, |m, cx| m.sign_in_firefox(cx))
    };
    let file = {
        let model = model.clone();
        move |_: &mut Window, cx: &mut App| {
            let picked = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Import".into()),
            });
            let model = model.clone();
            cx.spawn(async move |cx| {
                if let Ok(Ok(Some(paths))) = picked.await {
                    if let Some(path) = paths.into_iter().next() {
                        model.update(cx, |m, cx| m.sign_in_file(path, cx));
                    }
                }
            })
            .detach();
        }
    };
    let clipboard = {
        let model = model.clone();
        move |_: &mut Window, cx: &mut App| {
            let text = cx.read_from_clipboard().and_then(|item| item.text());
            model.update(cx, |m, cx| m.sign_in_clipboard(text, cx));
        }
    };
    col.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap(px(spacing::S1))
            .child(
                TextButton::new("use-firefox", &h[2], "Use Firefox", accent)
                    .enabled(!working)
                    .on_click(firefox),
            )
            .child(
                TextButton::new("import-file", &h[3], "Import file", accent)
                    .enabled(!working)
                    .on_click(file),
            )
            .child(
                TextButton::new("paste-cookies", &h[4], "Paste cookies", accent)
                    .enabled(!working)
                    .on_click(clipboard),
            ),
    )
    .into_any_element()
}

/// Sign out stops playback first (nothing keeps playing for an account that is gone), then
/// drops the session everywhere.
fn sign_out_handler(
    model: &Entity<AccountModel>,
    player: &Entity<PlayerModel>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let (model, player) = (model.clone(), player.clone());
    move |_, cx| {
        player.update(cx, |m, cx| {
            m.handle(Event::Stop, cx);
            // The saved queue is the account's own: it goes with the session.
            m.forget_saved_queue();
        });
        model.update(cx, |m, cx| m.sign_out(cx));
    }
}

fn avatar_of(who: &Who, model: &Entity<AccountModel>, cx: &App) -> Option<Arc<RenderImage>> {
    who.avatar_url
        .as_deref()
        .and_then(|url| model.read(cx).avatar(url))
}

/// The no-art tile: bg.overlay with a centred mark, the account's initial. The channel picture
/// replaces the mark when the service gave one.
fn tile(
    initial: SharedString,
    avatar: Option<Arc<RenderImage>>,
    size: f32,
    cx: &App,
) -> impl IntoElement {
    let base = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .overflow_hidden()
        .rounded(px(radius::MEDIUM))
        .bg(color::bg_overlay());
    match avatar {
        Some(image) => base.child(
            img(ImageSource::Render(image))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .rounded(px(radius::MEDIUM)),
        ),
        None => base.child(
            style::text_style(div(), typography::SECTION, cx)
                .text_color(color::text_primary())
                .child(initial),
        ),
    }
}

fn initial_of(name: &str) -> SharedString {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into())
        .into()
}

fn identity_row(
    who: &Who,
    avatar: Option<Arc<RenderImage>>,
    premium: Option<&'static str>,
    cx: &App,
) -> impl IntoElement {
    let secondary = |text: SharedString, id: &'static str| {
        style::text_style(div(), typography::BODY, cx)
            .id(id)
            .truncate()
            .text_color(color::text_secondary())
            .child(text)
    };
    let name = who.name.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .child(tile(initial_of(&who.name), avatar, TILE, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    style::text_style(div(), typography::LIST_TITLE, cx)
                        .id("account-name")
                        .truncate()
                        .text_color(color::text_primary())
                        .tooltip({
                            let name = name.clone();
                            move |_, cx| Tooltip::view(name.clone(), cx)
                        })
                        .child(name),
                )
                .children(who.handle.clone().map(|h| secondary(h, "account-handle")))
                .children(premium.map(|p| secondary(p.into(), "account-premium"))),
        )
}

fn warning_row(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S2))
        .h(px(layout::MIN_HIT_TARGET))
        .child(
            gpui::svg()
                .path(Icon::Warning.path())
                .size(px(layout::ICON_SIZE))
                .flex_none()
                .text_color(color::warning()),
        )
        .child(
            style::text_style(div(), typography::BODY, cx)
                .truncate()
                .text_color(color::text_secondary())
                .child(text),
        )
}

fn notice_row(
    notice: Notice,
    focus: &FocusHandle,
    accent: Accent,
    model: &Entity<AccountModel>,
    cx: &App,
) -> impl IntoElement {
    let model = model.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S2))
        .child(warning_row(notice.text(), cx))
        .when(notice.retryable(), |d| {
            d.child(
                TextButton::new("account-retry", focus, "Retry", accent)
                    .strong()
                    .on_click(move |_, cx| model.update(cx, |m, cx| m.retry(cx))),
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn choice_row(
    index: usize,
    who: &Who,
    avatar: Option<Arc<RenderImage>>,
    focus: &FocusHandle,
    accent: Accent,
    model: &Entity<AccountModel>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let ring = keyboard_focused(focus, window);
    let key = who.key.clone();
    let model = model.clone();
    let mut texts = div().flex().flex_col().flex_1().min_w_0().child(
        style::text_style(div(), typography::BODY_STRONG, cx)
            .truncate()
            .text_color(color::text_primary())
            .child(who.name.clone()),
    );
    if let Some(handle) = who.handle.clone() {
        texts = texts.child(
            style::text_style(div(), typography::BODY, cx)
                .truncate()
                .text_color(color::text_secondary())
                .child(handle),
        );
    }
    div()
        .id(("account-choice", index))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .px(px(spacing::S2))
        .py(px(spacing::S2))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .track_focus(focus)
        .role(gpui::Role::Button)
        .aria_label(who.name.clone())
        .hover(|s| s.bg(color::state_hover()))
        .on_click(move |event, _, cx| {
            if let gpui::ClickEvent::Keyboard(k) = event {
                if k.button == gpui::KeyboardButton::Space {
                    return;
                }
            }
            let key = key.clone();
            model.update(cx, |m, cx| m.choose(key, cx));
        })
        .child(tile(initial_of(&who.name), avatar, ROW_TILE, cx))
        .child(texts)
        .when(ring, |d| {
            d.child(focus_ring(color::accent(accent), radius::SMALL))
        })
}

/// Blocks at final size (white 4%) so nothing shifts when the state arrives. They pulse gently
/// unless the system has animation effects off.
fn skeleton(animate: bool) -> AnyElement {
    let block = |id: &'static str, w: Option<f32>, h: f32, r: f32| -> AnyElement {
        let el = div()
            .flex_none()
            .h(px(h))
            .rounded(px(r))
            .bg(style::hsla(apollo_theme::tokens::color::SKELETON));
        let el = match w {
            Some(w) => el.w(px(w)),
            None => el.size(px(h)),
        };
        skeleton::pulse(el, id, animate)
    };
    div()
        .id("account-skeleton")
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .child(block("sk-tile", None, TILE, radius::MEDIUM))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(spacing::S2))
                .child(block("sk-name", Some(160.0), 16.0, radius::SMALL))
                .child(block("sk-sub", Some(96.0), 12.0, radius::SMALL)),
        )
        .into_any_element()
}
