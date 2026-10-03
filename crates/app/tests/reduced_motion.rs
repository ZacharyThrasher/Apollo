//! The reduced-motion flag follows the system's animation setting while the app runs.

use std::cell::Cell;
use std::rc::Rc;

use apollo_app::app::{follow_reduced_motion, MOTION_POLL};
use gpui::TestAppContext;

#[gpui::test]
fn reduced_motion_follows_the_system_setting_without_a_restart(cx: &mut TestAppContext) {
    let animations = Rc::new(Cell::new(true));
    cx.update(|cx| {
        let animations = animations.clone();
        follow_reduced_motion(cx, move || animations.get());
    });
    assert!(!cx.update(|cx| cx.reduce_motion()), "animations on at start");

    // The user switches "Animation effects" off in Windows settings.
    animations.set(false);
    cx.executor().advance_clock(MOTION_POLL);
    cx.run_until_parked();
    assert!(cx.update(|cx| cx.reduce_motion()), "picked up at the next poll");

    // And back on.
    animations.set(true);
    cx.executor().advance_clock(MOTION_POLL);
    cx.run_until_parked();
    assert!(!cx.update(|cx| cx.reduce_motion()));
}

#[gpui::test]
fn the_setting_is_read_at_start_too(cx: &mut TestAppContext) {
    cx.update(|cx| follow_reduced_motion(cx, || false));
    assert!(cx.update(|cx| cx.reduce_motion()), "animations off at start");
}
