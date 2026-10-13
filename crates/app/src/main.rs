#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use apollo_app::assets::Assets;
use apollo_app::backend::Mode;
use gpui_platform::application;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The update swap runs in a copy of this exe started by the app; no window, no app.
    if args.first().map(String::as_str) == Some(apollo_updater::helper::HELPER_FLAG) {
        std::process::exit(apollo_updater::helper::run_helper(&args));
    }
    let mode = match Mode::from_args(&args) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    // One Apollo at a time: a second launch shows the first one's window and leaves. The
    // simulated backends are for development and screenshots, and may run beside the real app.
    #[cfg(windows)]
    if !matches!(mode, Mode::Sim | Mode::SimSignedOut) {
        if !apollo_app::single_instance::acquire() {
            return;
        }
        // An update fetched earlier and not yet swapped in: hand over and exit.
        if apollo_app::update::startup() {
            return;
        }
    }
    application().with_assets(Assets).run(move |cx| {
        apollo_app::app::run(cx, mode);
    });
}
