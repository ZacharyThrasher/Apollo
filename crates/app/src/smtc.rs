//! Windows media keys and the System Media Transport Controls overlay, through Souvlaki on the
//! HWND GPUI already owns (ADR 0001; proven in the M0 spike): no second event loop.
//!
//! Souvlaki calls the handler on a WinRT thread, so it only forwards [`OsCommand`]s down a
//! channel; a GPUI task applies them on the UI thread. The SMTC objects themselves live on the
//! UI thread and are only touched from [`MediaBridge`] updates there.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Entity, Window, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::os_media::{MediaBridge, OsCommand, OsMediaSink, OsMetadata, OsPlayback};
use crate::player::PlayerModel;
use crate::ui::shell::Shell;

/// What the overlay's fast-forward and rewind buttons skip.
const SEEK_STEP: Duration = Duration::from_secs(10);
/// Cover files kept in the art folder, newest first; older ones are deleted.
const KEPT_COVERS: usize = 16;
/// Name of the stand-in used when a track has no artwork, so the previous track's picture is
/// replaced rather than left on the overlay (Souvlaki cannot clear a thumbnail).
const BLANK_COVER: &str = "none.png";

/// The Souvlaki end of the bridge.
pub struct SmtcSink {
    controls: MediaControls,
    art_dir: Option<PathBuf>,
}

impl SmtcSink {
    /// The cover as a `file://` URL Souvlaki loads itself. The overlay needs a file or an https
    /// URL, and the artwork has already been downloaded and decoded, so it is written out once
    /// per track.
    fn cover_url(&self, metadata: &OsMetadata<'_>) -> Option<String> {
        let dir = self.art_dir.as_deref()?;
        std::fs::create_dir_all(dir).ok()?;
        let name = match metadata.cover {
            Some(_) => format!("{}.png", sanitize(metadata.video_id)),
            None => BLANK_COVER.to_owned(),
        };
        let path = dir.join(&name);
        let bytes: Vec<u8> = match metadata.cover {
            Some(png) => png.to_vec(),
            None => blank_png(),
        };
        let unchanged = std::fs::read(&path).is_ok_and(|on_disk| on_disk == bytes);
        if !unchanged {
            std::fs::write(&path, &bytes).ok()?;
        }
        prune(dir, &name);
        Some(format!("file://{}", path.display()))
    }
}

impl OsMediaSink for SmtcSink {
    fn set_metadata(&mut self, metadata: &OsMetadata<'_>) {
        let cover_url = self.cover_url(metadata);
        // Souvlaki only writes the fields it is given, so a missing album is an empty string:
        // `None` would leave the previous track's album on the overlay.
        let result = self.controls.set_metadata(MediaMetadata {
            title: Some(metadata.title),
            artist: Some(metadata.artist),
            album: Some(metadata.album),
            cover_url: cover_url.as_deref(),
            duration: metadata.duration,
        });
        if let Err(err) = result {
            eprintln!("media overlay: could not set metadata: {err}");
        }
    }

    fn set_playback(&mut self, playback: OsPlayback) {
        let playback = match playback {
            OsPlayback::Stopped => MediaPlayback::Stopped,
            OsPlayback::Paused { position } => MediaPlayback::Paused {
                progress: Some(MediaPosition(position)),
            },
            OsPlayback::Playing { position } => MediaPlayback::Playing {
                progress: Some(MediaPosition(position)),
            },
        };
        if let Err(err) = self.controls.set_playback(playback) {
            eprintln!("media overlay: could not set playback: {err}");
        }
    }
}

/// Hooks the media keys and overlay to `player` for the life of the app. Returns `false` (and
/// leaves things as they were) if the window has no Win32 handle or SMTC is unavailable.
pub fn install(cx: &mut App, window: WindowHandle<Shell>, player: Entity<PlayerModel>) -> bool {
    let hwnd = window
        .update(cx, |_, window, _| hwnd_of(window))
        .ok()
        .flatten();
    let Some(hwnd) = hwnd else {
        eprintln!("media overlay: window has no Win32 handle");
        return false;
    };
    let (tx, mut rx) = unbounded_channel::<OsCommand>();
    let Some(sink) = connect(hwnd, art_dir(), tx) else {
        return false;
    };
    let host = cx.new(|cx| {
        cx.observe(&player, |host: &mut Host, player, cx| {
            let state = player.read(cx).media_state();
            host.bridge.apply(&state, Instant::now());
        })
        .detach();
        Host {
            bridge: MediaBridge::new(sink),
        }
    });
    let state = player.read(cx).media_state();
    host.update(cx, |host, _| host.bridge.apply(&state, Instant::now()));

    let weak = player.downgrade();
    cx.spawn(async move |cx| {
        while let Some(command) = rx.recv().await {
            if weak.update(cx, |m, cx| m.os_command(command, cx)).is_err() {
                break;
            }
        }
    })
    .detach();

    // The overlay must not outlive the window: tell the OS playback has stopped.
    cx.on_app_quit(move |cx| {
        host.update(cx, |host, _| host.bridge.stop());
        async {}
    })
    .detach();
    true
}

struct Host {
    bridge: MediaBridge<SmtcSink>,
}

fn connect(
    hwnd: *mut c_void,
    art_dir: Option<PathBuf>,
    tx: UnboundedSender<OsCommand>,
) -> Option<SmtcSink> {
    let mut controls = MediaControls::new(PlatformConfig {
        dbus_name: "apollo",
        display_name: "Apollo",
        hwnd: Some(hwnd),
    })
    .map_err(|err| eprintln!("media overlay: could not create SMTC controls: {err}"))
    .ok()?;
    controls
        .attach(move |event| {
            if let Some(command) = command_for(event) {
                let _ = tx.send(command);
            }
        })
        .map_err(|err| eprintln!("media overlay: could not attach: {err}"))
        .ok()?;
    Some(SmtcSink { controls, art_dir })
}

fn command_for(event: MediaControlEvent) -> Option<OsCommand> {
    let signed = |direction: SeekDirection, by: Duration| {
        let ms = by.as_millis() as i64;
        match direction {
            SeekDirection::Forward => ms,
            SeekDirection::Backward => -ms,
        }
    };
    Some(match event {
        MediaControlEvent::Play => OsCommand::Play,
        MediaControlEvent::Pause => OsCommand::Pause,
        MediaControlEvent::Toggle => OsCommand::Toggle,
        MediaControlEvent::Next => OsCommand::Next,
        MediaControlEvent::Previous => OsCommand::Previous,
        MediaControlEvent::Stop => OsCommand::Stop,
        MediaControlEvent::Seek(direction) => OsCommand::SeekBy(signed(direction, SEEK_STEP)),
        MediaControlEvent::SeekBy(direction, by) => OsCommand::SeekBy(signed(direction, by)),
        MediaControlEvent::SetPosition(MediaPosition(at)) => {
            OsCommand::SeekTo(at.as_millis() as u64)
        }
        // Volume belongs to the player; Open and Raise have no meaning for the overlay on Windows.
        MediaControlEvent::SetVolume(_)
        | MediaControlEvent::OpenUri(_)
        | MediaControlEvent::Raise
        | MediaControlEvent::Quit => return None,
    })
}

pub(crate) fn hwnd_of(window: &Window) -> Option<*mut c_void> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
        _ => None,
    }
}

/// `%LOCALAPPDATA%\Apollo\app\media-art`, beside the settings database.
fn art_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("Apollo").join("app").join("media-art"))
}

/// A video id as a file name: ids are URL-safe already, but this never trusts that.
fn sanitize(id: &str) -> String {
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if clean.is_empty() {
        "track".to_owned()
    } else {
        clean
    }
}

/// A transparent 1x1 PNG.
fn blank_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode a 1x1 PNG");
    out.into_inner()
}

/// Keeps the newest [`KEPT_COVERS`] files, and always `keep`.
fn prune(dir: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != keep && e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(KEPT_COVERS - 1) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_overlay_events_become_commands() {
        use MediaControlEvent as E;
        assert_eq!(command_for(E::Play), Some(OsCommand::Play));
        assert_eq!(command_for(E::Pause), Some(OsCommand::Pause));
        assert_eq!(command_for(E::Toggle), Some(OsCommand::Toggle));
        assert_eq!(command_for(E::Next), Some(OsCommand::Next));
        assert_eq!(command_for(E::Previous), Some(OsCommand::Previous));
        assert_eq!(command_for(E::Stop), Some(OsCommand::Stop));
        assert_eq!(
            command_for(E::Seek(SeekDirection::Backward)),
            Some(OsCommand::SeekBy(-10_000))
        );
        assert_eq!(
            command_for(E::SetPosition(MediaPosition(Duration::from_millis(61_500)))),
            Some(OsCommand::SeekTo(61_500))
        );
        assert_eq!(command_for(E::SetVolume(0.5)), None);
        assert_eq!(command_for(E::Raise), None);
    }

    #[test]
    fn file_names_cannot_escape_the_art_folder() {
        assert_eq!(sanitize("dQw4w9WgXcQ"), "dQw4w9WgXcQ");
        assert_eq!(sanitize("../../evil"), "______evil");
        assert_eq!(sanitize(""), "track");
    }

    #[test]
    fn pruning_keeps_the_newest_covers_and_the_one_in_use() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(KEPT_COVERS + 5) {
            let path = dir.path().join(format!("c{i}.png"));
            std::fs::write(&path, b"x").unwrap();
            let t = std::time::SystemTime::now() - Duration::from_secs((1000 - i) as u64);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        std::fs::write(dir.path().join(BLANK_COVER), b"x").unwrap();
        prune(dir.path(), BLANK_COVER);
        let left = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(left, KEPT_COVERS);
        assert!(dir.path().join(BLANK_COVER).exists());
        assert!(dir.path().join(format!("c{}.png", KEPT_COVERS + 4)).exists());
        assert!(!dir.path().join("c0.png").exists());
    }
}
