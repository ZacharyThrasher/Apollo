//! Apollo desktop app: GPUI window, custom title bar, nav rail and player bar.

pub mod accent_anim;
pub mod account;
pub mod app;
pub mod art;
pub mod assets;
pub mod autostart;
pub mod backend;
pub mod home;
pub mod latency;
pub mod library;
pub mod lyrics;
pub mod related;
pub mod loudness;
pub mod motion;
pub mod os_media;
pub mod persist;
pub mod platform;
pub mod player;
pub mod search;
pub mod settings;
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
