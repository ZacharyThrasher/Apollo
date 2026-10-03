//! Launch at sign-in: a value in the current user's `Run` key, so Windows starts Apollo when the
//! owner signs in. The registry is the source of truth (there is no copy in the settings store),
//! so removing the entry by hand in Task Manager's Startup tab is reflected in Settings.

/// Whether this platform has a sign-in launch to offer; the Settings row is hidden without it.
pub const AVAILABLE: bool = cfg!(windows);

/// Whether Apollo is registered to start at sign-in.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Registers or removes the sign-in launch. Returns whether the registry now matches `enabled`.
pub fn set_enabled(enabled: bool) -> bool {
    imp::set_enabled(enabled)
}

/// The `Run` value: the running executable, quoted.
#[cfg(any(windows, test))]
fn command_line(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const REG_SZ: u32 = 1;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Apollo";

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
        fn RegSetKeyValueW(
            hkey: isize,
            subkey: *const u16,
            value: *const u16,
            kind: u32,
            data: *const c_void,
            len: u32,
        ) -> i32;
        fn RegDeleteKeyValueW(hkey: isize, subkey: *const u16, value: *const u16) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn is_enabled() -> bool {
        let (subkey, value) = (wide(SUBKEY), wide(VALUE));
        let mut len: u32 = 0;
        // SAFETY: a null data pointer asks only for the size; the strings are NUL-terminated.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        status == ERROR_SUCCESS && len > 2
    }

    pub fn set_enabled(enabled: bool) -> bool {
        let (subkey, value) = (wide(SUBKEY), wide(VALUE));
        if enabled {
            let Ok(exe) = std::env::current_exe() else {
                return false;
            };
            let data = wide(&super::command_line(&exe));
            // SAFETY: `data` is a NUL-terminated UTF-16 string and the length counts its bytes.
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            status == ERROR_SUCCESS
        } else {
            // SAFETY: both strings are NUL-terminated.
            let status =
                unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), value.as_ptr()) };
            status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enabled: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_value_quotes_the_path() {
        let exe = std::path::Path::new(r"C:\Program Files\Apollo\apollo.exe");
        assert_eq!(
            command_line(exe),
            "\"C:\\Program Files\\Apollo\\apollo.exe\""
        );
    }
}
