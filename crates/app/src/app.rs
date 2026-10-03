//! Application bootstrap: window, shortcuts, and the bridge from the service adapters to the UI.

use gpui::{
    px, size, App, AppContext, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowOptions,
};

use crate::backend::{channel_sink, Mode};
use crate::player::PlayerModel;
use crate::ui::shell::{self, Quit, Shell};

/// Smallest window that still fits the player bar's three columns.
const MIN_WIDTH: f32 = 720.0;
const MIN_HEIGHT: f32 = 480.0;

pub fn run(cx: &mut App, mode: Mode) -> WindowHandle<Shell> {
    // Windows "Animation effects" off means every transition is instant.
    cx.set_reduce_motion(!crate::platform::animations_enabled());
    crate::style::Fonts::install(cx);
    shell::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());

    let player = cx.new(|_| PlayerModel::new());
    let window = open_window(cx, player.clone());
    start_services(cx, mode, player.clone());

    cx.on_app_quit(move |cx| {
        player.update(cx, |m, _| m.shutdown());
        async {}
    })
    .detach();
    cx.activate(true);
    window
}

fn open_window(cx: &mut App, player: gpui::Entity<PlayerModel>) -> WindowHandle<Shell> {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Apollo".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Shell::new(player, cx)),
        )
        .expect("open window");
    window
        .update(cx, |shell, window, cx| shell.focus_root(window, cx))
        .expect("focus root");
    window
}

/// Prepares the backend off the UI thread, installs it, then feeds adapter completions to the
/// model until the window goes away.
fn start_services(cx: &mut App, mode: Mode, player: gpui::Entity<PlayerModel>) {
    let (sink, mut rx) = channel_sink();
    let weak = player.downgrade();
    cx.spawn(async move |cx| {
        let prepared = cx
            .background_executor()
            .spawn(async move { mode.prepare(sink) })
            .await;
        match prepared {
            Ok(prepared) => {
                let _ = weak.update(cx, |m, cx| m.install(prepared, cx));
            }
            Err(err) => {
                let _ = weak.update(cx, |m, cx| m.fail(&err, cx));
                return;
            }
        }
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            let alive = weak.update(cx, |m, cx| {
                for event in batch {
                    m.handle(event, cx);
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}
