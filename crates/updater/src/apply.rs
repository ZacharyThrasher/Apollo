//! Swapping a staged build into the install folder. Windows lets a running program be renamed but
//! not overwritten, so each file in the way is renamed to `<name>.old` and the new one copied
//! in; if anything fails, the renames are undone and the old build is exactly as it was. The
//! `.old` leftovers (a file still in use cannot be deleted) are swept up by [`cleanup_old`] on
//! the next start.

use std::fs;
use std::path::{Path, PathBuf};

use crate::stage::Staged;

/// Every file under `root`, as paths relative to it.
fn list_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
            let entry = entry.map_err(|err| format!("read {}: {err}", dir.display()))?;
            let path = entry.path();
            if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
                walk(&path, root, out)?;
            } else {
                out.push(
                    path.strip_prefix(root)
                        .map_err(|err| err.to_string())?
                        .to_path_buf(),
                );
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// A free `<file>.old`, `<file>.old1`, ... name, clearing a stale one where it can.
fn old_name(dest: &Path) -> PathBuf {
    let base = dest
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for n in 0.. {
        let suffix = if n == 0 { String::new() } else { n.to_string() };
        let candidate = dest.with_file_name(format!("{base}.old{suffix}"));
        if !candidate.exists() || fs::remove_file(&candidate).is_ok() {
            return candidate;
        }
    }
    unreachable!()
}

/// Puts the staged build over `install`. Either every file is replaced or none is.
pub fn apply(staged: &Staged, install: &Path) -> Result<(), String> {
    let files = list_files(&staged.files)?;
    // (destination, what its old content was renamed to)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<(), String> {
        for relative in &files {
            let dest = install.join(relative);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
            }
            let moved = if dest.exists() {
                let old = old_name(&dest);
                fs::rename(&dest, &old)
                    .map_err(|err| format!("could not move {} aside: {err}", relative.display()))?;
                Some(old)
            } else {
                None
            };
            done.push((dest.clone(), moved));
            fs::copy(staged.files.join(relative), &dest)
                .map_err(|err| format!("could not write {}: {err}", relative.display()))?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        for (dest, moved) in done.into_iter().rev() {
            let _ = fs::remove_file(&dest);
            if let Some(old) = moved {
                let _ = fs::rename(old, dest);
            }
        }
        return Err(err);
    }
    cleanup_old(install);
    Ok(())
}

/// True for `x.old`, `x.old1`, `x.old23`.
fn is_leftover(name: &str) -> bool {
    match name.rfind(".old") {
        Some(at) if at > 0 => name[at + 4..].bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

/// Deletes the `.old` files an earlier swap left; one still in use stays for next time.
pub fn cleanup_old(install: &Path) {
    fn walk(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&path),
                Ok(_) => {
                    if is_leftover(&entry.file_name().to_string_lossy()) {
                        let _ = fs::remove_file(&path);
                    }
                }
                Err(_) => {}
            }
        }
    }
    walk(install);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Version;

    fn staged_with(root: &Path, files: &[(&str, &str)]) -> Staged {
        let dir = root.join("files");
        for (name, data) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        }
        Staged {
            version: Version::parse("0.1.2").unwrap(),
            files: dir,
        }
    }

    #[test]
    fn replaces_adds_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Apollo");
        fs::create_dir_all(install.join("tools")).unwrap();
        fs::write(install.join("Apollo.exe"), "old").unwrap();
        fs::write(install.join("keep.txt"), "mine").unwrap();
        let staged = staged_with(
            &dir.path().join("staged"),
            &[("Apollo.exe", "new"), ("tools/yt-dlp.exe", "tool")],
        );
        apply(&staged, &install).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Apollo.exe")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(install.join("tools/yt-dlp.exe")).unwrap(),
            "tool"
        );
        assert_eq!(
            fs::read_to_string(install.join("keep.txt")).unwrap(),
            "mine"
        );
        assert!(!install.join("Apollo.exe.old").exists());
    }

    #[test]
    fn a_failure_puts_everything_back() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Apollo");
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("a.dll"), "old a").unwrap();
        // A file sits where the second entry needs a folder, so that entry cannot be written.
        fs::write(install.join("sub"), "in the way").unwrap();
        let staged = staged_with(
            &dir.path().join("staged"),
            &[("a.dll", "new a"), ("sub/b.dll", "new b")],
        );
        assert!(apply(&staged, &install).is_err());
        assert_eq!(fs::read_to_string(install.join("a.dll")).unwrap(), "old a");
        assert_eq!(
            fs::read_to_string(install.join("sub")).unwrap(),
            "in the way"
        );
        assert!(!install.join("a.dll.old").exists());
    }

    #[test]
    fn recognises_leftovers() {
        assert!(is_leftover("Apollo.exe.old"));
        assert!(is_leftover("Apollo.exe.old12"));
        assert!(!is_leftover("Apollo.exe"));
        assert!(!is_leftover("old.txt"));
        assert!(!is_leftover(".old"));
        assert!(!is_leftover("a.older"));
    }

    #[test]
    fn cleanup_removes_only_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("tools")).unwrap();
        fs::write(dir.path().join("Apollo.exe.old"), "x").unwrap();
        fs::write(dir.path().join("tools/deno.exe.old1"), "x").unwrap();
        fs::write(dir.path().join("Apollo.exe"), "x").unwrap();
        cleanup_old(dir.path());
        assert!(dir.path().join("Apollo.exe").exists());
        assert!(!dir.path().join("Apollo.exe.old").exists());
        assert!(!dir.path().join("tools/deno.exe.old1").exists());
    }

    /// The point of the rename dance: a program that is running cannot be overwritten, but it
    /// can be moved aside, and it keeps running.
    #[cfg(windows)]
    #[test]
    fn replaces_a_running_program() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Apollo");
        fs::create_dir_all(&install).unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        let running = install.join("Apollo.exe");
        fs::copy(
            std::path::Path::new(&system)
                .join("System32")
                .join("cmd.exe"),
            &running,
        )
        .unwrap();
        let mut child = std::process::Command::new(&running)
            .args(["/c", "ping -n 6 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let staged = staged_with(&dir.path().join("staged"), &[("Apollo.exe", "new build")]);
        let result = apply(&staged, &install);
        let still_running = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
        assert!(still_running);
        assert_eq!(
            fs::read_to_string(install.join("Apollo.exe")).unwrap(),
            "new build"
        );
        cleanup_old(&install);
        assert!(!install.join("Apollo.exe.old").exists());
    }
}
