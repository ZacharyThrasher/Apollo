//! Finding the Discord client's IPC endpoint: a named pipe `\\?\pipe\discord-ipc-N` on Windows
//! (N is 0 to 9, the first client running claims the lowest free one), a Unix socket of the same
//! name elsewhere (development machines).

use super::ipc::Transport;

const SLOTS: u8 = 10;

#[cfg(windows)]
pub fn connect() -> Option<Box<dyn Transport>> {
    use std::fs::OpenOptions;
    (0..SLOTS).find_map(|n| {
        let pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!(r"\\?\pipe\discord-ipc-{n}"))
            .ok()?;
        Some(Box::new(pipe) as Box<dyn Transport>)
    })
}

#[cfg(unix)]
pub fn connect() -> Option<Box<dyn Transport>> {
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::Duration;

    let dirs: Vec<PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .chain([PathBuf::from("/tmp")])
        .collect();
    dirs.iter().find_map(|dir| {
        (0..SLOTS).find_map(|n| {
            let stream = UnixStream::connect(dir.join(format!("discord-ipc-{n}"))).ok()?;
            // A client that stops answering must not hang the worker for good.
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok()?;
            Some(Box::new(stream) as Box<dyn Transport>)
        })
    })
}

#[cfg(not(any(windows, unix)))]
pub fn connect() -> Option<Box<dyn Transport>> {
    let _ = SLOTS;
    None
}
