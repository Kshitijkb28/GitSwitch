//! One place where child processes are created.
//!
//! On Windows a console program started from a GUI application gets a console
//! window of its own unless it is told not to — so every `git status` behind
//! the Changes page would flash a black window. `CREATE_NO_WINDOW` is that
//! "told not to"; on the other platforms these are plain constructors.

use std::ffi::OsStr;

/// `CREATE_NO_WINDOW` from processthreadsapi.h.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A blocking command with no console window.
pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// An async command with no console window.
pub fn async_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// A program by name, found the way a newly opened terminal would find it.
///
/// Windows hands a process its PATH once, when it starts, and GitSwitch sits
/// in the tray for days — so a tool installed since (gh, after the GitHub
/// page said it was missing) stayed "not installed" until a restart. When
/// `name` is not on our own PATH, this looks along the PATH saved in the
/// registry now (the machine's, then the user's) and gives the full path it
/// finds there. Elsewhere, or when nothing is found, it is `name` itself.
pub fn program(name: &str) -> std::ffi::OsString {
    #[cfg(windows)]
    {
        let current = std::env::var_os("PATH");
        let saved = [
            registry_string(windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE, "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment", "Path"),
            registry_string(windows_sys::Win32::System::Registry::HKEY_CURRENT_USER, "Environment", "Path"),
        ];
        let saved: Vec<String> = saved.into_iter().flatten().collect();
        find_program(name, current.as_deref(), &saved)
    }
    #[cfg(not(windows))]
    {
        name.into()
    }
}

/// `name` when `name.exe` is on `current`; else the first `name.exe` along
/// the `saved` PATHs, in order; else `name`.
#[cfg_attr(not(windows), allow(dead_code))]
fn find_program(name: &str, current: Option<&OsStr>, saved: &[String]) -> std::ffi::OsString {
    let exe = format!("{name}.exe");
    let on = |dirs: &OsStr| std::env::split_paths(dirs).map(|d| d.join(&exe)).find(|p| p.is_file());
    if current.is_some_and(|p| on(p).is_some()) {
        return name.into();
    }
    saved.iter().find_map(|p| on(OsStr::new(p))).map(|p| p.into_os_string()).unwrap_or_else(|| name.into())
}

/// A string value from the registry, `%VAR%`s expanded; `None` when missing.
#[cfg(windows)]
fn registry_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };
    let (key, value) = (wide(key), wide(value));
    let mut size = 0u32;
    // SAFETY: documented two-call pattern; buffer sized from the first call.
    // RRF_RT_REG_SZ also accepts REG_EXPAND_SZ, which comes back expanded.
    unsafe {
        if RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        if RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// The process tree behind one child, for killing it whole.
///
/// Unix has process groups for this (`process_group(0)` in git_exec.rs).
/// Windows has Job Objects: every process a job member starts joins the job,
/// and `TerminateJobObject` ends them all — which is what makes a timed-out
/// `git lfs pull` actually stop instead of leaving `git-lfs.exe` downloading
/// behind a reported error. No `KILL_ON_JOB_CLOSE`: a job that merely falls
/// out of scope must not take a legitimate background `git gc --auto` with it,
/// so the job is only ever terminated on purpose.
#[cfg(windows)]
pub struct JobObject(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job object handle is a process-wide kernel handle — Windows lets
// any thread use or close it, and nothing here keeps thread-local state. The
// raw pointer inside `HANDLE` is what makes the compiler say otherwise, and
// without this every command future that holds a job across an `.await`
// stops being `Send`, which Tauri requires of all of them.
#[cfg(windows)]
unsafe impl Send for JobObject {}
#[cfg(windows)]
unsafe impl Sync for JobObject {}

#[cfg(windows)]
impl JobObject {
    /// Put `child` (and, from now on, everything it starts) in a new job.
    /// `None` when Windows refuses, in which case only git itself can be
    /// killed — the pre-existing behaviour, not a new failure.
    pub fn adopt(child: &tokio::process::Child) -> Option<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        let process = child.raw_handle()?;
        // SAFETY: plain Win32 calls with null attributes and no name; the
        // handles are checked before use and closed exactly once in Drop.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            if AssignProcessToJobObject(job, process as _) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(JobObject(job))
        }
    }

    /// End every process in the job, now.
    pub fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the handle is live until Drop.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: closes the one handle this struct owns.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn both_constructors_still_run_the_program_they_are_given() {
        if let Ok(o) = command("git").arg("--version").output() {
            assert!(String::from_utf8_lossy(&o.stdout).starts_with("git version"));
        }
        if let Ok(o) = async_command("git").arg("--version").output().await {
            assert!(String::from_utf8_lossy(&o.stdout).starts_with("git version"));
        }
    }

    #[test]
    fn a_program_installed_after_start_is_found_on_the_saved_path() {
        let root = std::env::temp_dir().join(format!("gitswitch-program-{}", std::process::id()));
        let (old, new, later) = (root.join("old"), root.join("new"), root.join("later"));
        for d in [&old, &new, &later] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(new.join("gh.exe"), b"").unwrap();
        std::fs::write(later.join("gh.exe"), b"").unwrap();
        let path = |dirs: &[&std::path::PathBuf]| std::env::join_paths(dirs).unwrap().into_string().unwrap();
        let saved = [path(&[&old]), path(&[&new, &later])];
        // Not on the PATH we started with: the first one on the saved PATHs.
        assert_eq!(find_program("gh", Some(OsStr::new(&path(&[&old]))), &saved), new.join("gh.exe").into_os_string());
        // On the PATH we started with: left to it, as before.
        assert_eq!(find_program("gh", Some(OsStr::new(&path(&[&later]))), &saved), "gh");
        // Nowhere: the bare name, so the spawn fails as "not installed".
        assert_eq!(find_program("gh", None, &[path(&[&old])]), "gh");
        let _ = std::fs::remove_dir_all(&root);
    }
}
