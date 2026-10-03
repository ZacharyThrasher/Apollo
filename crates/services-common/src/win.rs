//! Windows-only primitives: process-tree containment and user-only private directories.
//!
//! * [`JobObject`] + [`spawn_in_job`]: a child (and every descendant) dies when the job is
//!   terminated or when our process exits, even if we crash (KILL_ON_JOB_CLOSE). The child is
//!   started suspended and only resumed after it is inside the job, so it cannot spawn a
//!   grandchild that escapes (yt-dlp's one-file exe re-spawns itself).
//! * [`PrivateDir`]: a freshly created directory whose DACL is *protected* (no inheritance) and
//!   grants only the current user. Files are created `CREATE_NEW`, so a pre-planted file or
//!   link cannot be followed. Contents are zeroed and removed on drop; stale dirs from dead
//!   processes are swept at startup. Deletion is not forensic erasure (journaling/SSD).

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

const CREATE_SUSPENDED: u32 = 0x0000_0004;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STILL_ACTIVE: u32 = 259;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: HANDLE) -> i32;
}

// ------------------------------------------------------------------ job object

pub struct JobObject(HANDLE);

// A job handle is just a kernel object reference; any thread may use it.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// A job whose processes are killed when the last handle closes.
    pub fn new_kill_on_close() -> io::Result<Self> {
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(|e| {
                let _ = CloseHandle(job);
                io::Error::other(e)
            })?;
            Ok(Self(job))
        }
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        unsafe {
            AssignProcessToJobObject(self.0, HANDLE(child.as_raw_handle())).map_err(io::Error::other)
        }
    }

    /// Kill every process in the job (the whole tree).
    pub fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Spawn `cmd` inside `job`. Started suspended, assigned, then resumed: no window for escape.
/// If assignment fails the child is killed and the error returned (never run uncontained).
pub fn spawn_in_job(cmd: &mut Command, job: &JobObject) -> io::Result<Child> {
    cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    let mut child = cmd.spawn()?;
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let status = unsafe { NtResumeProcess(HANDLE(child.as_raw_handle())) };
    if status < 0 {
        job.terminate();
        let _ = child.wait();
        return Err(io::Error::other(format!("NtResumeProcess failed: {status:#x}")));
    }
    Ok(child)
}

// ------------------------------------------------------------- private folders

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// SID string of the current user, e.g. `S-1-5-21-...`.
pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buf = vec![0u8; needed as usize];
        let r = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            needed,
            &mut needed,
        );
        let _ = CloseHandle(token);
        r.map_err(io::Error::other)?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_str).map_err(io::Error::other)?;
        let len = (0..).take_while(|&i| *sid_str.0.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(sid_str.0, len));
        let _ = LocalFree(Some(HLOCAL(sid_str.0 as *mut c_void)));
        Ok(s)
    }
}

/// Create `path` (and missing parents) as a *persistent* user-only directory: the leaf gets a
/// protected DACL granting only the current user. If it already exists it must be a real
/// directory (not a reparse point); its ACL is not rewritten here.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || !meta.is_dir() {
            return Err(io::Error::other("private dir path is not a plain directory"));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    create_protected_dir(path)
}

/// Creates the leaf directory with a protected DACL (no inheritance, current user only).
/// Fails if it already exists.
fn create_protected_dir(path: &Path) -> io::Result<()> {
    let sddl = format!("D:P(A;OICI;FA;;;{})", current_user_sid()?);
    let sddl_w: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_w.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .map_err(io::Error::other)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let path_w = wide(path.as_os_str());
        let r = CreateDirectoryW(PCWSTR(path_w.as_ptr()), Some(&sa));
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r.map_err(io::Error::other)
    }
}

/// A private directory only the current user can open. Removed (best effort, zeroing files)
/// on drop.
#[derive(Debug)]
pub struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    /// Creates `base/{prefix}-{pid}-{random}`. `base` is created if missing (inherited ACL;
    /// only the leaf is locked down). Fails if the leaf already exists or is a reparse point.
    pub fn create(base: &Path, prefix: &str) -> io::Result<Self> {
        std::fs::create_dir_all(base)?;
        let name = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            crate::random_hex(8)
        );
        let path = base.join(name);
        create_protected_dir(&path)?;
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            let _ = std::fs::remove_dir(&path);
            return Err(io::Error::other("private dir is a reparse point"));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write a new file (fails if it exists; never follows a link).
    pub fn create_file(&self, name: &str, contents: &[u8]) -> io::Result<PathBuf> {
        if name.contains(['/', '\\']) || name.contains("..") {
            return Err(io::Error::other("file name must be a single component"));
        }
        let p = self.path.join(name);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&p)?;
        f.write_all(contents)?;
        f.sync_all()?;
        Ok(p)
    }

    /// Overwrite with zeros then delete everything inside.
    fn wipe(path: &Path) {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                let p = e.path();
                if let Ok(m) = e.metadata() {
                    if m.is_file() {
                        if let Ok(mut f) = OpenOptions::new().write(true).open(&p) {
                            let _ = f.write_all(&vec![0u8; m.len() as usize]);
                            let _ = f.sync_all();
                        }
                    }
                }
                let _ = std::fs::remove_file(&p);
            }
        }
        let _ = std::fs::remove_dir(path);
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        Self::wipe(&self.path);
    }
}

fn process_alive(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == STILL_ACTIVE
    }
}

/// Remove private dirs (`{prefix}-{pid}-...`) left behind by processes that are no longer
/// running (crash/kill). Returns how many were removed. Run at startup.
pub fn sweep_stale(base: &Path, prefix: &str) -> usize {
    let Ok(rd) = std::fs::read_dir(base) else {
        return 0;
    };
    let me = std::process::id();
    let mut removed = 0;
    for e in rd.flatten() {
        let name = e.file_name();
        let name = std::ffi::OsString::from_wide(&name.encode_wide().collect::<Vec<_>>());
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(&format!("{prefix}-")) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if pid != me && !process_alive(pid) {
            PrivateDir::wipe(&e.path());
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("apollo-win-test-{tag}-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn private_dir_is_user_only_and_cleans_up() {
        let base = scratch("acl");
        let path;
        {
            let d = PrivateDir::create(&base, "t").unwrap();
            path = d.path().to_path_buf();
            let f = d.create_file("cookies.txt", b"canary").unwrap();
            assert!(f.exists());
            // CREATE_NEW: a second file with the same name must fail.
            assert!(d.create_file("cookies.txt", b"x").is_err());
            assert!(d.create_file("..\\evil", b"x").is_err());
            // Only the current user has an ACE, and inheritance is off (icacls prints one
            // ACE per line, indented; the first line starts with the path).
            let out = Command::new("icacls").arg(&path).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let user = std::env::var("USERNAME").unwrap();
            let aces: Vec<String> = text
                .lines()
                .filter_map(|l| l.find(":(").map(|_| l.to_string()))
                .map(|l| l.replace(&path.display().to_string(), ""))
                .collect();
            assert_eq!(aces.len(), 1, "exactly one ACE expected: {text}");
            assert!(aces[0].contains(&user), "{text}");
            assert!(!text.contains("(I)"), "inherited ACE present: {text}");
        }
        assert!(!path.exists(), "dir removed on drop");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sweep_removes_dirs_of_dead_processes_only() {
        let base = scratch("sweep");
        // pid 4294967 is not a live process; ours is.
        let dead = base.join("t-4294967-aaaa");
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("c.txt"), b"x").unwrap();
        let live = PrivateDir::create(&base, "t").unwrap();
        assert_eq!(sweep_stale(&base, "t"), 1);
        assert!(!dead.exists());
        assert!(live.path().exists(), "own dir survives the sweep");
        drop(live);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn terminating_the_job_kills_the_whole_tree() {
        // cmd /c "ping ... & ping ..." spawns a grandchild (ping.exe) that outlives a plain kill.
        let ping_running = || {
            let out = Command::new("tasklist")
                .args(["/FI", "IMAGENAME eq PING.EXE", "/FO", "CSV", "/NH"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).matches("PING.EXE").count()
        };
        let before = ping_running();
        let job = JobObject::new_kill_on_close().unwrap();
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_in_job(&mut cmd, &job).unwrap();
        let ppid = child.id();
        // Give cmd time to launch ping.exe.
        let deadline = Instant::now() + Duration::from_secs(5);
        while ping_running() <= before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ping_running() > before, "grandchild ping.exe did not start (pid {ppid})");
        job.terminate();
        let _ = child.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while ping_running() > before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(ping_running(), before, "grandchild must die with the job");
    }
}
