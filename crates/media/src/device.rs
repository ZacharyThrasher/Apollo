//! Output devices as mpv lists them.
//!
//! mpv's `audio-device-list` names every device of every audio output API it was built with
//! (`auto`, `wasapi/{id}`, `sdl/...`, `openal/...`). Apollo plays through WASAPI, so only those
//! are offered, and `auto` is the app's own "System default".

use std::time::Duration;

use apollo_domain::audio::{AudioDevice, SYSTEM_DEFAULT};

/// Devices of this output API are the ones on offer (every API where there is no single one).
pub const OUTPUT_PREFIX: &str = if cfg!(windows) { "wasapi/" } else { "" };

/// How often the list is read, so an unplugged or returning device is noticed without anyone
/// asking. A failing output also asks for an early read.
pub const DEVICE_POLL: Duration = Duration::from_secs(2);

/// The devices to offer from mpv's `(name, description)` pairs, in mpv's order. Drops `auto`,
/// other output APIs and duplicates; a device with no description is shown by its name.
pub fn from_entries(entries: impl IntoIterator<Item = (String, String)>) -> Vec<AudioDevice> {
    let mut out: Vec<AudioDevice> = Vec::new();
    for (id, description) in entries {
        if id == SYSTEM_DEFAULT || id.is_empty() || !id.starts_with(OUTPUT_PREFIX) {
            continue;
        }
        if out.iter().any(|d| d.id == id) {
            continue;
        }
        let description = description.trim();
        let name = if description.is_empty() {
            id.clone()
        } else {
            description.to_owned()
        };
        out.push(AudioDevice { id, name });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, d: &str) -> (String, String) {
        (id.to_owned(), d.to_owned())
    }

    #[test]
    fn auto_is_the_apps_system_default_not_a_device() {
        let got = from_entries([e("auto", "Autoselect device"), e("wasapi/{1}", "Speakers")]);
        assert!(got.iter().all(|d| d.id != "auto"));
        assert!(got.iter().any(|d| d.id == "wasapi/{1}"));
    }

    #[test]
    fn a_device_is_shown_by_its_description_or_else_its_name() {
        let got = from_entries([
            e("wasapi/{1}", "  Speakers (Realtek)  "),
            e("wasapi/{2}", " "),
        ]);
        assert_eq!(got[0].name, "Speakers (Realtek)");
        assert_eq!(got[1].name, "wasapi/{2}");
    }

    #[test]
    fn duplicates_and_nameless_entries_are_dropped_and_order_is_kept() {
        let got = from_entries([
            e("wasapi/{b}", "B"),
            e("", "nothing"),
            e("wasapi/{a}", "A"),
            e("wasapi/{b}", "B again"),
        ]);
        let ids: Vec<_> = got.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["wasapi/{b}", "wasapi/{a}"]);
        assert_eq!(got[0].name, "B");
    }

    #[cfg(windows)]
    #[test]
    fn only_wasapi_devices_are_offered_on_windows() {
        let got = from_entries([
            e("sdl/x", "SDL"),
            e("openal/y", "OpenAL"),
            e("wasapi/{1}", "S"),
        ]);
        assert_eq!(got.len(), 1);
    }
}
