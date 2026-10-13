//! Self-update, the app's side. A build is published to GitHub Releases on every change to
//! `main`; here Apollo notices a newer one and gets it ready, and the swap happens only while
//! Apollo is not running: at quit, or at the next start if the quit never got to it. Playback is
//! never touched. Settings > Window > Install updates automatically turns all of it off.
//!
//! Only the packaged install (`Apollo.exe` beside `libmpv-2.dll` and `tools\`) updates itself; a
//! build run from `target\` never replaces itself. The mechanics are in the `apollo-updater` crate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_updater::helper::{install_dir_for, log, spawn_helper, updates_dir};
use apollo_updater::{check_and_stage, discard_staged, staged, Version};
use gpui::{App, Entity};

use crate::settings::{stored_bool, SettingsModel, INSTALL_UPDATES};

/// Whether this platform can update itself; the Settings row is hidden without it.
pub const AVAILABLE: bool = cfg!(windows);

/// The version of this build: stamped in by the release workflow, `0.0.0` for a local build
/// (which any release is newer than).
pub const VERSION: &str = match option_env!("APOLLO_VERSION") {
    Some(version) => version,
    None => "0.0.0",
};

/// First look for an update, after the app has settled.
const FIRST_CHECK: Duration = Duration::from_secs(45);
/// Then every few hours.
const EVERY: Duration = Duration::from_secs(4 * 60 * 60);
/// How often the checker wakes to see whether it is due (and whether the switch is on).
const TICK: Duration = Duration::from_secs(20);

fn installed() -> Version {
    Version::parse(VERSION).unwrap_or_else(|| Version::parse("0.0.0").expect("literal"))
}

/// Called first thing after this process becomes the running Apollo. If an update was fetched
/// and the quit did not install it, hands over to the swap helper and returns `true`: the caller
/// exits, and the helper starts the new build once this process is gone.
pub fn startup() -> bool {
    if !AVAILABLE {
        return false;
    }
    let (Ok(exe), Some(updates)) = (std::env::current_exe(), updates_dir()) else {
        return false;
    };
    let Some(install) = install_dir_for(&exe) else {
        return false;
    };
    // Leftovers of the last swap (a running file could not be deleted then).
    apollo_updater::apply::cleanup_old(&install);
    let Some(build) = staged(&updates) else {
        return false;
    };
    if build.version <= installed() {
        discard_staged(&updates);
        return false;
    }
    if !stored_bool(&INSTALL_UPDATES) {
        return false;
    }
    match spawn_helper(&exe, true) {
        Ok(()) => true,
        Err(err) => {
            log(&updates, &format!("could not start the updater: {err}"));
            false
        }
    }
}

/// Looks for updates in the background while the switch is on, and installs a fetched one at
/// quit.
pub fn install(cx: &mut App, settings: Entity<SettingsModel>) {
    if !AVAILABLE {
        return;
    }
    let (Ok(exe), Some(updates)) = (std::env::current_exe(), updates_dir()) else {
        return;
    };
    if install_dir_for(&exe).is_none() {
        return;
    }
    let enabled = Arc::new(AtomicBool::new(settings.read(cx).get(&INSTALL_UPDATES)));
    {
        let enabled = enabled.clone();
        cx.observe(&settings, move |settings, cx| {
            enabled.store(settings.read(cx).get(&INSTALL_UPDATES), Ordering::Relaxed);
        })
        .detach();
    }
    {
        let (enabled, updates) = (enabled.clone(), updates.clone());
        let thread_updates = updates.clone();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let mut due = Instant::now() + FIRST_CHECK;
                loop {
                    std::thread::sleep(TICK);
                    if !enabled.load(Ordering::Relaxed) {
                        // Switched on again later: look soon, not hours from now.
                        due = Instant::now() + FIRST_CHECK;
                        continue;
                    }
                    if Instant::now() < due {
                        continue;
                    }
                    due = Instant::now() + EVERY;
                    match check_and_stage(&installed(), &thread_updates) {
                        Ok(Some(version)) => log(&thread_updates, &format!("{version} is ready")),
                        Ok(None) => {}
                        Err(err) => log(&thread_updates, &err),
                    }
                }
            });
        if let Err(err) = spawned {
            log(&updates, &format!("update checker did not start: {err}"));
        }
    }
    cx.on_app_quit(move |_| {
        if enabled.load(Ordering::Relaxed)
            && staged(&updates).is_some_and(|build| build.version > installed())
        {
            if let Err(err) = spawn_helper(&exe, false) {
                log(&updates, &format!("could not start the updater: {err}"));
            }
        }
        async {}
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_are_on_by_default_and_show_in_the_report() {
        assert!(INSTALL_UPDATES.default);
        assert!(crate::settings::REPORTED_KEYS.contains(&INSTALL_UPDATES.key));
    }

    #[test]
    fn the_build_has_a_version() {
        assert!(Version::parse(VERSION).is_some(), "{VERSION}");
    }
}
