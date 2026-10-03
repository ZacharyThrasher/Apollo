//! Embedded Fluent System Icons (regular and filled, 20px; MIT, see
//! `assets/FLUENT-ICONS-LICENSE.txt`) plus the spinner. Served to GPUI's `svg()` by path.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($variant),+ }

        impl Icon {
            pub const ALL: &'static [Icon] = &[$(Icon::$variant),+];

            pub fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file, ".svg")),+ }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!("../assets/icons/", $file, ".svg"))),+
                }
            }
        }
    };
}

icons! {
    HomeRegular => "home-regular",
    HomeFilled => "home-filled",
    SearchRegular => "search-regular",
    SearchFilled => "search-filled",
    LibraryRegular => "library-regular",
    LibraryFilled => "library-filled",
    SettingsRegular => "settings-regular",
    SettingsFilled => "settings-filled",
    Navigation => "navigation-regular",
    Previous => "previous-regular",
    Next => "next-regular",
    Play => "play-filled",
    Pause => "pause-filled",
    ShuffleRegular => "shuffle-regular",
    ShuffleFilled => "shuffle-filled",
    RepeatAllRegular => "repeat-all-regular",
    RepeatAllFilled => "repeat-all-filled",
    RepeatOneRegular => "repeat-one-regular",
    RepeatOneFilled => "repeat-one-filled",
    VolumeHigh => "volume-high-regular",
    VolumeLow => "volume-low-regular",
    VolumeMute => "volume-mute-regular",
    QueueRegular => "queue-regular",
    QueueFilled => "queue-filled",
    MusicNote => "music-note-regular",
    Warning => "warning-regular",
    Dismiss => "dismiss-regular",
    Delete => "delete-regular",
    ChevronDown => "chevron-down-regular",
    Spinner => "spinner",
    Add => "add-regular",
    ArrowLeft => "arrow-left-regular",
    HeartFilled => "heart-filled",
    HeartRegular => "heart-regular",
    PlaylistAdd => "playlist-add-regular",
    Checkmark => "checkmark-regular",
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .iter()
            .find(|i| i.path() == path)
            .map(|i| Cow::Borrowed(i.bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .filter(|i| i.path().starts_with(path))
            .map(|i| SharedString::from(i.path()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded_and_is_an_svg() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().expect("embedded");
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("<svg"),
                "{icon:?}"
            );
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
