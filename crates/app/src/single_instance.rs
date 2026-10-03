//! One Apollo per signed-in user. A second launch (the desktop shortcut while the first sits in
//! the tray, say) asks the first to show its window and exits, instead of fighting it over the
//! audio device, the database and the helpers.
//!
//! Windows only: a named mutex says who is first, and a named event carries the "show your
//! window" request. The first instance waits on the event from a small thread and forwards each
//! signal to the UI through a channel. Nothing is read from the second launch's arguments.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::sync::OnceLock;

const MUTEX_NAME: &str = "Local\\Apollo.SingleInstance";
const EVENT_NAME: &str = "Local\\Apollo.ShowWindow";
const ERROR_ALREADY_EXISTS: u32 = 183;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> isize;
    fn CreateEventW(
        attrs: *const c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> isize;
    fn SetEvent(handle: isize) -> i32;
    fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    fn GetLastError() -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A kernel handle the process keeps until it exits.
#[derive(Clone, Copy)]
struct Handle(isize);

// SAFETY: a kernel object handle is a plain id usable from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// The first instance's handles; kept for the life of the process (the OS releases the mutex
/// when it exits, however it exits).
struct Held {
    _mutex: Handle,
    show: Handle,
}

static HELD: OnceLock<Held> = OnceLock::new();

/// Claims the right to be the running Apollo. `true`: this is the first instance and carries
/// on. `false`: another is running and has been asked to show its window; the caller exits.
/// If the OS refuses the mutex altogether the app carries on rather than fail to start.
pub fn acquire() -> bool {
    acquire_named(MUTEX_NAME, EVENT_NAME)
}

fn acquire_named(mutex_name: &str, event_name: &str) -> bool {
    let (mutex_name, event_name) = (wide(mutex_name), wide(event_name));
    // SAFETY: names are NUL-terminated; null attributes mean defaults.
    let (mutex, already, event) = unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
        let already = GetLastError() == ERROR_ALREADY_EXISTS;
        // Auto-reset, so one signal wakes the first instance exactly once.
        let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
        (mutex, already, event)
    };
    if mutex == 0 || event == 0 {
        eprintln!("single instance: could not create the lock; starting anyway");
        return true;
    }
    if already {
        // SAFETY: `event` is a valid handle just returned by CreateEventW.
        unsafe { SetEvent(event) };
        return false;
    }
    let _ = HELD.set(Held {
        _mutex: Handle(mutex),
        show: Handle(event),
    });
    true
}

/// Calls `on_second_launch` (on a helper thread) each time a later launch asks for the window.
/// Does nothing if this process did not take the lock.
pub fn listen(on_second_launch: impl Fn() + Send + 'static) {
    let Some(held) = HELD.get() else {
        return;
    };
    let show = held.show;
    std::thread::spawn(move || loop {
        // SAFETY: the handle lives as long as the process.
        if unsafe { WaitForSingleObject(show.0, INFINITE) } != WAIT_OBJECT_0 {
            break;
        }
        on_second_launch();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_second_launch_loses_and_wakes_the_first() {
        let name = format!("Local\\Apollo.Test.{}", std::process::id());
        let (mutex, event) = (format!("{name}.Mutex"), format!("{name}.Event"));
        assert!(acquire_named(&mutex, &event), "the first launch wins");
        let (tx, rx) = channel();
        listen(move || {
            let _ = tx.send(());
        });
        assert!(!acquire_named(&mutex, &event), "the second launch loses");
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
