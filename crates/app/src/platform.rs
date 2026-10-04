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

/// Turns off the thin frame Windows 11 draws round a window (the HUD wants a clean edge). No
/// effect off Windows or on builds before Windows 11.
pub fn hide_border(window: &Window) {
    #[cfg(windows)]
    win::hide_border(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Shows a window without taking the keyboard from whatever has it (the HUD coming back).
pub fn show_window_inactive(window: &mut Window) {
    #[cfg(windows)]
    win::show_inactive(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Makes a window one that never becomes the active window, so clicking it cannot take the
/// keyboard from a game. No effect off Windows.
pub fn set_no_activate(window: &Window) {
    #[cfg(windows)]
    win::set_no_activate(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Lets mouse clicks fall through a window to whatever is under it (on), or catches them (off).
/// No effect off Windows.
pub fn set_click_through(window: &Window, on: bool) {
    #[cfg(windows)]
    win::set_click_through(window, on);
    #[cfg(not(windows))]
    let _ = (window, on);
}

/// Whether the mouse pointer is over the window's rectangle less `inset` logical pixels on
/// every side, wherever the clicks go. Always false off Windows.
pub fn pointer_over(window: &Window, inset: f32) -> bool {
    #[cfg(windows)]
    return win::pointer_over(window, inset);
    #[cfg(not(windows))]
    {
        let _ = (window, inset);
        false
    }
}

/// Moves a window's top-left corner to `x`, `y` (logical pixels, screen coordinates).
pub fn move_window(window: &Window, x: f32, y: f32) {
    #[cfg(windows)]
    win::move_window(window, x, y);
    #[cfg(not(windows))]
    let _ = (window, x, y);
}

/// The primary monitor's usable area (without the taskbar) as x, y, width, height in logical
/// pixels at the given scale. `None` off Windows.
pub fn primary_work_area(scale: f32) -> Option<(f32, f32, f32, f32)> {
    #[cfg(windows)]
    return win::primary_work_area(scale);
    #[cfg(not(windows))]
    {
        let _ = scale;
        None
    }
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
    const SWP_NOZORDER: u32 = 0x0004;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const LWA_ALPHA: u32 = 0x2;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const SPI_GETWORKAREA: u32 = 0x0030;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;
    const DWMWA_BORDER_COLOR: u32 = 34;
    /// "No border" for DWMWA_BORDER_COLOR.
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

    #[repr(C)]
    #[derive(Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, value: isize) -> isize;
        fn SetLayeredWindowAttributes(hwnd: *mut c_void, key: u32, alpha: u8, flags: u32) -> i32;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            pv_param: *mut c_void,
            win_ini: u32,
        ) -> i32;
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

    pub fn hide_border(window: &Window) {
        let Some(hwnd) = hwnd_of(window) else { return };
        let color = DWMWA_COLOR_NONE;
        // SAFETY: DWMWA_BORDER_COLOR reads one u32 through the pointer; an unsupported
        // attribute (before Windows 11) just returns an error code.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
    }

    fn ex_style(hwnd: *mut c_void) -> isize {
        // SAFETY: reads a window long of a handle GPUI owns.
        unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
    }

    fn set_ex_style(hwnd: *mut c_void, style: isize) {
        // SAFETY: writes the extended style of a handle GPUI owns.
        unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style) };
    }

    pub fn show_inactive(window: &Window) {
        let Some(hwnd) = hwnd_of(window) else { return };
        // SAFETY: shows a window handle GPUI owns without activating it.
        unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }

    pub fn set_no_activate(window: &Window) {
        let Some(hwnd) = hwnd_of(window) else { return };
        set_ex_style(hwnd, ex_style(hwnd) | WS_EX_NOACTIVATE);
    }

    pub fn set_click_through(window: &Window, on: bool) {
        let Some(hwnd) = hwnd_of(window) else { return };
        let style = ex_style(hwnd);
        if on {
            // A layered window with the transparent style is what lets clicks pass to the
            // window below; the alpha call makes the layered style take effect without
            // changing how the window looks.
            set_ex_style(hwnd, style | WS_EX_LAYERED | WS_EX_TRANSPARENT);
            // SAFETY: full-opacity layered attributes on a handle GPUI owns.
            unsafe { SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) };
        } else {
            set_ex_style(hwnd, style & !WS_EX_TRANSPARENT);
        }
    }

    pub fn pointer_over(window: &Window, inset: f32) -> bool {
        let Some(hwnd) = hwnd_of(window) else {
            return false;
        };
        let (mut point, mut rect) = (Point::default(), Rect::default());
        // SAFETY: both calls fill the structs they are given.
        let ok = unsafe { GetCursorPos(&mut point) != 0 && GetWindowRect(hwnd, &mut rect) != 0 };
        let inset = (inset * window.scale_factor()).round() as i32;
        ok && point.x >= rect.left + inset
            && point.x < rect.right - inset
            && point.y >= rect.top + inset
            && point.y < rect.bottom - inset
    }

    pub fn move_window(window: &Window, x: f32, y: f32) {
        let Some(hwnd) = hwnd_of(window) else { return };
        let scale = window.scale_factor();
        // SAFETY: moves a window handle GPUI owns without resizing, restacking or activating.
        unsafe {
            SetWindowPos(
                hwnd,
                0,
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub fn primary_work_area(scale: f32) -> Option<(f32, f32, f32, f32)> {
        let mut rect = Rect::default();
        // SAFETY: SPI_GETWORKAREA writes one RECT through the pointer.
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut rect as *mut Rect).cast(), 0)
        };
        (ok != 0 && scale > 0.0).then(|| {
            (
                rect.left as f32 / scale,
                rect.top as f32 / scale,
                (rect.right - rect.left) as f32 / scale,
                (rect.bottom - rect.top) as f32 / scale,
            )
        })
    }
}
