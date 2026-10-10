//! A real shell, in a pty, scoped to one repository.
//!
//! Everything else in this app runs one explicit git command and reports what
//! changed. This does the opposite: it hands the person a shell. That is
//! deliberate — there are things git does that no button will ever cover — but
//! it needs care in two places the rest of the app does not.
//!
//! **Identity comes for free.** The app never passes `user.name`/`user.email`
//! on the command line; `git_config.rs` writes `includeIf "gitdir:…"` rules into
//! `~/.gitconfig` instead. A shell started in a repository folder therefore
//! resolves the same account, the same SSH key and the same per-profile push
//! block that the app would. Nothing here has to arrange that.
//!
//! **The push lock does not come for free.** It lives in admin-owned *system*
//! git config, and `push_lock::caveats()` already tells people the honest truth:
//! `GIT_CONFIG_NOSYSTEM=1` skips system scope. In a real shell that is one
//! `export` away — so shipping a terminal inside the app whose promise is "only
//! the administrator password turns this off" means guarding that one line.
//! `shim_script` is the answer: a `git` wrapper first on the session's PATH.
//! It is a guard-rail against a slip, not a barrier against intent, and it says
//! so rather than pretending otherwise.

use crate::error::AppError;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// git subcommand arguments that take a value of their own, so the value is not
/// mistaken for the subcommand: `git -c foo=bar push` is a push.
const GIT_ARGS_WITH_VALUES: &str = "-c|-C|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix";

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The shell's process id: the Ports page refuses to stop it, and the
    /// test that proves none is left behind reads it.
    pid: Option<u32>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // A shell that outlived its window would keep the repository busy with
        // nothing on screen to explain it.
        let _ = self.killer.kill();
    }
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The pids of every shell the drawer has open: the Ports page must never
/// offer to stop one (closing the drawer is how).
pub(crate) fn shell_pids() -> Vec<u32> {
    sessions().lock().map(|m| m.values().filter_map(|s| s.pid).collect()).unwrap_or_default()
}

fn app_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.gitswitch.app"))
}

fn shim_dir() -> Option<PathBuf> {
    Some(app_dir()?.join("terminal-shim"))
}

/// The real git, found the way the shell would but ignoring our own shim, so a
/// regenerated shim can never end up calling itself.
fn real_git(skip: &Path) -> PathBuf {
    // On macOS the push lock is written through the system git (/usr/bin/git),
    // into that git's system config. A Homebrew git first on PATH reads a
    // different system config and would walk straight past the lock — so the
    // shim hands off to the same git the lock was written with.
    if cfg!(target_os = "macos") {
        let sys = crate::push_lock::system_git();
        if sys.is_absolute() && sys.is_file() {
            return sys;
        }
    }
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir == skip {
                continue;
            }
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("git")
}

/// Split a byte stream on a character boundary.
///
/// A pty read can end in the middle of a multi-byte character, and decoding
/// that half lossily would put a replacement character in the middle of
/// somebody's branch name. Whatever does not decode yet is carried to the next
/// read instead.
pub fn split_utf8(buf: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(buf) {
        Ok(s) => (s.to_string(), Vec::new()),
        Err(e) => {
            let good = e.valid_up_to();
            // An invalid sequence (rather than a truncated one) is decoded
            // lossily; carrying it forever would stall the stream.
            let carry = if e.error_len().is_some() {
                let skip = good + e.error_len().unwrap_or(1);
                return (
                    format!(
                        "{}\u{FFFD}",
                        std::str::from_utf8(&buf[..good]).unwrap_or_default()
                    ),
                    buf[skip.min(buf.len())..].to_vec(),
                );
            } else {
                buf[good..].to_vec()
            };
            (
                std::str::from_utf8(&buf[..good]).unwrap_or_default().to_string(),
                carry,
            )
        }
    }
}

/// The `git` wrapper placed first on the session's PATH.
///
/// It guards three things, each one something the app refuses everywhere else,
/// and each refusal names a way forward that is not "disable the protection".
pub fn shim_script(real_git: &str) -> String {
    format!(
        r##"#!/bin/sh
# GitSwitch terminal shim. Placed first on PATH for this window only.
# It guards what the app refuses everywhere; it never prints a way to switch
# a lock off. An absolute path to git, or a PATH edit, steps around it — that
# is deliberate, it is a guard-rail against a slip, not a barrier.

skip=0
verb=""
for a in "$@"; do
  if [ "$skip" = "1" ]; then skip=0; continue; fi
  case "$a" in
    {with_values}) skip=1; continue ;;
    -*) continue ;;
    *) verb="$a"; break ;;
  esac
done

if [ "$verb" = "push" ]; then
  bypass=""
  [ -n "$GIT_CONFIG_NOSYSTEM" ] && bypass="GIT_CONFIG_NOSYSTEM"
  [ -n "$GIT_CONFIG_SYSTEM" ] && bypass="GIT_CONFIG_SYSTEM"
  if [ -n "$bypass" ]; then
    echo "GitSwitch: this push would skip the system git config, which is where a push lock lives." >&2
    echo "Unset $bypass first, or use your own Terminal if you really mean to." >&2
    exit 1
  fi
fi

# `-n` means --no-verify for commit but --dry-run for push, so only commit
# takes the short form.
if [ "$verb" = "commit" ] || [ "$verb" = "push" ]; then
  for a in "$@"; do
    if [ "$a" = "--no-verify" ] || {{ [ "$verb" = "commit" ] && [ "$a" = "-n" ]; }}; then
      echo "GitSwitch: --no-verify skips the identity guard that stops commits from the wrong account." >&2
      echo "Drop the flag, or use your own Terminal if you really mean to." >&2
      exit 1
    fi
  done
fi

if [ "$verb" = "push" ]; then
  seen_verb=0
  for a in "$@"; do
    case "$a" in
      --force|-f|--force-with-lease|--force-with-lease=*)
        echo "GitSwitch never force-pushes. Use your own Terminal if you really mean to." >&2
        exit 1 ;;
      push) seen_verb=1 ;;
      # A refspec beginning with + is a force in another spelling.
      +*)
        if [ "$seen_verb" = "1" ]; then
          echo "GitSwitch never force-pushes, and a leading + is a force. Use your own Terminal if you mean to." >&2
          exit 1
        fi ;;
    esac
  done
fi

exec "{real_git}" "$@"
"##,
        with_values = GIT_ARGS_WITH_VALUES,
        real_git = real_git,
    )
}

/// The guard binary (the `git-shim` crate), bundled beside the app like the
/// lock helper. On Windows it is copied into the shim directory as `git.exe`.
const SHIM_BIN_NAME: &str = "gitswitch-git-shim";

/// Write the shim; return the directory to put first on PATH and the real git
/// the session should hand off to.
fn install_shim() -> Result<(PathBuf, PathBuf), AppError> {
    let dir = shim_dir().ok_or_else(|| AppError::Command("No application data directory".into()))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Command(format!("Could not create {}: {}", dir.display(), e)))?;
    let git = real_git(&dir);
    if cfg!(windows) {
        // No /bin/sh here, and a batch file cannot parse arguments faithfully;
        // the guard is a small console program instead, which reads the real
        // git from GITSWITCH_REAL_GIT (set on the session below).
        let src = crate::push_lock::bundled_sidecar_path(SHIM_BIN_NAME).ok_or_else(|| {
            AppError::Command(format!("The terminal's git guard ({}.exe) is not beside the app", SHIM_BIN_NAME))
        })?;
        let dst = dir.join("git.exe");
        let unchanged = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
            _ => false,
        };
        if !unchanged {
            // A shell that is still open may hold the old file; then the old
            // guard keeps serving it and the next open gets the new one.
            if let Err(e) = std::fs::copy(&src, &dst) {
                if !dst.is_file() {
                    return Err(AppError::Command(format!("Could not place the git guard: {}", e)));
                }
            }
        }
        return Ok((dir, git));
    }
    let path = dir.join("git");
    std::fs::write(&path, shim_script(&git.to_string_lossy()))
        .map_err(|e| AppError::Command(format!("Could not write the git shim: {}", e)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok((dir, git))
}

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
    }
}

/// Start a shell in `repo_path` and stream everything it prints down `output`.
pub fn open(
    repo_path: &str,
    cols: u16,
    rows: u16,
    output: tauri::ipc::Channel<String>,
) -> Result<String, AppError> {
    open_with(repo_path, cols, rows, move |chunk| output.send(chunk).is_ok())
}

/// The real thing. `sink` receives each decoded chunk and returns false to stop
/// reading. Taking a closure rather than a `Channel` is what lets the sandbox
/// suite drive a genuine shell through the same path the app uses.
pub fn open_with<F>(repo_path: &str, cols: u16, rows: u16, sink: F) -> Result<String, AppError>
where
    F: Fn(String) -> bool + Send + 'static,
{
    let repo = PathBuf::from(repo_path);
    if !repo.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} no longer exists on disk",
            repo_path
        )));
    }
    let (shim, real_git) = install_shim()?;

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(2),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| AppError::Command(format!("Could not open a terminal: {}", e)))?;

    let mut cmd = CommandBuilder::new(default_shell());
    cmd.cwd(&repo);
    // The same variables the app strips for its own git calls. Starting a shell
    // that inherited a stale GIT_DIR — or a push-lock bypass — from however the
    // app was launched would be a surprise nobody asked for.
    for key in crate::git_exec::inherited_git_env_to_strip() {
        cmd.env_remove(key);
    }
    cmd.env_remove("GIT_CONFIG_NOSYSTEM");
    cmd.env_remove("GIT_CONFIG_SYSTEM");
    cmd.env_remove("GIT_CONFIG_GLOBAL");
    cmd.env("TERM", "xterm-256color");
    cmd.env("GITSWITCH_TERMINAL", "1");
    // The git the guard hands off to. The sh shim has it baked in; the
    // Windows guard reads it from here.
    cmd.env("GITSWITCH_REAL_GIT", real_git.as_os_str());
    let path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut dirs = vec![shim.clone()];
            dirs.extend(std::env::split_paths(&existing));
            std::env::join_paths(dirs)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| existing.to_string_lossy().to_string())
        }
        None => shim.to_string_lossy().to_string(),
    };
    cmd.env("PATH", path);

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| AppError::Command(format!("Could not start a shell: {}", e)))?;
    // Holding the slave open would keep the pty alive after the shell exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| AppError::Command(format!("Could not read from the terminal: {}", e)))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| AppError::Command(format!("Could not write to the terminal: {}", e)))?;
    let killer = child.clone_killer();
    let pid = child.process_id();

    let id = uuid::Uuid::new_v4().to_string();
    let done_id = id.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 8192];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let (text, rest) = split_utf8(&carry);
                    carry = rest;
                    if !text.is_empty() && !sink(text) {
                        break;
                    }
                }
            }
        }
        // The shell exited on its own (someone typed `exit`); drop the session
        // so the next open starts a fresh one rather than writing into a corpse.
        if let Ok(mut map) = sessions().lock() {
            map.remove(&done_id);
        }
        sink("\r\n\u{1b}[2m[the shell exited]\u{1b}[0m\r\n".to_string());
    });

    sessions()
        .lock()
        .map_err(|_| AppError::Command("The terminal registry is unavailable".into()))?
        .insert(
            id.clone(),
            Session {
                master: pair.master,
                writer,
                killer,
                pid,
            },
        );

    // Wait for the shell itself. On unix the reader's EOF already says it
    // exited; on Windows ConPTY sends no EOF until the master is closed, so
    // without this a shell that had exited stayed "open" forever. Waiting also
    // reaps the process, so a closed terminal never leaves a zombie behind.
    let wait_id = id.clone();
    let mut child = child;
    std::thread::spawn(move || {
        let _ = child.wait();
        if let Ok(mut map) = sessions().lock() {
            map.remove(&wait_id);
        }
    });
    Ok(id)
}

/// The shell's process id, while the session is open.
#[allow(dead_code)] // the leak test below
fn session_pid(id: &str) -> Option<u32> {
    sessions().lock().ok().and_then(|m| m.get(id).and_then(|s| s.pid))
}

pub fn write(id: &str, data: &str) -> Result<(), AppError> {
    let mut map = sessions()
        .lock()
        .map_err(|_| AppError::Command("The terminal registry is unavailable".into()))?;
    let session = map
        .get_mut(id)
        .ok_or_else(|| AppError::NotFound("That terminal is no longer open".into()))?;
    session
        .writer
        .write_all(data.as_bytes())
        .and_then(|_| session.writer.flush())
        .map_err(|e| AppError::Command(format!("Could not reach the shell: {}", e)))
}

pub fn resize(id: &str, cols: u16, rows: u16) -> Result<(), AppError> {
    let map = sessions()
        .lock()
        .map_err(|_| AppError::Command("The terminal registry is unavailable".into()))?;
    let session = map
        .get(id)
        .ok_or_else(|| AppError::NotFound("That terminal is no longer open".into()))?;
    session
        .master
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| AppError::Command(format!("Could not resize the terminal: {}", e)))
}

pub fn close(id: &str) {
    if let Ok(mut map) = sessions().lock() {
        map.remove(id); // Drop kills the shell.
    }
}

/// Every session, killed. Called when the window goes away.
pub fn close_all() {
    if let Ok(mut map) = sessions().lock() {
        map.clear();
    }
}

/// Open a shell, run one line in it, and return everything it printed.
///
/// The sandbox suite uses this: it exercises the real pty, the real shell and
/// the real shim, which is the only way to prove the guards actually fire.
#[allow(dead_code)] // driven by the test-only probe, which the app build omits
pub fn run_once(repo_path: &str, script: &str, timeout: Duration) -> Result<String, AppError> {
    let collected = Arc::new(Mutex::new(String::new()));
    let last = Arc::new(Mutex::new(Instant::now()));
    let out = collected.clone();
    let tick = last.clone();
    let id = open_with(repo_path, 200, 50, move |chunk| {
        if let Ok(mut s) = out.lock() {
            s.push_str(&chunk);
        }
        if let Ok(mut t) = tick.lock() {
            *t = Instant::now();
        }
        true
    })?;

    // An interactive shell sources its startup files before it reads anything.
    // Typing into it first works — the pty buffers — but its own errors and
    // first prompt then arrive interleaved with the command's output, which
    // makes an assertion about that output a race. Waiting for the shell to
    // settle first is what makes this deterministic.
    let settle = |quiet: Duration, limit: Duration| {
        let start = Instant::now();
        loop {
            let idle = last.lock().map(|t| t.elapsed()).unwrap_or(quiet);
            if idle >= quiet || start.elapsed() >= limit || !is_open(&id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    settle(Duration::from_millis(350), Duration::from_secs(5));
    if let Ok(mut s) = collected.lock() {
        s.clear(); // the banner and the first prompt are not the answer
    }

    write(&id, script)?;
    write(&id, "\n")?;
    settle(Duration::from_millis(600), timeout);

    let _ = write(&id, "exit\n");
    std::thread::sleep(Duration::from_millis(120));
    close(&id);
    Ok(collected.lock().map(|s| s.clone()).unwrap_or_default())
}

/// Open a shell at one size, run a line, resize it, run the line again, and
/// return what each run printed — the proof that a resize reaches the shell
/// (`stty size` is the line the suite uses). Sizes are (cols, rows).
#[allow(dead_code)] // driven by the test-only probe, which the app build omits
pub fn run_resized(repo_path: &str, script: &str, first: (u16, u16), second: (u16, u16), timeout: Duration) -> Result<(String, String), AppError> {
    let collected = Arc::new(Mutex::new(String::new()));
    let last = Arc::new(Mutex::new(Instant::now()));
    let out = collected.clone();
    let tick = last.clone();
    let id = open_with(repo_path, first.0, first.1, move |chunk| {
        if let Ok(mut s) = out.lock() {
            s.push_str(&chunk);
        }
        if let Ok(mut t) = tick.lock() {
            *t = Instant::now();
        }
        true
    })?;
    let settle = |quiet: Duration, limit: Duration| {
        let start = Instant::now();
        loop {
            let idle = last.lock().map(|t| t.elapsed()).unwrap_or(quiet);
            if idle >= quiet || start.elapsed() >= limit || !is_open(&id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    let take = || collected.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
    // Quiet is measured from the moment something was asked: without this a
    // shell that had already been silent for a while counts as settled before
    // it has answered at all.
    let asked = || {
        if let Ok(mut t) = last.lock() {
            *t = Instant::now();
        }
    };
    settle(Duration::from_millis(350), Duration::from_secs(5));
    let _ = take(); // the banner and the first prompt are not the answer

    write(&id, script)?;
    write(&id, "\n")?;
    asked();
    settle(Duration::from_millis(600), timeout);
    let before = take();

    resize(&id, second.0, second.1)?;
    // The shell redraws its prompt when it is told its window changed.
    asked();
    settle(Duration::from_millis(350), Duration::from_secs(5));
    let _ = take();
    write(&id, script)?;
    write(&id, "\n")?;
    asked();
    settle(Duration::from_millis(600), timeout);
    let after = take();

    let _ = write(&id, "exit\n");
    std::thread::sleep(Duration::from_millis(120));
    close(&id);
    Ok((before, after))
}

#[allow(dead_code)] // used by run_once above
pub fn is_open(id: &str) -> bool {
    sessions().lock().map(|m| m.contains_key(id)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closing a terminal must end its shell — and reap it, or a zombie would
    /// still answer `kill -0`. This used to be a `pgrep` count in console.sh,
    /// which compared before and after and could not tell a leak from noise.
    #[cfg(unix)]
    #[test]
    fn closing_a_terminal_leaves_no_shell_behind() {
        let dir = std::env::temp_dir();
        let id = match open_with(&dir.to_string_lossy(), 80, 24, |_| true) {
            Ok(id) => id,
            Err(_) => return, // no shell on this machine (a bare container): nothing to prove
        };
        let pid = session_pid(&id).expect("the session knows its shell's pid") as i32;
        // SAFETY: signal 0 only checks that the process exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the shell is running");
        close(&id);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            // ESRCH once it is gone AND reaped; a zombie still answers 0.
            if unsafe { libc::kill(pid, 0) } != 0 {
                assert!(!is_open(&id));
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the shell (pid {}) outlived its terminal", pid);
    }

    #[test]
    fn a_character_split_across_two_reads_is_carried_not_mangled() {
        // "é" is two bytes; a pty read can end between them.
        let whole = "né".as_bytes();
        let (text, carry) = split_utf8(&whole[..whole.len() - 1]);
        assert_eq!(text, "n");
        assert_eq!(carry, vec![whole[1]]);
        // Feeding the carry plus the rest produces the whole character.
        let mut next = carry;
        next.push(whole[2]);
        let (text, carry) = split_utf8(&next);
        assert_eq!(text, "é");
        assert!(carry.is_empty());
    }

    #[test]
    fn genuinely_invalid_bytes_do_not_stall_the_stream() {
        // A lone 0xFF can never begin a character; carrying it would mean the
        // terminal never printed anything again.
        let (text, carry) = split_utf8(&[b'a', 0xFF, b'b']);
        assert!(text.starts_with('a'), "{}", text);
        assert_eq!(carry, vec![b'b']);
    }

    #[test]
    fn plain_text_passes_straight_through() {
        let (text, carry) = split_utf8(b"on branch main\r\n");
        assert_eq!(text, "on branch main\r\n");
        assert!(carry.is_empty());
    }

    #[test]
    fn the_shim_guards_each_thing_the_app_refuses_and_offers_no_way_around_a_lock() {
        let s = shim_script("/usr/bin/git");
        assert!(s.contains("GIT_CONFIG_NOSYSTEM"), "guards the documented bypass");
        assert!(s.contains("GIT_CONFIG_SYSTEM"));
        assert!(s.contains("--no-verify"));
        assert!(s.contains("--force-with-lease"));
        assert!(s.ends_with("\n"));
        assert!(s.contains("exec \"/usr/bin/git\" \"$@\""), "{}", s);
        // `-c key=value` must not be read as the subcommand.
        assert!(s.contains("-c|-C|--git-dir"));
        // It must never tell somebody how to switch a lock off.
        assert!(!s.contains("--uninstall"), "never prints a way to disable itself");
        assert!(!s.to_lowercase().contains("export git_config_nosystem=1"));
    }

    #[test]
    fn the_shim_never_resolves_to_itself() {
        // Given a PATH whose first entry IS the shim directory, the real git
        // must be found further along, or an infinite exec loop is one open away.
        let dir = std::env::temp_dir().join("gitswitch-shim-resolve-test");
        let found = real_git(&dir);
        assert_ne!(found, dir.join("git"));
    }
}
