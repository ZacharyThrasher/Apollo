//! Apollo desktop app: GPUI window, custom title bar, nav rail and player bar.

pub mod accent_anim;
pub mod account;
pub mod app;
pub mod art;
pub mod assets;
pub mod audio;
pub mod autostart;
pub mod backend;
pub mod explore;
pub mod discord;
pub mod home;
#[cfg(windows)]
pub mod hotkeys_win;
pub mod keymap;
pub mod latency;
pub mod library;
pub mod lyrics;
pub mod related;
pub mod loudness;
pub mod mini;
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
pub mod search;
pub mod settings;
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
