//! One hardened way to run git, used by every Changes-page operation.
//!
//! The existing runners in `sparse.rs` / `git_history.rs` stay as they are —
//! this exists because the Changes page needs four things none of them offer:
//! stdin (commit messages and NUL pathspecs never become arguments), per-command
//! `-c` overrides, timeouts (a GUI must not hang forever on an unreachable
//! remote), and the ability to inspect a non-zero exit instead of turning it
//! straight into an error string.

use crate::error::AppError;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Network operations: fetch, push, pull, submodule update.
pub const NET_TIMEOUT: Duration = Duration::from_secs(180);
/// Local mutations: add, commit, restore, stash, switch.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Reads: status, config, rev-parse, diff.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a timed-out command gets to leave on its own after SIGTERM —
/// enough for git to remove its `index.lock` and for git-lfs to finish the
/// object it is writing — before SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(5);
/// Git LFS transfers: a repository's large files can be many gigabytes, and a
/// download that is still making progress must not be killed for taking longer
/// than an ordinary fetch. Progress is reported separately, so a genuinely
/// stalled transfer is visible rather than hidden behind this ceiling.
pub const LFS_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

pub struct GitOutput {
    pub code: i32,
    /// Raw bytes: porcelain `-z` output is not line-oriented and must not be
    /// lossily decoded before it has been framed on NUL boundaries.
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Lossy UTF-8 with trailing whitespace trimmed — the convention the other
    /// runners already use for human-readable output.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_string()
    }

    /// Trimmed of all surrounding whitespace, for single-value reads like
    /// `rev-parse HEAD`.
    pub fn trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

/// The environment every git child gets. Each entry exists for a failure mode a
/// GUI app hits and a terminal does not; `None` means "remove this variable".
pub fn hardened_env() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        // No terminal exists, so a credential prompt would hang forever.
        ("GIT_TERMINAL_PROMPT", Some("0")),
        // Same for an SSH passphrase dialog (OpenSSH 8.4+).
        ("SSH_ASKPASS_REQUIRE", Some("never")),
        // An inherited askpass helper would reintroduce the hang.
        ("GIT_ASKPASS", None),
        ("SSH_ASKPASS", None),
        // Nothing may ever wait on an editor.
        ("GIT_EDITOR", Some("true")),
        ("GIT_SEQUENCE_EDITOR", Some("true")),
        // git translates its messages; the advice layer matches English.
        ("LC_ALL", Some("C")),
        ("LANG", Some("C")),
    ]
}

/// Variables an inherited shell environment could use to redirect what the
/// app's own git reads or measures — `npm run tauri dev` from a terminal
/// inherits them all. None is something the app wants; `GIT_CONFIG_*` in
/// particular would let the push lock's measurement be fooled.
pub fn inherited_git_env_to_strip() -> Vec<String> {
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| {
            k.starts_with("GIT_CONFIG_")
                || matches!(
                    k.as_str(),
                    "GIT_EXEC_PATH" | "GIT_DIR" | "GIT_WORK_TREE" | "GIT_COMMON_DIR" | "GIT_INDEX_FILE"
                )
        })
        .collect();
    // The verify suites redirect the system and global scopes on purpose.
    if cfg!(test) {
        names.retain(|k| !matches!(k.as_str(), "GIT_CONFIG_NOSYSTEM" | "GIT_CONFIG_SYSTEM" | "GIT_CONFIG_GLOBAL"));
    }
    names
}

pub struct GitCmd {
    repo: Option<PathBuf>,
    program: Option<PathBuf>,
    cfg: Vec<(String, String)>,
    top: Vec<String>,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    env: Vec<(String, String)>,
    timeout: Duration,
    /// False for a command whose output is a credential (`credential fill`):
    /// the log keeps the command and its exit code, never what it printed.
    log_output: bool,
}

impl GitCmd {
    /// `git -C <repo> …`
    pub fn at(repo: impl AsRef<Path>) -> Self {
        Self {
            repo: Some(repo.as_ref().to_path_buf()),
            program: None,
            cfg: Vec::new(),
            top: Vec::new(),
            args: Vec::new(),
            stdin: None,
            env: Vec::new(),
            timeout: READ_TIMEOUT,
            log_output: true,
        }
    }

    /// Keep this command's output out of the command log altogether.
    pub fn private_output(mut self) -> Self {
        self.log_output = false;
        self
    }

    /// Run a specific git binary instead of the first on PATH — the push lock
    /// measures with the system git whose config it lives in.
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = Some(program.into());
        self
    }

    /// A `-c key=value` for this one command. Never written to any config file.
    pub fn cfg(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.cfg.push((key.into(), value.into()));
        self
    }

    /// A ready-made `key=value` (the clone path builds `core.sshCommand=…`).
    pub fn cfg_raw(mut self, kv: &str) -> Self {
        match kv.split_once('=') {
            Some((k, v)) => self.cfg.push((k.to_string(), v.to_string())),
            None => self.cfg.push((kv.to_string(), String::new())),
        }
        self
    }

    /// Pin every user setting that would change what a rebase, fetch or diff
    /// means, so the sync does the same thing on every machine:
    /// `rebase.updateRefs=true` would move a backup branch along with the
    /// rebase; `submodule.recurse` would detach submodules on every checkout;
    /// `fetch.recurseSubmodules` would fail the superproject fetch on one
    /// unreachable submodule remote; the apply backend has no ancestry-aware
    /// gitlink merge; `rebaseMerges` changes what "my commits on top" means;
    /// colour would corrupt the plumbing output the app parses.
    pub fn pinned(self) -> Self {
        self.cfg("submodule.recurse", "false")
            .cfg("fetch.recurseSubmodules", "false")
            .cfg("rebase.updateRefs", "false")
            .cfg("rebase.backend", "merge")
            .cfg("rebase.rebaseMerges", "false")
            .cfg("color.ui", "never")
    }

    /// A top-level flag that must precede the subcommand (e.g.
    /// `--no-optional-locks`).
    pub fn top_flag(mut self, flag: &str) -> Self {
        self.top.push(flag.to_string());
        self
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Feed bytes on stdin — for `commit --file=-` and
    /// `--pathspec-from-file=-`, so neither a message nor a filename is ever
    /// parsed as an option.
    /// One extra environment variable for this command, applied after the
    /// hardening below so a caller can add what only it needs — today that is
    /// `GIT_LFS_PROGRESS`, which makes a long download observable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn stdin_bytes(mut self, data: Vec<u8>) -> Self {
        self.stdin = Some(data);
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// The subcommand name, for error messages.
    fn subcommand(&self) -> String {
        self.args
            .iter()
            .find(|a| !a.starts_with('-'))
            .cloned()
            .unwrap_or_else(|| "git".to_string())
    }

    /// Run, returning the exit code and both streams. Only a failure to *spawn*
    /// or a timeout is an `Err`; a non-zero exit is for the caller to read.
    pub async fn run(self) -> Result<GitOutput, AppError> {
        let sub = self.subcommand();
        let program = self.program.clone().unwrap_or_else(|| PathBuf::from("git"));
        // The argv is assembled straight onto the child below and never exists
        // as a list, so the Activity tab's copy is built here alongside it.
        let mut argv: Vec<String> = vec![program.to_string_lossy().to_string()];
        if let Some(dir) = &self.repo {
            argv.push("-C".into());
            argv.push(dir.to_string_lossy().to_string());
        }
        argv.extend(self.top.iter().cloned());
        for (k, v) in &self.cfg {
            argv.push("-c".into());
            argv.push(format!("{}={}", k, v));
        }
        argv.extend(self.args.iter().cloned());
        let where_ = self.repo.clone().unwrap_or_else(|| PathBuf::from("."));
        let started = std::time::Instant::now();

        let mut cmd = crate::spawn::async_command(program);
        if let Some(dir) = &self.repo {
            cmd.arg("-C").arg(dir);
        }
        for flag in &self.top {
            cmd.arg(flag);
        }
        for (k, v) in &self.cfg {
            cmd.arg("-c").arg(format!("{}={}", k, v));
        }
        cmd.args(&self.args);

        for (key, value) in hardened_env() {
            match value {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        for key in inherited_git_env_to_strip() {
            cmd.env_remove(key);
        }
        // Caller-supplied variables come last: they are the only ones chosen
        // for this single command, so nothing above may silently drop them.
        for (k, v) in &self.env {
            cmd.env(k, v);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match &self.stdin {
            Some(_) => cmd.stdin(Stdio::piped()),
            None => cmd.stdin(Stdio::null()),
        };
        // If the timeout fires we drop the child future; without this the git
        // process would outlive it.
        cmd.kill_on_drop(true);
        // `git lfs pull` is `git` running `git-lfs` as a CHILD process, so
        // killing git alone leaves git-lfs downloading, re-parented to init.
        // That is how a timed-out download used to report an error and then
        // quietly finish anyway — the next look said everything was present.
        // Its own process group makes the whole tree killable at once.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                // A command that never started is still something that was
                // attempted, and its absence from the log would be a lie.
                crate::cmdlog::record(&where_, argv, -1, started.elapsed(), "", &e.to_string());
                return Err(AppError::Command(format!("Failed to run git: {}", e)));
            }
        };

        if let Some(data) = self.stdin {
            use tokio::io::AsyncWriteExt;
            if let Some(mut pipe) = child.stdin.take() {
                pipe.write_all(&data)
                    .await
                    .map_err(|e| AppError::Command(format!("Failed to send input to git: {}", e)))?;
                // Dropping the handle closes the pipe, which git waits for.
                drop(pipe);
            }
        }

        // On Windows the child's descendants are reached through a Job Object;
        // on unix through the process group set up above.
        #[cfg(windows)]
        let job = crate::spawn::JobObject::adopt(&child);

        // Both pipes are drained while waiting — so a chatty command can never
        // block on a full pipe, and so what it said before a timeout is kept
        // for the log and the error instead of being thrown away.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let drain = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut o = Vec::new();
            let mut e = Vec::new();
            let read_o = async {
                if let Some(mut p) = stdout_pipe {
                    let _ = p.read_to_end(&mut o).await;
                }
            };
            let read_e = async {
                if let Some(mut p) = stderr_pipe {
                    let _ = p.read_to_end(&mut e).await;
                }
            };
            tokio::join!(read_o, read_e);
            (o, e)
        });

        let secs = self.timeout.as_secs();
        let status = match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(result) => result.map_err(|e| AppError::Command(format!("git {} failed: {}", sub, e)))?,
            Err(_) => {
                #[cfg(windows)]
                if let Some(j) = &job {
                    j.terminate();
                }
                terminate_tree(&mut child).await;
                // The pipes close as the tree dies; a short cap in case a
                // process outside both the group and the job holds one open.
                let (o, e) = tokio::time::timeout(Duration::from_secs(2), drain)
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .unwrap_or_default();
                let partial = String::from_utf8_lossy(&e).trim().to_string();
                let why = if sub == "lfs" {
                    format!("killed after {}s without finishing", secs)
                } else {
                    format!("timed out after {}s", secs)
                };
                let logged = if partial.is_empty() { why.clone() } else { format!("{}\n{}", why, partial) };
                crate::cmdlog::record(&where_, argv, -9, started.elapsed(), &String::from_utf8_lossy(&o), &logged);
                return Err(AppError::Command(timeout_message(&sub, self.timeout, &partial)));
            }
        };

        let (stdout, stderr) = drain
            .await
            .map_err(|e| AppError::Command(format!("git {}: reading its output failed: {}", sub, e)))?;
        let code = status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        if self.log_output {
            crate::cmdlog::record(&where_, argv, code, started.elapsed(), &String::from_utf8_lossy(&stdout), &stderr);
        } else {
            crate::cmdlog::record(&where_, argv, code, started.elapsed(), "(output not logged: it is a credential)", &stderr);
        }
        Ok(GitOutput { code, stdout, stderr })
    }

    /// Run and fail on a non-zero exit, with the same error shape the rest of
    /// the app already produces: `git <sub> failed: <stderr>`.
    pub async fn text(self) -> Result<String, AppError> {
        let sub = self.subcommand();
        let out = self.run().await?;
        if !out.ok() {
            return Err(AppError::Command(format!(
                "git {} failed: {}",
                sub,
                if out.stderr.is_empty() {
                    out.text()
                } else {
                    out.stderr.clone()
                }
            )));
        }
        Ok(out.trimmed())
    }

    /// Run and return `None` on a non-zero exit — for probes like
    /// `rev-parse @{upstream}` where "no upstream" is an answer, not an error.
    pub async fn ok_text(self) -> Option<String> {
        match self.run().await {
            Ok(out) if out.ok() => Some(out.trimmed()),
            _ => None,
        }
    }
}

/// Stop a child and everything it started. A polite SIGTERM to the process
/// group first, a short wait for it to clean up, then SIGKILL for whatever
/// ignored it. SIGKILL straight away was how a stopped command left an
/// `index.lock` behind — and the next command then reported "another git
/// process is holding a lock" about a process the app itself had killed.
/// On Windows `kill()` is `TerminateProcess` on git alone; the Job Object
/// taken out at spawn has already ended its descendants.
async fn terminate_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let group = -(pid as i32);
        // SAFETY: a signal to a process group this process created.
        unsafe {
            libc::kill(group, libc::SIGTERM);
        }
        let left_on_its_own = tokio::time::timeout(KILL_GRACE, child.wait()).await.is_ok();
        // The leader may have gone while a grandchild lingers: insist on the
        // whole group either way. ESRCH (nobody left) is the good outcome.
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        if left_on_its_own {
            return;
        }
    }
    let _ = child.kill().await;
}

/// Why a command was stopped, in the words of the class it belongs to: a
/// command given a network timeout was most likely waiting on the remote; a
/// local one on a lock or on the size of the repository. The old wording
/// blamed the remote for a `git status` that never touched one.
fn timeout_message(sub: &str, timeout: Duration, partial_stderr: &str) -> String {
    let secs = timeout.as_secs();
    if sub == "lfs" {
        return format!(
            "The large-file download was stopped after {}s without finishing. Whatever already arrived is kept, so running it again continues rather than starting over.",
            secs
        );
    }
    let likely = if timeout >= NET_TIMEOUT {
        "the remote may be unreachable, or the connection very slow"
    } else {
        "another git process may be holding a lock, or the repository is very large"
    };
    let mut m = format!("git {} was stopped after {}s without finishing — {}.", sub, secs, likely);
    if let Some(last) = partial_stderr.lines().rev().find(|l| !l.trim().is_empty()) {
        m.push_str(&format!(" It had got as far as: {}", last.trim()));
    }
    m
}

/// A git remote name we are willing to pass to git. Rejects anything that could
/// be read as an option or a path.
pub fn validate_remote_name(name: &str) -> Result<(), AppError> {
    // A slash inside the name is legal to git (`git remote add team/gh …`),
    // and refusing it silently left such a remote out of the push guard's
    // rewrite list. Leading, trailing or doubled slashes are still not names.
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("//")
        || name.contains(['\\', ' ', '\t', '\n', ':', '?', '*', '[', '~', '^'])
        || name.contains("..");
    if bad {
        return Err(AppError::Command(format!(
            "'{}' is not a valid remote name",
            name
        )));
    }
    Ok(())
}

/// Branch names go through `git check-ref-format`'s rules, implemented here
/// rather than shelled out: `git check-ref-format --branch` does not accept
/// `--` before its argument, so a branch named `-x` would be read as an option.
pub fn validate_branch_name(name: &str) -> Result<(), AppError> {
    let invalid = name.is_empty()
        || name.starts_with('-')
        || name.starts_with('.')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name.contains(['~', '^', ':', '?', '*', '[', '\\', ' ', '\t', '\n'])
        || name.chars().any(|c| c.is_control());
    if invalid {
        return Err(AppError::Command(format!(
            "'{}' is not a valid branch name",
            name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardened_env_covers_every_hang_source() {
        let env = hardened_env();
        let get = |k: &str| env.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        assert_eq!(get("GIT_TERMINAL_PROMPT"), Some(Some("0")));
        assert_eq!(get("SSH_ASKPASS_REQUIRE"), Some(Some("never")));
        assert_eq!(get("GIT_EDITOR"), Some(Some("true")));
        assert_eq!(get("GIT_SEQUENCE_EDITOR"), Some(Some("true")));
        // Removed, not set.
        assert_eq!(get("GIT_ASKPASS"), Some(None));
        assert_eq!(get("SSH_ASKPASS"), Some(None));
        // The advice layer matches English git output.
        assert_eq!(get("LC_ALL"), Some(Some("C")));
    }

    #[test]
    fn remote_names_that_could_be_options_or_paths_are_rejected() {
        assert!(validate_remote_name("origin").is_ok());
        assert!(validate_remote_name("up-stream2").is_ok());
        assert!(validate_remote_name("team/gh").is_ok(), "git itself accepts a slash in a remote name");
        for bad in ["", "--upload-pack=x", "-o", "/a", "a/", "a//b", "a b", "a..b", "a:b"] {
            assert!(
                validate_remote_name(bad).is_err(),
                "{} should be rejected",
                bad
            );
        }
    }

    #[test]
    fn branch_names_follow_check_ref_format() {
        for good in ["main", "feat/ok", "release-1.2", "user/fix_2", "ünïcode"] {
            assert!(validate_branch_name(good).is_ok(), "{} should pass", good);
        }
        for bad in [
            "", "-x", "a b", "a..b", "a@{b}", "x.lock", "a~1", "a^", "a:b", "a\\b", ".hidden",
            "trailing/", "a//b", "ends.",
        ] {
            assert!(validate_branch_name(bad).is_err(), "{} should fail", bad);
        }
    }

    /// The failure this guards against, in the shape it really has: `git lfs
    /// pull` is `git` running `git-lfs` as a child, so killing only the process
    /// we spawned leaves the download running. It then finishes in the
    /// background — after the app has already reported a timeout — and the next
    /// look says everything is present. A `!shell` alias reproduces that tree
    /// with no network and no git-lfs needed.
    #[tokio::test]
    async fn a_timed_out_command_takes_its_grandchildren_with_it() {
        let dir = std::env::temp_dir().join(format!("gitswitch-pgroup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let init = crate::spawn::command("git").arg("init").arg("-q").arg(&dir).output();
        match init {
            Ok(o) if o.status.success() => {}
            // No usable git on this machine: there is nothing to prove here.
            _ => return,
        }
        let marker = dir.join("the-grandchild-kept-working");
        let alias = format!(
            "!sh -c 'sh -c \"sleep 3; touch {}\" & wait'",
            marker.display()
        );

        let out = GitCmd::at(&dir)
            .cfg("alias.slowspawn", alias)
            .args(["slowspawn"])
            .timeout(Duration::from_millis(400))
            .run()
            .await;
        assert!(out.is_err(), "the command should have timed out");

        // Long enough for the grandchild's sleep to have finished, had it lived.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !marker.exists(),
            "a grandchild outlived the timeout and went on working"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
