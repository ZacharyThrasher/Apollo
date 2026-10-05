//! Apollo desktop app: GPUI window, custom title bar, nav rail and player bar.

/// Shadows `std::eprintln!` for the whole crate: the line is printed as before and also kept in
/// the diagnostics report's short log (`diagnostics::log`), because a packaged Windows build has
/// no console and would otherwise lose it. Defined before the modules so every one of them sees
/// it; a module that wants the plain macro writes `std::eprintln!`.
macro_rules! eprintln {
    () => {
        $crate::diagnostics::log::print(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::diagnostics::log::print(format_args!($($arg)*))
    };
}

pub mod accent_anim;
pub mod account;
pub mod app;
pub mod art;
pub mod crate_list;
pub mod assets;
pub mod audio;
pub mod blocked_artists;
pub mod autostart;
pub mod backend;
pub mod explore;
pub mod export;
pub mod diagnostics;
pub mod discord;
pub mod home;
pub mod hud;
#[cfg(windows)]
pub mod hotkeys_win;
pub mod keymap;
pub mod latency;
pub mod library;
pub mod lyrics;
pub mod related;
pub mod loudness;
pub mod mini;
pub mod more_like;
pub mod motion;
pub mod os_media;
pub mod page_cache;
pub mod pc_watch;
#[cfg(windows)]
pub mod pc_watch_win;
pub mod persist;
pub mod platform;
pub mod player;
pub mod reporting;
pub mod queue_source;
pub mod search;
pub mod settings;
pub mod shortcuts;
pub mod stats;
pub mod sleep_timer;
#[cfg(windows)]
pub mod single_instance;
#[cfg(windows)]
pub mod smtc;
pub mod style;
pub mod thumbs;
pub mod tray;
#[cfg(windows)]
pub mod tray_win;
pub mod ui;
pub mod url_thumbs;
pub mod window_state;
