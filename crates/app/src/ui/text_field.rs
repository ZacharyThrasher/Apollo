//! Text field (one line, or a few wrapped rows for a description) with keyboard, clipboard and IME input (adapted from GPUI's `input`
//! example, as proven in the M0 spike), drawn with theme tokens.
//!
//! It claims Space in its own key context, so typing a space never reaches the shell's global
//! play/pause binding. The owner listens for [`TextFieldEvent`]s and draws the focus ring
//! (keyboard focus only, DIRECTION.md) around the field with [`TextField::focus_handle`].
//! Escape on an empty field propagates to the owner's `Escape` action handler.
//!
//! A multi-line field ([`TextField::multiline`]) wraps its text to the field's width, shows a fixed
//! number of rows and scrolls to keep the caret in view; Enter is a newline there, and Up and Down
//! move between rows.

use std::ops::Range;
use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::assets::Icon;
use crate::style::{self, color};
use crate::ui::button::IconButton;

actions!(
    apollo_text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Enter,
        Escape,
        InsertSpace,
        Up,
        Down,
    ]
);

/// Key context of the field; its bindings win over the shell's.
pub const KEY_CONTEXT: &str = "TextField";

/// Added to the context of a multi-line field, which alone takes Up and Down.
const MULTILINE_CONTEXT: &str = "TextField multiline";

/// Longest text the field accepts, in characters.
pub const MAX_CHARS: usize = 200;

const FIELD_HEIGHT: f32 = 40.0;
const CARET_WIDTH: f32 = 1.0;

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("ctrl-c", Copy, ctx),
        KeyBinding::new("ctrl-x", Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Enter, ctx),
        KeyBinding::new("escape", Escape, ctx),
        // The shell binds a bare Space to play/pause; here it is a character.
        KeyBinding::new("space", InsertSpace, ctx),
        KeyBinding::new("up", Up, Some("TextField && multiline")),
        KeyBinding::new("down", Down, Some("TextField && multiline")),
    ]);
}

/// Put the caret in `field`.
pub fn focus(field: &Entity<TextField>, window: &mut Window, cx: &mut App) {
    let handle = field.read(cx).focus_handle.clone();
    window.focus(&handle, cx);
}

/// What a paste does instead of typing: it is given the clipboard's text and says whether it took
/// it (`true`: nothing is typed). It must not update the field itself.
pub type PasteHook = Rc<dyn Fn(&str, &mut Window, &mut App) -> bool>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFieldEvent {
    /// The text changed (typing, paste, clear).
    Changed,
    /// Enter.
    Submit,
}

pub struct TextField {
    focus_handle: FocusHandle,
    clear_focus: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    /// A multi-line field's shaped paragraphs and the row height they were shaped for.
    last_lines: Option<(Vec<WrappedLine>, Pixels)>,
    /// How far a multi-line field's text is scrolled up, to keep the caret in view.
    scroll_y: Pixels,
    /// Rows a multi-line field shows; 0 is a one-line field.
    rows: usize,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// A plain field is for naming something (a dialog's): no search icon, no clear button, and
    /// Escape always goes to the owner.
    plain: bool,
    /// Set in the page-title type, on a raised fill, for renaming a page's title in place.
    title: bool,
    /// While the field is empty a Space goes to the owner instead of typing a leading space
    /// (the playlist picker marks the row under its cursor with it).
    space_to_owner_when_empty: bool,
    max_chars: usize,
    /// Offered every paste before it is typed (the Crate's add box reads links and lists).
    on_paste: Option<PasteHook>,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl TextField {
    /// `focus` and `clear_focus` are created by the owner so it controls their tab order.
    pub fn new(
        placeholder: impl Into<SharedString>,
        focus: FocusHandle,
        clear_focus: FocusHandle,
    ) -> Self {
        Self {
            focus_handle: focus,
            clear_focus,
            content: SharedString::default(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_lines: None,
            scroll_y: px(0.),
            rows: 0,
            last_bounds: None,
            is_selecting: false,
            plain: false,
            title: false,
            space_to_owner_when_empty: false,
            max_chars: MAX_CHARS,
            on_paste: None,
        }
    }

    /// Offer every paste to `hook` before it is typed.
    pub fn on_paste(mut self, hook: PasteHook) -> Self {
        self.on_paste = Some(hook);
        self
    }

    /// While the field is empty, Space propagates to the owner's `InsertSpace` handler rather
    /// than typing a space in front of nothing.
    pub fn space_to_owner_when_empty(mut self) -> Self {
        self.space_to_owner_when_empty = true;
        self
    }

    /// Make this a plain naming field that takes at most `max_chars` characters.
    pub fn plain(mut self, max_chars: usize) -> Self {
        self.plain = true;
        self.max_chars = max_chars;
        self
    }

    /// A one-line plain field that looks like a page title: the title's type, no border, a raised
    /// fill that reaches a little past the text (the text itself stays where the title was).
    pub fn title(mut self, max_chars: usize) -> Self {
        self.title = true;
        self.plain(max_chars)
    }

    /// Make this a plain field of `rows` wrapped rows that takes at most `max_chars` characters.
    pub fn multiline(mut self, rows: usize, max_chars: usize) -> Self {
        self.rows = rows.max(2);
        self.plain(max_chars)
    }

    fn is_multiline(&self) -> bool {
        self.rows > 0
    }

    /// Select all the text, so typing replaces it (a rename starts from the current name).
    pub fn select_everything(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replace the text from outside (the clear button). Emits `Changed`.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = truncate_chars(text, self.max_chars).into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.is_multiline() {
            self.content[..self.cursor_offset()]
                .rfind('\n')
                .map_or(0, |i| i + 1)
        } else {
            0
        };
        self.move_to(to, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.is_multiline() {
            let at = self.cursor_offset();
            self.content[at..].find('\n').map_or(self.content.len(), |i| at + i)
        } else {
            self.content.len()
        };
        self.move_to(to, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, cx);
    }

    /// Move the caret `rows` rows up or down, keeping its column; past the first or last row it
    /// goes to the start or the end.
    fn move_rows(&mut self, rows: i32, cx: &mut Context<Self>) {
        let Some((lines, lh)) = self.last_lines.as_ref() else {
            return;
        };
        let at = position_in(lines, self.cursor_offset(), *lh);
        let y = at.y + *lh * rows as f32;
        let to = if y < px(0.) {
            0
        } else if y >= total_height(lines, *lh) {
            self.content.len()
        } else {
            index_in(lines, point(at.x, y), *lh)
        };
        self.move_to(to, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_multiline() {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(TextFieldEvent::Submit);
        }
    }

    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.plain || self.content.is_empty() {
            // Nothing to clear: let the owner (an ancestor's handler) decide where focus goes.
            cx.propagate();
        } else {
            self.set_text("", cx);
        }
    }

    fn insert_space(&mut self, _: &InsertSpace, window: &mut Window, cx: &mut Context<Self>) {
        if self.space_to_owner_when_empty && self.content.is_empty() {
            cx.propagate();
            return;
        }
        self.replace_text_in_range(None, " ", window, cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if let Some(hook) = self.on_paste.clone() {
                if hook(&text, window, cx) {
                    return;
                }
            }
            let text = if self.is_multiline() {
                multi_line(&text)
            } else {
                single_line(&text)
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        if let (Some(bounds), Some((lines, lh))) = (self.last_bounds.as_ref(), self.last_lines.as_ref())
        {
            if position.y > bounds.bottom() {
                return self.content.len();
            }
            let at = point(
                position.x - bounds.left(),
                position.y - bounds.top() + self.scroll_y,
            );
            return index_in(lines, at, *lh).min(self.content.len());
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// `new_text` cut so the result stays within the field's limit after `range` is replaced.
    fn fit(&self, range: &Range<usize>, new_text: &str) -> String {
        let kept =
            self.content[..range.start].chars().count() + self.content[range.end..].chars().count();
        truncate_chars(new_text, self.max_chars.saturating_sub(kept))
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = self.fit(&range, new_text);

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = self.fit(&range, new_text);

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .map(|r| r.start.min(self.content.len())..r.end.min(self.content.len()))
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        if let Some((lines, lh)) = self.last_lines.as_ref() {
            let from = position_in(lines, range.start, *lh);
            let to = position_in(lines, range.end, *lh);
            let origin = point(bounds.left(), bounds.top() - self.scroll_y);
            return Some(Bounds::from_corners(
                point(origin.x + from.x, origin.y + from.y),
                point(origin.x + to.x, origin.y + to.y + *lh),
            ));
        }
        let last_layout = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        if let Some((lines, lh)) = self.last_lines.as_ref() {
            let at = gpui::point(line_point.x, line_point.y + self.scroll_y);
            return Some(self.offset_to_utf16(index_in(lines, at, *lh).min(self.content.len())));
        }
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// The text line inside the field: shaped here so the caret, selection and IME bounds line up.
struct TextElement {
    input: Entity<TextField>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    /// A multi-line field's paragraphs, the row height, the scroll and the selection's boxes.
    lines: Option<(Vec<WrappedLine>, Pixels, Pixels)>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let rows = self.input.read(cx).rows.max(1);
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), color::text_tertiary())
        } else {
            (content, color::text_primary())
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input
            .marked_range
            .as_ref()
            .filter(|_| !input.content.is_empty())
        {
            Some(marked_range) => vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        if input.is_multiline() {
            let lh = window.line_height();
            let wrapped = window
                .text_system()
                .shape_text(display_text, font_size, &runs, Some(bounds.size.width), None)
                .map(|lines| lines.into_vec())
                .unwrap_or_default();
            let showing_placeholder = input.content.is_empty();
            let at = if showing_placeholder {
                point(px(0.), px(0.))
            } else {
                position_in(&wrapped, cursor, lh)
            };
            // Keep the caret's row in view.
            let mut scroll = input.scroll_y;
            if at.y < scroll {
                scroll = at.y;
            }
            if at.y + lh > scroll + bounds.size.height {
                scroll = at.y + lh - bounds.size.height;
            }
            let most = (total_height(&wrapped, lh) - bounds.size.height).max(px(0.));
            let scroll = scroll.clamp(px(0.), most);
            let origin = point(bounds.left(), bounds.top() - scroll);
            let mut boxes = Vec::new();
            let mut caret = None;
            if selected_range.is_empty() {
                caret = Some(fill(
                    Bounds::new(
                        point(origin.x + at.x, origin.y + at.y),
                        size(px(CARET_WIDTH), lh),
                    ),
                    color::text_primary(),
                ));
            } else {
                for (y, x0, x1) in selection_boxes(&wrapped, &input.content, &selected_range, lh) {
                    boxes.push(fill(
                        Bounds::from_corners(
                            point(origin.x + x0, origin.y + y),
                            point(origin.x + x1, origin.y + y + lh),
                        ),
                        color::border_strong(),
                    ));
                }
            }
            return PrepaintState {
                line: None,
                lines: Some((wrapped, lh, scroll)),
                cursor: caret,
                selection: boxes,
            };
        }
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let showing_placeholder = input.content.is_empty();
        let cursor_pos = if showing_placeholder {
            px(0.)
        } else {
            line.x_for_index(cursor)
        };
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(CARET_WIDTH), bounds.bottom() - bounds.top()),
                    ),
                    color::text_primary(),
                )),
            )
        } else {
            (
                vec![fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    color::border_strong(),
                )],
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            lines: None,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some((lines, lh, scroll)) = prepaint.lines.take() {
            // Only the field's own box shows: the text is shifted up by `scroll`.
            let mask = gpui::ContentMask { bounds };
            window.with_content_mask(Some(mask), |window| {
                for quad in prepaint.selection.drain(..) {
                    window.paint_quad(quad)
                }
                let mut y = px(0.);
                for line in &lines {
                    let origin = point(bounds.left(), bounds.top() - scroll + y);
                    let _ = line.paint(origin, lh, gpui::TextAlign::Left, None, window, cx);
                    y += line.size(lh).height;
                }
                if focus_handle.is_focused(window) {
                    if let Some(cursor) = prepaint.cursor.take() {
                        window.paint_quad(cursor);
                    }
                }
            });
            let typed = !self.input.read(cx).content.is_empty();
            self.input.update(cx, |input, _cx| {
                input.last_lines = typed.then_some((lines, lh));
                input.scroll_y = scroll;
                input.last_bounds = Some(bounds);
            });
            return;
        }
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        // The empty field shows its placeholder, so what is measured for the mouse and the IME
        // is the typed text only: nothing to measure while it is empty.
        let typed = !self.input.read(cx).content.is_empty();
        self.input.update(cx, |input, _cx| {
            input.last_layout = typed.then_some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let clear_target = entity.clone();
        let plain = self.plain;
        let multiline = self.is_multiline();
        let rows = self.rows;
        let has_text = !self.content.is_empty() && !plain;
        let title = self.title;
        let size = if title {
            typography::PAGE_TITLE
        } else {
            typography::LIST_TITLE
        };
        style::text_style(div(), size, cx)
            .id("text-field")
            .key_context(if multiline { MULTILINE_CONTEXT } else { KEY_CONTEXT })
            .track_focus(&self.focus_handle)
            .role(gpui::Role::TextInput)
            .aria_label(self.placeholder.clone())
            .aria_value(self.content.clone())
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::insert_space))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .when(!multiline && !title, |d| d.h(px(FIELD_HEIGHT)))
            .when(multiline, |d| {
                d.items_start()
                    .py(px(spacing::S2))
                    .h(px(layout::ICON_SIZE * rows as f32 + spacing::S2 * 2.0 + 2.0))
            })
            .w_full()
            .px(px(if title { spacing::S2 } else { spacing::S3 }))
            .rounded(px(radius::SMALL))
            .bg(color::bg_raised())
            .when(!title, |d| d.border_1().border_color(color::border_subtle()))
            // The text of a title field sits where the title's text was.
            .when(title, |d| d.ml(px(-spacing::S2)))
            .text_color(color::text_primary())
            .when(!title, |d| d.line_height(px(layout::ICON_SIZE)))
            .when(!plain, |d| {
                d.child(
                    gpui::svg()
                        .path(Icon::SearchRegular.path())
                        .size(px(layout::ICON_SIZE))
                        .flex_none()
                        .text_color(color::text_secondary()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(TextElement { input: entity }),
            )
            .when(has_text, |d| {
                d.child(
                    IconButton::new(
                        "text-field-clear",
                        &self.clear_focus,
                        Icon::Dismiss,
                        "Clear",
                        Accent::fallback(),
                    )
                    .on_click(move |_, cx| {
                        clear_target.update(cx, |this, cx| this.set_text("", cx))
                    }),
                )
            })
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Pasted text for a multi-line field: line breaks stay, tabs become spaces, other control
/// characters go.
fn multi_line(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Where `offset` is in the shaped paragraphs, from the top left of the text. Paragraphs are
/// the pieces between line breaks, each taking as many rows as it wrapped to.
fn position_in(lines: &[WrappedLine], offset: usize, lh: Pixels) -> Point<Pixels> {
    let (mut start, mut y) = (0, px(0.));
    for (i, line) in lines.iter().enumerate() {
        let end = start + line.len();
        if offset <= end || i + 1 == lines.len() {
            let local = offset.saturating_sub(start).min(line.len());
            let at = line
                .position_for_index(local, lh)
                .unwrap_or_else(|| point(px(0.), px(0.)));
            return point(at.x, y + at.y);
        }
        y += line.size(lh).height;
        start = end + 1;
    }
    point(px(0.), px(0.))
}

/// The byte offset nearest `at` (from the top left of the text).
fn index_in(lines: &[WrappedLine], at: Point<Pixels>, lh: Pixels) -> usize {
    let (mut start, mut y) = (0, px(0.));
    for (i, line) in lines.iter().enumerate() {
        let height = line.size(lh).height;
        if at.y < y + height || i + 1 == lines.len() {
            let local = point(at.x.max(px(0.)), (at.y - y).max(px(0.)));
            let index = match line.closest_index_for_position(local, lh) {
                Ok(i) | Err(i) => i,
            };
            return start + index.min(line.len());
        }
        y += height;
        start += line.len() + 1;
    }
    0
}

fn total_height(lines: &[WrappedLine], lh: Pixels) -> Pixels {
    lines
        .iter()
        .fold(px(0.), |height, line| height + line.size(lh).height)
}

/// The selected text as boxes, one per row it touches: `(top, left, right)`. A selected line
/// break shows as a narrow stub at its row's end.
fn selection_boxes(
    lines: &[WrappedLine],
    text: &str,
    range: &Range<usize>,
    lh: Pixels,
) -> Vec<(Pixels, Pixels, Pixels)> {
    let mut boxes: Vec<(Pixels, Pixels, Pixels)> = Vec::new();
    let mut add = |y: Pixels, x0: Pixels, x1: Pixels| match boxes.last_mut() {
        Some((by, _, bx1)) if *by == y && (x0 - *bx1).abs() < px(0.5) => *bx1 = x1,
        _ => boxes.push((y, x0, x1)),
    };
    for (start, grapheme) in text[range.clone()].grapheme_indices(true) {
        let from = range.start + start;
        let to = from + grapheme.len();
        let a = position_in(lines, from, lh);
        let b = position_in(lines, to, lh);
        if grapheme == "\n" || b.y != a.y {
            add(a.y, a.x, a.x + px(4.));
        } else {
            add(a.y, a.x, b.x);
        }
    }
    boxes
}

/// Newlines and tabs in pasted text become spaces: the field is one line.
fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_text_is_one_line() {
        assert_eq!(single_line("a\nb\tc\r\nd"), "a b c  d");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("héllo", 3), "hél");
        assert_eq!(truncate_chars("ab", 5), "ab");
    }
}
