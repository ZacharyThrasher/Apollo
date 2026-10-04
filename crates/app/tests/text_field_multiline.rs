//! Headless tests for the multi-line text field (the Edit playlist description): Enter is a line
//! break, Up and Down move between rows (wrapped rows too), Home and End work on the row's line,
//! and the text is limited like any field's.

use apollo_app::style::Fonts;
use apollo_app::ui::text_field::{self, TextField, TextFieldEvent};
use gpui::{
    div, px, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TestAppContext, VisualTestContext, Window,
};

/// A narrow column with the field in it, so a long line has to wrap.
struct Narrow(Entity<TextField>);

impl Render for Narrow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(220.0)).child(self.0.clone())
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<TextField>, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        text_field::bind_keys(cx);
    });
    let (field, vcx) = cx.add_window_view(|_, cx| {
        TextField::new("Description", cx.focus_handle(), cx.focus_handle()).multiline(3, 200)
    });
    vcx.update(|window, cx| text_field::focus(&field, window, cx));
    vcx.run_until_parked();
    (field, vcx)
}

fn text(field: &Entity<TextField>, vcx: &mut VisualTestContext) -> String {
    field.read_with(vcx, |f, _| f.text().to_owned())
}

#[gpui::test]
fn enter_is_a_line_break_and_does_not_submit(cx: &mut TestAppContext) {
    let (field, vcx) = setup(cx);
    let submitted = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = submitted.clone();
    let _subscription = vcx.update(|_, cx| {
        cx.subscribe(&field, move |_, event: &TextFieldEvent, _| {
            if *event == TextFieldEvent::Submit {
                seen.set(true);
            }
        })
    });
    vcx.simulate_input("one");
    vcx.simulate_keystrokes("enter");
    vcx.simulate_input("two");
    assert_eq!(text(&field, vcx), "one\ntwo");
    assert!(!submitted.get());
}

#[gpui::test]
fn up_and_down_move_between_lines_keeping_the_column(cx: &mut TestAppContext) {
    let (field, vcx) = setup(cx);
    vcx.simulate_input("abcd");
    vcx.simulate_keystrokes("enter");
    vcx.simulate_input("ef");
    // The caret is after "ef". Up goes to the line above, near the same column.
    vcx.simulate_keystrokes("up");
    vcx.simulate_input("X");
    let t = text(&field, vcx);
    assert!(t.starts_with("abc") || t.starts_with("ab"), "{t}");
    assert!(t.contains('X') && t.find('X').unwrap() < 5, "X went on the first line: {t}");
    // Down again, then past the last line, which goes to the end.
    vcx.simulate_keystrokes("down down");
    vcx.simulate_input("Y");
    assert!(text(&field, vcx).ends_with("efY"));
    // Up from the first line goes to the very start.
    vcx.simulate_keystrokes("up up up");
    vcx.simulate_input("S");
    assert!(text(&field, vcx).starts_with('S'));
}

#[gpui::test]
fn home_and_end_stay_on_the_line(cx: &mut TestAppContext) {
    let (field, vcx) = setup(cx);
    vcx.simulate_input("abc");
    vcx.simulate_keystrokes("enter");
    vcx.simulate_input("def");
    vcx.simulate_keystrokes("home");
    vcx.simulate_input("1");
    vcx.simulate_keystrokes("up");
    vcx.simulate_keystrokes("end");
    vcx.simulate_input("2");
    assert_eq!(text(&field, vcx), "abc2\n1def");
}

#[gpui::test]
fn a_long_line_wraps_and_up_moves_one_wrapped_row(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        text_field::bind_keys(cx);
    });
    let field = cx.new(|cx| {
        TextField::new("Description", cx.focus_handle(), cx.focus_handle()).multiline(3, 200)
    });
    let (_, vcx) = cx.add_window_view({
        let field = field.clone();
        move |_, _| Narrow(field)
    });
    vcx.update(|window, cx| text_field::focus(&field, window, cx));
    vcx.run_until_parked();
    let long = "MMMM ".repeat(38);
    field.update(vcx, |f, cx| f.set_text(long.trim_end(), cx));
    vcx.run_until_parked();
    let len = text(&field, vcx).len();
    // One paragraph that wrapped: Up stays inside it, so the caret is not at the end any more.
    vcx.simulate_keystrokes("up");
    vcx.simulate_input("X");
    let t = text(&field, vcx);
    let at = t.find('X').expect("typed");
    assert!(at > 0 && at < len, "the caret moved up one wrapped row, to {at} of {len}");
}

#[gpui::test]
fn pasted_line_breaks_stay_and_the_limit_counts_them(cx: &mut TestAppContext) {
    let (field, vcx) = setup(cx);
    field.update(vcx, |f, cx| f.set_text(&"a\n".repeat(150), cx));
    assert_eq!(text(&field, vcx).chars().count(), 200, "cut to the limit");
    assert!(text(&field, vcx).contains('\n'));
}
