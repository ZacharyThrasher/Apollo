//! Where a window opens: the size, position and maximized state a window had when the app last
//! ran, put back on a monitor that still exists.
//!
//! * **The decision** ([`place`]) is plain arithmetic over rectangles in logical pixels, so it is
//!   tested without a window. A saved position that still has a grabbable strip of its top edge
//!   on some monitor is kept (nudged fully on to that monitor); one that does not (the monitor
//!   was unplugged, or the layout changed) falls back to the window's default spot on the
//!   primary monitor.
//! * **The record** ([`SavedWindow`]) is one short string in the `settings` table, one key per
//!   window (the main window and the mini player).
//! * **The recorder** ([`remember`]) watches a live window and writes the record down a moment
//!   after it stops moving or resizing, and once more when the app quits.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{px, App, Bounds, Context, Pixels, Point, Size, Subscription, Window, WindowBounds};

use crate::settings::SettingsBackend;

/// The main window's record.
pub const KEY_MAIN: &str = "window_main";
/// The mini player's record.
pub const KEY_MINI: &str = "window_mini";
/// The game HUD's own place (it is a different size from the mini player). The `2` is for the
/// HUD's size change (it lost its shadow margin): places saved at the old size are not reused.
pub const KEY_HUD: &str = "window_hud2";

/// How long a window must sit still before its place is written down.
const SAVE_DELAY: Duration = Duration::from_millis(600);
/// The strip of a window's top edge that must still show on a monitor for the window to be
/// reachable: enough to grab and drag.
const GRAB_WIDTH: f32 = 96.0;
const GRAB_HEIGHT: f32 = 32.0;
/// How far a fixed-size window's reported size may be from its size and still count as it
/// (rounding between logical and physical pixels).
const FIXED_SIZE_SLACK: f32 = 2.0;
/// Gap from the monitor edge for a window parked in a corner (the taskbar sits in the bottom
/// margin).
const CORNER_MARGIN_X: f32 = 24.0;
const CORNER_MARGIN_Y: f32 = 72.0;

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// This rectangle drawn in by `by` on every side.
    pub fn inset(&self, by: f32) -> Self {
        Self::new(
            self.x + by,
            self.y + by,
            self.w - 2.0 * by,
            self.h - 2.0 * by,
        )
    }

    /// This rectangle grown by `by` on every side.
    pub fn outset(&self, by: f32) -> Self {
        self.inset(-by)
    }

    /// The overlap with `other` as (width, height); both zero when they do not touch.
    fn overlap(&self, other: &Rect) -> (f32, f32) {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        (w, h)
    }

    fn is_sane(&self) -> bool {
        [self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite())
            && self.w > 0.0
            && self.h > 0.0
            // Nothing a real monitor layout produces is this far out.
            && self.x.abs() < 1.0e6
            && self.y.abs() < 1.0e6
            && self.w < 1.0e5
            && self.h < 1.0e5
    }
}

/// A window's remembered place: where it sits when not maximized, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedWindow {
    pub rect: Rect,
    pub maximized: bool,
}

impl SavedWindow {
    /// `x,y,w,h,m`, whole pixels, `m` being 1 for maximized.
    pub fn encode(&self) -> String {
        let r = &self.rect;
        format!(
            "{},{},{},{},{}",
            r.x.round() as i64,
            r.y.round() as i64,
            r.w.round() as i64,
            r.h.round() as i64,
            u8::from(self.maximized)
        )
    }

    /// The record `encode` wrote. Anything else (an older format, a damaged value) is `None`
    /// and the window opens at its default place.
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.trim().split(',');
        let mut number = || parts.next()?.trim().parse::<i64>().ok();
        let (x, y, w, h) = (number()?, number()?, number()?, number()?);
        let maximized = match parts.next()?.trim() {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        if parts.next().is_some() {
            return None;
        }
        let rect = Rect::new(x as f32, y as f32, w as f32, h as f32);
        rect.is_sane().then_some(Self { rect, maximized })
    }
}

/// Where a window with no usable record goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    /// Centred on the primary monitor.
    Center,
    /// The primary monitor's bottom-right corner, clear of the taskbar.
    BottomRight,
}

/// What a window needs to be placed.
#[derive(Clone, Copy, Debug)]
pub struct Constraints {
    pub default_size: (f32, f32),
    pub min_size: (f32, f32),
    pub home: Home,
    /// The window has one size and never maximizes (the mini player, the HUD): only its
    /// position is remembered, and whatever size or maximized flag a record carries is ignored.
    pub fixed: bool,
}

/// The place a window opens at: the saved one kept on a monitor, or its default on the primary.
///
/// `displays` are the monitors' bounds; `primary` is the one to fall back to (the first display
/// when absent). With no monitors at all the saved rect (or the default size at the origin) is
/// returned as it is.
pub fn place(
    saved: Option<SavedWindow>,
    displays: &[Rect],
    primary: Option<Rect>,
    c: &Constraints,
) -> SavedWindow {
    let primary = primary.or_else(|| displays.first().copied());
    let default_rect = |size: (f32, f32)| match primary {
        Some(d) => home_rect(&d, size, c),
        None => Rect::new(0.0, 0.0, size.0, size.1),
    };
    let Some(saved) = saved else {
        return SavedWindow {
            rect: default_rect(c.default_size),
            maximized: false,
        };
    };
    // A fixed window keeps only the saved position; a record of any other size (a window that
    // was once stretched or maximized) is no reason to open big.
    let (size, maximized) = if c.fixed {
        (c.default_size, false)
    } else {
        (
            (
                saved.rect.w.max(c.min_size.0),
                saved.rect.h.max(c.min_size.1),
            ),
            saved.maximized,
        )
    };
    // The monitor that shows the most of the grabbable strip, if any shows enough.
    let strip = Rect::new(saved.rect.x, saved.rect.y, size.0, GRAB_HEIGHT.min(size.1));
    let target = displays
        .iter()
        .filter_map(|d| {
            let (w, h) = strip.overlap(d);
            (w >= GRAB_WIDTH.min(size.0) && h >= GRAB_HEIGHT.min(size.1) * 0.75)
                .then_some((d, w * h))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(d, _)| *d);
    match target {
        Some(d) => SavedWindow {
            rect: fit_inside(Rect::new(saved.rect.x, saved.rect.y, size.0, size.1), &d, c),
            maximized,
        },
        None => SavedWindow {
            // Its monitor is gone: the saved size, at home on the primary.
            rect: match primary {
                Some(d) => home_rect(&d, (size.0, size.1), c),
                None => Rect::new(saved.rect.x, saved.rect.y, size.0, size.1),
            },
            maximized,
        },
    }
}

/// `size` placed at the default home inside `display`, shrunk to fit it.
fn home_rect(display: &Rect, size: (f32, f32), c: &Constraints) -> Rect {
    let w = size.0.min(display.w).max(c.min_size.0.min(display.w));
    let h = size.1.min(display.h).max(c.min_size.1.min(display.h));
    let (x, y) = match c.home {
        Home::Center => (
            display.x + (display.w - w) / 2.0,
            display.y + (display.h - h) / 2.0,
        ),
        Home::BottomRight => (
            display.right() - w - CORNER_MARGIN_X,
            display.bottom() - h - CORNER_MARGIN_Y,
        ),
    };
    fit_inside(Rect::new(x, y, w, h), display, c)
}

/// `rect` shrunk to the monitor if bigger, then moved so no edge leaves it.
fn fit_inside(rect: Rect, display: &Rect, c: &Constraints) -> Rect {
    let w = rect.w.min(display.w).max(c.min_size.0.min(display.w));
    let h = rect.h.min(display.h).max(c.min_size.1.min(display.h));
    let x = rect
        .x
        .clamp(display.x, (display.right() - w).max(display.x));
    let y = rect
        .y
        .clamp(display.y, (display.bottom() - h).max(display.y));
    Rect::new(x, y, w, h)
}

/// Gap a snapped window keeps from the monitor edge.
pub const SNAP_MARGIN: f32 = 16.0;
/// How near an edge a window must be dropped to snap to it.
pub const SNAP_REACH: f32 = 48.0;

/// Where a window dropped at `rect` ends up: against any monitor edge it is within
/// [`SNAP_REACH`] of, [`SNAP_MARGIN`] in from it (both axes at once make a corner), and
/// otherwise where it is. Only the monitor showing most of it counts; a window on no monitor
/// is left alone. `displays` should be the usable area (the primary's without the taskbar).
pub fn snap(rect: Rect, displays: &[Rect]) -> Rect {
    let area = |d: &Rect| {
        let (w, h) = rect.overlap(d);
        w * h
    };
    let Some(d) = displays
        .iter()
        .filter(|d| area(d) > 0.0)
        .max_by(|a, b| area(a).total_cmp(&area(b)))
    else {
        return rect;
    };
    // Close enough to a side to jump to its margin line; the nearer side wins a tie.
    let axis = |pos: f32, len: f32, lo: f32, hi: f32| {
        let (near, far) = (lo + SNAP_MARGIN, hi - SNAP_MARGIN - len);
        let (to_near, to_far) = ((pos - near).abs(), (pos - far).abs());
        if to_near <= SNAP_REACH && to_near <= to_far {
            near
        } else if to_far <= SNAP_REACH {
            far
        } else {
            pos
        }
    };
    Rect::new(
        axis(rect.x, rect.w, d.x, d.right()),
        axis(rect.y, rect.h, d.y, d.bottom()),
        rect.w,
        rect.h,
    )
}

// ---- GPUI side -------------------------------------------------------------------------------

impl Rect {
    pub fn from_bounds(b: Bounds<Pixels>) -> Self {
        Self {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            w: f32::from(b.size.width),
            h: f32::from(b.size.height),
        }
    }

    pub fn to_bounds(self) -> Bounds<Pixels> {
        Bounds {
            origin: Point {
                x: px(self.x),
                y: px(self.y),
            },
            size: Size {
                width: px(self.w),
                height: px(self.h),
            },
        }
    }
}

impl SavedWindow {
    /// The window's place now: its restore bounds, and whether it is maximized. (Fullscreen
    /// and minimized windows report the bounds they return to.)
    pub fn of(window: &Window) -> Self {
        let (bounds, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(b) => (b, false),
            WindowBounds::Maximized(b) => (b, true),
            WindowBounds::Fullscreen(b) => (b, false),
        };
        Self {
            rect: Rect::from_bounds(bounds),
            maximized,
        }
    }

    pub fn window_bounds(&self) -> WindowBounds {
        let bounds = self.rect.to_bounds();
        if self.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }
}

/// The bounds a window under `key` should open with: its saved place kept on a monitor that
/// still exists, or `c`'s default.
pub fn restore(
    cx: &App,
    store: Option<&Arc<dyn SettingsBackend>>,
    key: &str,
    c: &Constraints,
) -> SavedWindow {
    let saved = store
        .and_then(|s| s.get(key))
        .and_then(|text| SavedWindow::decode(&text));
    let displays: Vec<Rect> = cx
        .displays()
        .iter()
        .map(|d| Rect::from_bounds(d.bounds()))
        .collect();
    let primary = cx.primary_display().map(|d| Rect::from_bounds(d.bounds()));
    let placed = place(saved, &displays, primary, c);
    // A fixed window whose record said another size (or maximized) is put right on disk now, so
    // the bad row does not wait for the window to move before it is replaced.
    if let (true, Some(old), Some(store)) = (c.fixed, saved, store) {
        if old.rect.w != placed.rect.w || old.rect.h != placed.rect.h || old.maximized {
            store.set(key, &placed.encode());
        }
    }
    placed
}

/// The monitors' usable areas for snapping: their bounds, with the primary's taskbar taken off
/// where the platform says how big it is.
pub fn usable_displays(cx: &App, scale: f32) -> Vec<Rect> {
    let primary = cx.primary_display().map(|d| Rect::from_bounds(d.bounds()));
    let work = crate::platform::primary_work_area(scale).map(|(x, y, w, h)| Rect::new(x, y, w, h));
    cx.displays()
        .iter()
        .map(|d| Rect::from_bounds(d.bounds()))
        .map(|bounds| match (primary, work) {
            (Some(p), Some(work)) if p == bounds => work,
            _ => bounds,
        })
        .collect()
}

/// Whether a fixed-size window's `seen` rectangle is not its one `size` (beyond rounding).
pub fn size_is_off(seen: &Rect, size: (f32, f32)) -> bool {
    (seen.w - size.0).abs() > FIXED_SIZE_SLACK || (seen.h - size.1).abs() > FIXED_SIZE_SLACK
}

/// What the recorder writes down for a window now at `seen`, or `None` when it should write
/// nothing. A fixed-size window is recorded at its one size, never maximized, and not at all
/// when it is not that size right now.
fn record_of(seen: SavedWindow, fixed: Option<(f32, f32)>) -> Option<SavedWindow> {
    if !seen.rect.is_sane() {
        return None;
    }
    let Some((w, h)) = fixed else {
        return Some(seen);
    };
    (!size_is_off(&seen.rect, (w, h)) && !seen.maximized).then_some(SavedWindow {
        rect: Rect::new(seen.rect.x, seen.rect.y, w, h),
        maximized: false,
    })
}

/// What the recorder has seen and not yet written.
struct Pending {
    latest: Option<SavedWindow>,
    written: Option<SavedWindow>,
    /// A save is already waiting out its delay.
    scheduled: bool,
}

impl Pending {
    /// The record to write, if the latest place differs from the one last written.
    fn take_unwritten(&mut self) -> Option<String> {
        let latest = self.latest.filter(|l| Some(*l) != self.written)?;
        self.written = Some(latest);
        Some(latest.encode())
    }
}

/// Records the window's place under `key` as it changes: a moment after it settles, and on quit.
/// Keep the returned subscription alive for the life of the window. `fixed` is the one size of a
/// window that never resizes or maximizes (the mini player, the HUD): only its position is
/// recorded, always with that size, and a moment where the window reports another size
/// (mid-DPI-change, being torn down) is not recorded at all. `None` records size and maximized.
pub fn remember<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    store: Option<Arc<dyn SettingsBackend>>,
    key: &'static str,
    fixed: Option<(f32, f32)>,
) -> Option<Subscription> {
    let store = store?;
    let pending = Rc::new(RefCell::new(Pending {
        latest: None,
        written: None,
        scheduled: false,
    }));
    // The last place before the app ends, even if the delay has not run out. This one is
    // written on the spot, since nothing runs after it.
    {
        let (store, pending) = (store.clone(), pending.clone());
        cx.on_app_quit(move |_, _| {
            if let Some(text) = pending.borrow_mut().take_unwritten() {
                store.set(key, &text);
            }
            async {}
        })
        .detach();
    }
    Some(cx.observe_window_bounds(window, move |_, window, cx| {
        // A window being minimized, hidden or torn down can report nonsense; never record it.
        let Some(now) = record_of(SavedWindow::of(window), fixed) else {
            return;
        };
        {
            let mut p = pending.borrow_mut();
            p.latest = Some(now);
            if p.scheduled {
                return;
            }
            p.scheduled = true;
        }
        let (pending, store) = (pending.clone(), store.clone());
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let text = {
                let mut p = pending.borrow_mut();
                p.scheduled = false;
                p.take_unwritten()
            };
            if let Some(text) = text {
                cx.background_executor()
                    .spawn(async move { store.set(key, &text) })
                    .await;
            }
        })
        .detach();
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Constraints = Constraints {
        default_size: (1100.0, 720.0),
        min_size: (864.0, 480.0),
        home: Home::Center,
        fixed: false,
    };
    const MINI: Constraints = Constraints {
        default_size: (360.0, 96.0),
        min_size: (360.0, 96.0),
        home: Home::BottomRight,
        fixed: true,
    };
    const PRIMARY: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    /// To the left of the primary, so its x is negative.
    const LEFT: Rect = Rect::new(-1280.0, -100.0, 1280.0, 1024.0);
    /// Above and to the right.
    const RIGHT: Rect = Rect::new(1920.0, -300.0, 2560.0, 1440.0);

    fn saved(x: f32, y: f32, w: f32, h: f32, maximized: bool) -> Option<SavedWindow> {
        Some(SavedWindow {
            rect: Rect::new(x, y, w, h),
            maximized,
        })
    }

    #[test]
    fn a_record_round_trips() {
        let s = SavedWindow {
            rect: Rect::new(-1200.0, 40.0, 1100.0, 720.0),
            maximized: true,
        };
        assert_eq!(s.encode(), "-1200,40,1100,720,1");
        assert_eq!(SavedWindow::decode(&s.encode()), Some(s));
        assert_eq!(
            SavedWindow::decode(" 10, 20, 300, 400, 0 "),
            Some(SavedWindow {
                rect: Rect::new(10.0, 20.0, 300.0, 400.0),
                maximized: false
            })
        );
    }

    #[test]
    fn a_damaged_record_is_not_a_place() {
        for bad in [
            "",
            "1,2,3",
            "1,2,3,4",
            "1,2,3,4,2",
            "1,2,3,4,1,5",
            "a,2,3,4,0",
            "1,2,0,4,0",
            "1,2,3,-4,0",
            "1.5,2,3,4,0",
            "999999999,2,3,4,0",
        ] {
            assert_eq!(SavedWindow::decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nothing_saved_opens_at_the_default() {
        let p = place(None, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(410.0, 180.0, 1100.0, 720.0));
        assert!(!p.maximized);
        // The mini player parks in the bottom-right corner above the taskbar.
        let p = place(None, &[PRIMARY], Some(PRIMARY), &MINI);
        assert_eq!(p.rect, Rect::new(1536.0, 912.0, 360.0, 96.0));
    }

    #[test]
    fn a_place_on_screen_is_kept_exactly() {
        let want = saved(100.0, 80.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(Some(p), want);
    }

    #[test]
    fn maximized_is_kept_with_its_restore_bounds() {
        let want = saved(100.0, 80.0, 1000.0, 700.0, true);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(Some(p), want);
    }

    #[test]
    fn a_window_on_a_second_monitor_stays_there() {
        let want = saved(-1200.0, 20.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY, LEFT], Some(PRIMARY), &MAIN);
        assert_eq!(Some(p), want);
        let want = saved(2000.0, -200.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY, RIGHT], Some(PRIMARY), &MAIN);
        assert_eq!(Some(p), want);
    }

    #[test]
    fn an_unplugged_monitor_sends_the_window_to_the_primary_centre() {
        let want = saved(-1200.0, 20.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(460.0, 190.0, 1000.0, 700.0));
        // A maximized window stays maximized, on the primary.
        let want = saved(2000.0, 20.0, 1000.0, 700.0, true);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(460.0, 190.0, 1000.0, 700.0));
        assert!(p.maximized);
    }

    #[test]
    fn the_mini_player_returns_to_its_corner_when_its_monitor_is_gone() {
        let want = saved(-300.0, 20.0, 360.0, 96.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MINI);
        assert_eq!(p.rect, Rect::new(1536.0, 912.0, 360.0, 96.0));
    }

    #[test]
    fn a_window_hanging_off_an_edge_is_pulled_fully_on() {
        // Off the right and bottom edges, but plenty of its top strip shows.
        let want = saved(1500.0, 900.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(920.0, 380.0, 1000.0, 700.0));
        // Off the left edge.
        let want = saved(-300.0, 0.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(0.0, 0.0, 1000.0, 700.0));
    }

    #[test]
    fn a_window_whose_title_strip_is_off_every_monitor_is_not_reachable() {
        // Only a sliver of the top strip shows on the right: not enough to grab.
        let want = saved(1900.0, 100.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(460.0, 190.0, 1000.0, 700.0));
        // Entirely below the screen.
        let want = saved(100.0, 1200.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(460.0, 190.0, 1000.0, 700.0));
    }

    #[test]
    fn the_mini_player_ignores_a_saved_size_and_keeps_only_its_position() {
        // The row that made it open full screen: a stretched, maximized record.
        let want = saved(0.0, 0.0, 1535.0, 1100.0, true);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MINI);
        assert_eq!(p.rect, Rect::new(0.0, 0.0, 360.0, 96.0));
        assert!(!p.maximized);
        // Its position survives, nudged on screen as ever.
        let want = saved(300.0, 200.0, 900.0, 600.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MINI);
        assert_eq!(p.rect, Rect::new(300.0, 200.0, 360.0, 96.0));
        // Its monitor is gone and the size was huge: home, at the one size.
        let want = saved(-2000.0, 20.0, 1535.0, 1100.0, true);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MINI);
        assert_eq!(p.rect, Rect::new(1536.0, 912.0, 360.0, 96.0));
        assert!(!p.maximized);
    }

    #[test]
    fn a_fixed_window_records_only_its_position_and_only_at_its_size() {
        let at = |w: f32, h: f32, m: bool| SavedWindow {
            rect: Rect::new(40.0, 50.0, w, h),
            maximized: m,
        };
        let one = Some((360.0, 96.0));
        // At its size (within rounding): recorded at exactly its size.
        assert_eq!(
            record_of(at(361.0, 95.0, false), one).map(|s| s.encode()),
            Some("40,50,360,96,0".to_string())
        );
        // Stretched, maximized or nonsense: nothing is written.
        assert_eq!(record_of(at(1535.0, 1100.0, false), one), None);
        assert_eq!(record_of(at(360.0, 96.0, true), one), None);
        assert_eq!(record_of(at(0.0, 0.0, false), one), None);
        // A resizable window records whatever it is.
        assert_eq!(record_of(at(1535.0, 1100.0, true), None), Some(at(1535.0, 1100.0, true)));
    }

    #[test]
    fn a_fixed_window_notices_when_it_is_not_its_size() {
        let size = (360.0, 96.0);
        assert!(!size_is_off(&Rect::new(0.0, 0.0, 360.0, 96.0), size));
        assert!(!size_is_off(&Rect::new(0.0, 0.0, 361.0, 95.0), size));
        assert!(size_is_off(&Rect::new(0.0, 0.0, 1535.0, 1100.0), size));
        assert!(size_is_off(&Rect::new(0.0, 0.0, 360.0, 400.0), size));
    }

    #[test]
    fn a_window_bigger_than_its_monitor_shrinks_to_it() {
        let want = saved(0.0, 0.0, 3000.0, 2000.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, PRIMARY);
        // Moving to a smaller monitor shrinks it there, and a monitor smaller than the
        // window's minimum is not asked for more than it has.
        let small = Rect::new(0.0, 0.0, 800.0, 450.0);
        let want = saved(0.0, 0.0, 1000.0, 700.0, false);
        let p = place(want, &[small], Some(small), &MAIN);
        assert_eq!(p.rect, small);
    }

    #[test]
    fn a_record_smaller_than_the_minimum_is_grown_to_it() {
        let want = saved(100.0, 100.0, 300.0, 200.0, false);
        let p = place(want, &[PRIMARY], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(100.0, 100.0, 864.0, 480.0));
    }

    #[test]
    fn the_monitor_showing_more_of_the_window_wins() {
        // Straddling the seam between the primary and the monitor on its right: most of the
        // strip is on the right one, so the window is fitted to it.
        let want = saved(1800.0, 0.0, 1000.0, 700.0, false);
        let p = place(want, &[PRIMARY, RIGHT], Some(PRIMARY), &MAIN);
        assert_eq!(p.rect, Rect::new(1920.0, 0.0, 1000.0, 700.0));
    }

    #[test]
    fn no_monitors_at_all_leaves_the_record_alone() {
        let want = saved(100.0, 80.0, 1000.0, 700.0, false);
        let p = place(want, &[], None, &MAIN);
        assert_eq!(Some(p), want);
        let p = place(None, &[], None, &MAIN);
        assert_eq!(p.rect, Rect::new(0.0, 0.0, 1100.0, 720.0));
    }

    #[test]
    fn a_window_dropped_near_a_corner_snaps_to_it_with_the_margin() {
        let hud = Rect::new(1500.0, 950.0, 360.0, 96.0);
        // 1920-360-16 = 1544, 1080-96-16 = 968.
        assert_eq!(snap(hud, &[PRIMARY]), Rect::new(1544.0, 968.0, 360.0, 96.0));
        let top_left = Rect::new(30.0, 40.0, 360.0, 96.0);
        assert_eq!(
            snap(top_left, &[PRIMARY]),
            Rect::new(16.0, 16.0, 360.0, 96.0)
        );
    }

    #[test]
    fn a_window_near_one_edge_snaps_on_that_axis_only() {
        let hud = Rect::new(800.0, 20.0, 360.0, 96.0);
        assert_eq!(snap(hud, &[PRIMARY]), Rect::new(800.0, 16.0, 360.0, 96.0));
        let hud = Rect::new(1560.0, 500.0, 360.0, 96.0);
        assert_eq!(snap(hud, &[PRIMARY]), Rect::new(1544.0, 500.0, 360.0, 96.0));
    }

    #[test]
    fn a_window_far_from_every_edge_stays_where_it_was_dropped() {
        let hud = Rect::new(700.0, 400.0, 360.0, 96.0);
        assert_eq!(snap(hud, &[PRIMARY]), hud);
        // Off every monitor: left alone.
        let lost = Rect::new(5000.0, 5000.0, 360.0, 96.0);
        assert_eq!(snap(lost, &[PRIMARY]), lost);
    }

    #[test]
    fn snapping_uses_the_monitor_the_window_is_mostly_on() {
        // On the second monitor, whose left edge is at -1280.
        let hud = Rect::new(-1270.0, 600.0, 360.0, 96.0);
        assert_eq!(
            snap(hud, &[PRIMARY, LEFT]),
            Rect::new(-1264.0, 600.0, 360.0, 96.0)
        );
    }

    #[test]
    fn snapping_twice_changes_nothing() {
        let once = snap(Rect::new(1500.0, 950.0, 360.0, 96.0), &[PRIMARY]);
        assert_eq!(snap(once, &[PRIMARY]), once);
    }
}
