//! The swap itself runs in a short-lived copy of the installed `Apollo.exe` started as
//! `Apollo.exe --apply-update <pid> <launch|quit>`: it waits for the Apollo that started it to
//! exit, swaps the staged build in, and (for `launch`) starts the new Apollo. It takes no paths
//! from its arguments: the install folder is where it runs from, the staged build is under
//! `%LOCALAPPDATA%\Apollo\updates`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apply::{apply, cleanup_old};
use crate::stage::{discard_staged, mark_failed, staged};

/// The flag that makes `Apollo.exe` run as the swap helper instead of the app.
pub const HELPER_FLAG: &str = "--apply-update";
const WAIT_FOR_APP: Duration = Duration::from_secs(30);
const LOG_LIMIT: u64 = 64 * 1024;

/// `%LOCALAPPDATA%\Apollo\updates` (none where the variable is unset).
pub fn updates_dir() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("Apollo")
            .join("updates"),
    )
}

/// The install folder when `exe` is the packaged `Apollo.exe` (libmpv and the tools beside it),
/// so a `cargo run` build never replaces itself.
pub fn install_dir_for(exe: &Path) -> Option<PathBuf> {
    let name = exe.file_name()?.to_string_lossy();
    if !name.eq_ignore_ascii_case("Apollo.exe") {
        return None;
    }
    let dir = exe.parent()?;
    (dir.join("libmpv-2.dll").is_file() && dir.join("tools").is_dir()).then(|| dir.to_path_buf())
}

/// A line in `updates\update.log` (versions and outcomes only).
pub fn log(updates: &Path, line: &str) {
    let _ = fs::create_dir_all(updates);
    let path = updates.join("update.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = fs::remove_file(&path);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Starts the swap helper (a copy of this very exe) and returns at once. The caller then exits.
pub fn spawn_helper(exe: &Path, relaunch: bool) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .arg(HELPER_FLAG)
        .arg(std::process::id().to_string())
        .arg(if relaunch { "launch" } else { "quit" });
    detach(&mut command);
    command.spawn().map(|_| ())
}

#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn detach(_: &mut std::process::Command) {}

/// Blocks until process `pid` has exited, up to `timeout`. A process that cannot be opened has
/// already gone.
#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    // SAFETY: plain Win32 calls on a handle opened and closed here.
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle == 0 {
            return true;
        }
        let status =
            WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128 - 1) as u32);
        CloseHandle(handle);
        status == WAIT_OBJECT_0
    }
}

#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let start = std::time::Instant::now();
    while proc.exists() {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

/// Runs the helper: `args` are the arguments after the program name. Returns the exit code.
pub fn run_helper(args: &[String]) -> i32 {
    let (Some(updates), Ok(exe)) = (updates_dir(), std::env::current_exe()) else {
        return 1;
    };
    let relaunch = args.get(2).map(String::as_str) == Some("launch");
    let Some(install) = install_dir_for(&exe) else {
        log(
            &updates,
            "helper: not running from a packaged install, nothing done",
        );
        return 1;
    };
    let parent_gone = args
        .get(1)
        .and_then(|pid| pid.parse::<u32>().ok())
        .is_none_or(|pid| wait_for_exit(pid, WAIT_FOR_APP));
    if !parent_gone {
        log(
            &updates,
            "helper: Apollo did not exit in time, update left staged",
        );
        return 1;
    }
    let mut code = 0;
    if let Some(build) = staged(&updates) {
        match apply(&build, &install) {
            Ok(()) => log(&updates, &format!("installed {}", build.version)),
            Err(err) => {
                mark_failed(&updates, &build.version);
                log(
                    &updates,
                    &format!("could not install {}: {err}", build.version),
                );
                code = 1;
            }
        }
        // Applied or not, never apply the same staged build twice.
        discard_staged(&updates);
    }
    cleanup_old(&install);
    if relaunch {
        let mut command = std::process::Command::new(install.join("Apollo.exe"));
        command.current_dir(&install);
        detach(&mut command);
        if let Err(err) = command.spawn() {
            log(&updates, &format!("could not start Apollo again: {err}"));
            code = 1;
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_packaged_exe_is_an_install() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Apollo.exe");
        assert!(install_dir_for(&exe).is_none());
        fs::write(dir.path().join("libmpv-2.dll"), "").unwrap();
        assert!(install_dir_for(&exe).is_none());
        fs::create_dir(dir.path().join("tools")).unwrap();
        assert_eq!(install_dir_for(&exe).as_deref(), Some(dir.path()));
        // The development binary has another name and never counts.
        assert!(install_dir_for(&dir.path().join("apollo-app")).is_none());
    }

    #[test]
    fn a_missing_process_counts_as_exited() {
        assert!(wait_for_exit(u32::MAX - 1, Duration::from_millis(50)));
    }
}
