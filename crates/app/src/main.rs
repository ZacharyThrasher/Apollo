#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use apollo_app::assets::Assets;
use apollo_app::backend::Mode;
use gpui_platform::application;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match Mode::from_args(&args) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    application().with_assets(Assets).run(move |cx| {
        apollo_app::app::run(cx, mode);
    });
}
