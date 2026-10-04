//! Small Windows queries GPUI does not make for us.

/// Windows "Animation effects" (Settings > Accessibility > Visual effects). When off, every
/// transition is instant (DIRECTION.md, Motion). Other platforms report true.
#[cfg(windows)]
pub fn animations_enabled() -> bool {
    use std::ffi::c_void;

    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            pv_param: *mut c_void,
            win_ini: u32,
        ) -> i32;
    }

    let mut enabled: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        )
    };
    ok == 0 || enabled != 0
}

#[cfg(not(windows))]
pub fn animations_enabled() -> bool {
    true
}

// ---- window-level helpers -------------------------------------------------------------------

use gpui::Window;

/// Takes a window off the screen and the taskbar; playback and the app carry on. A no-op off
/// Windows, where GPUI has no way to hide a window (the app only ships on Windows).
pub fn hide_window(window: &mut Window) {
    #[cfg(windows)]
    crate::tray_win::hide(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Brings a window back (restoring it if minimized) and to the front.
pub fn show_window(window: &mut Window) {
    #[cfg(windows)]
    crate::tray_win::show(window);
    #[cfg(not(windows))]
    window.activate_window();
}

/// Floats a window above all others, or puts it back among them. No effect off Windows.
pub fn set_topmost(window: &Window, on: bool) {
    #[cfg(windows)]
    win::set_topmost(window, on);
    #[cfg(not(windows))]
    let _ = (window, on);
}

/// Asks Windows 11 for the standard 8px rounded corners on a window that has no frame of its
/// own to get them from. No effect off Windows or on builds before Windows 11.
pub fn round_corners(window: &Window) {
    #[cfg(windows)]
    win::round_corners(window);
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use gpui::Window;

    use crate::smtc::hwnd_of;

    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: *mut c_void,
            insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    pub fn set_topmost(window: &Window, on: bool) {
        let Some(hwnd) = hwnd_of(window) else { return };
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: restacks a window handle GPUI owns without moving, resizing or activating it.
        unsafe {
            SetWindowPos(
                hwnd,
                after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }

    pub fn round_corners(window: &Window) {
        let Some(hwnd) = hwnd_of(window) else { return };
        let preference = DWMWCP_ROUND;
        // SAFETY: DWMWA_WINDOW_CORNER_PREFERENCE reads one i32 through the pointer; an
        // unsupported attribute (before Windows 11) just returns an error code.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&preference as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            )
        };
    }
}
