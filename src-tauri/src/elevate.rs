//! Running the lock helper with administrator rights — one job at a time,
//! through the operating system's own prompt.
//!
//! macOS: `osascript … with administrator privileges`, passing paths as
//! AppleScript arguments so nothing is ever quoted by hand. Linux: `pkexec`,
//! falling back to showing the exact `sudo` command when no polkit agent can
//! prompt (ssh, a text console). Windows arrives with its own step. Debug
//! builds also honour `GITSWITCH_ELEVATE` so the verify suites can run the
//! helper directly or simulate a cancelled or denied prompt without a dialog.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The prompt can sit for as long as the person needs; a job never takes long.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const PROMPT_TEXT: &str = "GitSwitch needs an administrator to change a push lock.";

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Held while a prompt is open; a second request is refused as `busy` rather
/// than stacking two dialogs.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

pub fn begin() -> Option<Guard> {
    IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| Guard)
}

pub fn in_flight() -> bool {
    IN_FLIGHT.load(Ordering::SeqCst)
}

#[derive(Debug)]
pub enum Elevated {
    /// The helper ran. `stdout` may be empty when the OS layer swallowed it
    /// (AppleScript raises on a non-zero exit) — the caller then reads the
    /// result file the helper wrote next to the job.
    Ran { code: i32, stdout: String, stderr: String },
    Cancelled,
    Denied,
    NoAgent,
    Manual { command: String },
    /// Built only on platforms without an elevation path yet (Windows).
    #[allow(dead_code)]
    Unsupported(String),
    Failed(String),
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The command a person would run themselves, for the Linux fallback and for
/// the README.
pub fn manual_command(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> String {
    let mut parts = vec!["sudo".to_string(), shell_quote(&helper.to_string_lossy())];
    if bootstrap {
        parts.push("--bootstrap".into());
    }
    parts.push(shell_quote(&job.to_string_lossy()));
    parts.push("--sha256".into());
    parts.push(sha.to_string());
    parts.join(" ")
}

fn helper_args(job: &Path, sha: &str, bootstrap: bool) -> Vec<String> {
    let mut v = Vec::new();
    if bootstrap {
        v.push("--bootstrap".to_string());
    }
    v.push(job.to_string_lossy().to_string());
    v.push("--sha256".to_string());
    v.push(sha.to_string());
    v
}

#[cfg(debug_assertions)]
fn test_mode() -> Option<String> {
    std::env::var("GITSWITCH_ELEVATE").ok().filter(|s| !s.is_empty())
}

#[cfg(not(debug_assertions))]
fn test_mode() -> Option<String> {
    None
}

/// Run the helper directly, unprivileged — the verify suites' mode, where the
/// helper honours a test root instead of needing root. With `via_sudo` the
/// same thing runs through `sudo -n` (never prompts): CI runners have
/// passwordless sudo, so the real root code path is exercised there.
async fn run_direct(helper: &Path, job: &Path, sha: &str, bootstrap: bool, via_sudo: bool) -> Elevated {
    let mut cmd = if via_sudo {
        let mut c = tokio::process::Command::new("sudo");
        c.arg("-n").arg(helper);
        c
    } else {
        tokio::process::Command::new(helper)
    };
    cmd.args(helper_args(job, sha, bootstrap));
    let out = tokio::time::timeout(PROMPT_TIMEOUT, cmd.output()).await;
    match out {
        Ok(Ok(o)) => Elevated::Ran {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        },
        Ok(Err(e)) => Elevated::Failed(format!("could not start {}: {}", helper.display(), e)),
        Err(_) => Elevated::Failed("the lock helper did not finish in time".into()),
    }
}

pub async fn run(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> Elevated {
    if let Some(mode) = test_mode() {
        return match mode.as_str() {
            "direct" => run_direct(helper, job, sha, bootstrap, false).await,
            "sudo" => run_direct(helper, job, sha, bootstrap, true).await,
            "cancelled" => Elevated::Cancelled,
            "denied" => Elevated::Denied,
            "no-agent" => Elevated::NoAgent,
            "manual" => Elevated::Manual { command: manual_command(helper, job, sha, bootstrap) },
            "failed" => Elevated::Failed("simulated failure".into()),
            other => Elevated::Failed(format!("unknown GITSWITCH_ELEVATE mode {}", other)),
        };
    }
    #[cfg(target_os = "macos")]
    {
        macos(helper, job, sha, bootstrap).await
    }
    #[cfg(target_os = "linux")]
    {
        linux(helper, job, sha, bootstrap).await
    }
    #[cfg(windows)]
    {
        windows(helper, job, sha, bootstrap).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (helper, job, sha, bootstrap);
        Elevated::Unsupported("Push locks are not available on this operating system.".into())
    }
}

/// UAC: `ShellExecuteExW` with the `runas` verb. On an administrator account
/// this is a consent click, not a password — disclosed in the caveats. No
/// stdout comes back across the boundary; the caller reads the result file.
#[cfg(windows)]
async fn windows(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> Elevated {
    let (helper, job, sha) = (helper.to_path_buf(), job.to_path_buf(), sha.to_string());
    let run = tokio::task::spawn_blocking(move || windows_blocking(&helper, &job, &sha, bootstrap));
    match tokio::time::timeout(PROMPT_TIMEOUT, run).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => Elevated::Failed(format!("the elevation thread failed: {}", e)),
        Err(_) => Elevated::Failed("the administrator prompt was left open for too long".into()),
    }
}

#[cfg(windows)]
fn windows_blocking(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> Elevated {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NO_CONSOLE, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let wide = |s: &OsStr| -> Vec<u16> { s.encode_wide().chain(std::iter::once(0)).collect() };
    let verb = wide(OsStr::new("runas"));
    let file = wide(helper.as_os_str());
    let params_s = format!(
        "{}\"{}\" --sha256 {}",
        if bootstrap { "--bootstrap " } else { "" },
        job.to_string_lossy(),
        sha
    );
    let params = wide(OsStr::new(&params_s));
    // SAFETY: a zeroed SHELLEXECUTEINFOW is the documented starting point; the
    // wide strings outlive the call; handles are closed.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_NO_CONSOLE;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let err = GetLastError();
            return if err == ERROR_CANCELLED {
                Elevated::Cancelled
            } else {
                Elevated::Failed(format!("Windows could not start the lock helper (error {})", err))
            };
        }
        if info.hProcess.is_null() {
            return Elevated::Failed("Windows started the helper but gave no process handle".into());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code: u32 = 0;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Elevated::Ran { code: code as i32, stdout: String::new(), stderr: String::new() }
    }
}

/// The AppleScript error number at the end of osascript's stderr, e.g.
/// `execution error: User canceled. (-128)`.
pub fn applescript_error_code(stderr: &str) -> Option<i64> {
    let t = stderr.trim_end();
    let close = t.rfind(')')?;
    let open = t[..close].rfind('(')?;
    t[open + 1..close].trim().parse().ok()
}

/// Everything before the trailing `(code)`, minus osascript's own prefix.
pub fn applescript_error_text(stderr: &str) -> String {
    let t = stderr.trim();
    let body = match t.rfind('(') {
        Some(i) if applescript_error_code(t).is_some() => &t[..i],
        _ => t,
    };
    let body = body.trim();
    match body.find("execution error: ") {
        Some(i) => body[i + "execution error: ".len()..].trim().to_string(),
        None => body.to_string(),
    }
}

#[cfg(target_os = "macos")]
async fn macos(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> Elevated {
    // Paths travel as AppleScript argv and are quoted by `quoted form of`; the
    // two flags are literal text in the script, so no argument ever starts
    // with a dash and osascript never mistakes one for its own option.
    let flag = if bootstrap { " --bootstrap " } else { " " };
    let script = format!(
        "on run argv\n\
         do shell script (quoted form of item 1 of argv) & \"{flag}\" & (quoted form of item 2 of argv) & \" --sha256 \" & (quoted form of item 3 of argv) with administrator privileges with prompt \"{prompt}\"\n\
         end run",
        flag = flag,
        prompt = PROMPT_TEXT
    );
    let out = tokio::time::timeout(
        PROMPT_TIMEOUT,
        tokio::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .arg(helper)
            .arg(job)
            .arg(sha)
            .output(),
    )
    .await;
    let o = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Elevated::Failed(format!("could not start osascript: {}", e)),
        Err(_) => return Elevated::Failed("the administrator prompt was left open for too long".into()),
    };
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    if o.status.success() {
        return Elevated::Ran { code: 0, stdout, stderr };
    }
    match applescript_error_code(&stderr) {
        Some(-128) | Some(-60006) => Elevated::Cancelled,
        Some(-60005) => Elevated::Denied,
        Some(-60007) => Elevated::NoAgent,
        Some(-60031) => Elevated::Failed("macOS could not start the lock helper".into()),
        Some(code) if (1..=255).contains(&code) => Elevated::Ran {
            code: code as i32,
            stdout: String::new(),
            stderr: applescript_error_text(&stderr),
        },
        _ => Elevated::Failed(applescript_error_text(&stderr)),
    }
}

/// Linux: `pkexec` (a polkit agent shows the dialog); without an agent — ssh,
/// a text console — `sudo -A` with an askpass program if one is available;
/// failing that, the exact command for the person to run.
#[cfg(target_os = "linux")]
async fn linux(helper: &Path, job: &Path, sha: &str, bootstrap: bool) -> Elevated {
    let manual = manual_command(helper, job, sha, bootstrap);
    let out = tokio::time::timeout(
        PROMPT_TIMEOUT,
        tokio::process::Command::new("pkexec").arg(helper).args(helper_args(job, sha, bootstrap)).output(),
    )
    .await;
    let no_agent = match out {
        Ok(Ok(o)) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            match o.status.code() {
                Some(0) => return Elevated::Ran { code: 0, stdout, stderr },
                Some(126) => return Elevated::Cancelled,
                Some(127) => {
                    let s = stderr.to_ascii_lowercase();
                    if !(s.contains("authentication agent") || s.contains("cannot run program") || s.contains("no session")) {
                        return Elevated::Denied;
                    }
                    true
                }
                Some(code) => return Elevated::Ran { code, stdout, stderr },
                None => return Elevated::Failed("pkexec was killed".into()),
            }
        }
        Ok(Err(_)) => true,
        Err(_) => return Elevated::Failed("the administrator prompt was left open for too long".into()),
    };
    if no_agent {
        if let Some(askpass) = askpass_program() {
            let out = tokio::time::timeout(
                PROMPT_TIMEOUT,
                tokio::process::Command::new("sudo")
                    .env("SUDO_ASKPASS", &askpass)
                    .arg("-A")
                    .arg(helper)
                    .args(helper_args(job, sha, bootstrap))
                    .output(),
            )
            .await;
            if let Ok(Ok(o)) = out {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                return match o.status.code() {
                    Some(0) => Elevated::Ran { code: 0, stdout, stderr },
                    Some(1) if stderr.to_ascii_lowercase().contains("incorrect password") => Elevated::Denied,
                    Some(1) => Elevated::Cancelled,
                    Some(code) => Elevated::Ran { code, stdout, stderr },
                    None => Elevated::Failed("sudo was killed".into()),
                };
            }
        }
    }
    Elevated::Manual { command: manual }
}

/// `SUDO_ASKPASS` if set, else the first askpass program on this system.
#[cfg(target_os = "linux")]
fn askpass_program() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("SUDO_ASKPASS") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for candidate in ["/usr/bin/ssh-askpass", "/usr/lib/ssh/x11-ssh-askpass", "/usr/libexec/openssh/ssh-askpass", "/usr/bin/lxqt-openssh-askpass", "/usr/bin/ksshaskpass"] {
        let p = std::path::PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_errors_are_parsed_by_their_trailing_code() {
        let cancel = "42:200: execution error: User canceled. (-128)\n";
        assert_eq!(applescript_error_code(cancel), Some(-128));
        let helper = "42:200: execution error: gitswitch-lock-helper: the job file does not match the checksum that was approved (3)";
        assert_eq!(applescript_error_code(helper), Some(3));
        assert_eq!(
            applescript_error_text(helper),
            "gitswitch-lock-helper: the job file does not match the checksum that was approved"
        );
        assert_eq!(applescript_error_code("no code here"), None);
        assert_eq!(applescript_error_text("plain failure"), "plain failure");
    }

    #[test]
    fn the_manual_command_quotes_paths_and_carries_the_hash() {
        let c = manual_command(
            Path::new("/Library/PrivilegedHelperTools/com.gitswitch.lock-helper"),
            Path::new("/Users/me/Library/Application Support/com.gitswitch.app/lock-jobs/a.json"),
            "abcd",
            true,
        );
        assert!(c.starts_with("sudo '/Library/PrivilegedHelperTools/com.gitswitch.lock-helper' --bootstrap '"));
        assert!(c.contains("Application Support"));
        assert!(c.ends_with("--sha256 abcd"));
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn only_one_prompt_at_a_time() {
        let g = begin().expect("first begin succeeds");
        assert!(in_flight());
        assert!(begin().is_none(), "a second request is busy");
        drop(g);
        assert!(!in_flight());
    }
}
