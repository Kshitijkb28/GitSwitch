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
}
