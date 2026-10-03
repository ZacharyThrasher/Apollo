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
