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

/// Network operations: fetch, push, pull, submodule update. For a transfer
/// run with `--progress` this is the longest SILENCE allowed (`stall`), so a
/// big repository on an ordinary connection is never cut short for taking
/// long; for the rest it is the total.
pub const NET_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a transfer run with `--progress` may say nothing before it is
/// stopped.
pub fn net_stall() -> Duration {
    // The suites prove the stop with a stuck server; waiting three minutes
    // for it would prove the same thing slowly.
    #[cfg(test)]
    if let Some(secs) = std::env::var("GITSWITCH_STALL_SECS").ok().and_then(|v| v.parse::<u64>().ok()) {
        return Duration::from_secs(secs);
    }
    NET_TIMEOUT
}
/// git's own housekeeping after a fetch (see `housekeeping`): a repack of a
/// big repository can take minutes, and nothing waits on it but the person.
pub const HOUSEKEEPING_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// git's own housekeeping, run after a fetch that held it back (`for_fetch`):
/// `maintenance run --auto` (or `gc --auto` for a git that predates it). It
/// is quick when there is nothing to do and detaches where git can detach
/// it; where it cannot (Windows, `gc.autoDetach=false`) it runs here, on a
/// generous clock and no silence clock. Best effort: it never changes the
/// fetch's outcome.
pub async fn housekeeping(repo: impl AsRef<Path>) {
    let repo = repo.as_ref();
    let maintained = GitCmd::at(repo).args(["maintenance", "run", "--auto", "--quiet"]).timeout(HOUSEKEEPING_TIMEOUT).run().await;
    if matches!(&maintained, Ok(o) if o.ok()) {
        return;
    }
    let _ = GitCmd::at(repo).args(["gc", "--auto", "--quiet"]).timeout(HOUSEKEEPING_TIMEOUT).run().await;
}
/// Local mutations: add, commit, restore, stash, switch.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Reads: status, config, rev-parse, diff.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a timed-out command gets to leave on its own after SIGTERM —
/// enough for git to remove its `index.lock` and for git-lfs to finish the
/// object it is writing — before SIGKILL. Unix only: Windows ends the whole
/// job object at once (spawn.rs), so there is no grace period there.
#[cfg(unix)]
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

/// A `-c` that every command of the `log` family gets, whoever runs it: with
/// `log.showSignature` set in someone's config, git writes each commit's
/// signature check into the very output the app parses (`stash list` and
/// `reflog` are `log` underneath). An explicit `--show-signature` still wins.
pub fn parse_safe_cfg(subcommand: &str) -> Option<&'static str> {
    matches!(subcommand, "log" | "show" | "whatchanged" | "reflog" | "stash").then_some("log.showSignature=false")
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
    /// Set for a transfer that reports progress: the command is stopped only
    /// when it has said nothing for this long, never for taking long.
    stall: Option<Duration>,
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
            stall: None,
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

    /// Stop the command only when it has reported nothing for `d` — for a
    /// fetch or push run with `--progress`, whose counters keep coming while
    /// bytes move. Replaces the total ceiling for this command.
    pub fn stall(mut self, d: Duration) -> Self {
        self.stall = Some(d);
        self
    }

    /// A fetch the app runs (the caller passes `--progress`): stopped only for
    /// silence; what arrives kept as a pack (`fetch.unpackLimit=1`), because
    /// git says nothing while it unpacks a small fetch however big it is; and
    /// git's own housekeeping held back — it would run INSIDE the fetch, and
    /// where it cannot detach (Windows, `gc.autoDetach=false`) it repacks
    /// without a word, which the silence clock would take for a dead
    /// connection. Run `housekeeping` afterwards.
    pub fn for_fetch(self) -> Self {
        self.cfg("fetch.unpackLimit", "1").cfg("maintenance.auto", "false").cfg("gc.auto", "0").stall(net_stall())
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
    pub async fn run(mut self) -> Result<GitOutput, AppError> {
        let sub = self.subcommand();
        if let Some(kv) = parse_safe_cfg(&sub) {
            self = self.cfg_raw(kv);
        }
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

        // Its input is written by a task of its own, alongside the drain
        // below. A command that answers while it reads (`cat-file --batch`,
        // `check-attr --stdin`) fills its output pipe long before it has
        // taken all of its input; writing everything first would leave it
        // blocked on a full pipe nobody reads, and us blocked on a pipe it no
        // longer reads from — for good, since that write sat outside the
        // timeout.
        let writer = match (self.stdin, child.stdin.take()) {
            (Some(data), Some(mut pipe)) => Some(tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                // A child that exits without reading the rest closes the pipe
                // on us: its exit status tells that story, not this error.
                let _ = pipe.write_all(&data).await;
                let _ = pipe.shutdown().await;
                // Dropping the handle closes the pipe, which git waits for.
                drop(pipe);
            })),
            _ => None,
        };

        // On Windows the child's descendants are reached through a Job Object;
        // on unix through the process group set up above.
        #[cfg(windows)]
        let job = crate::spawn::JobObject::adopt(&child);

        // Both pipes are drained while waiting — so a chatty command can never
        // block on a full pipe, and so what it said before a timeout is kept
        // for the log and the error instead of being thrown away.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        // Read in pieces, not to the end, so the stall clock can see each
        // one arrive: the last moment either pipe said anything.
        let activity = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        let act = activity.clone();
        let drain = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut o = Vec::new();
            let mut e = Vec::new();
            let act_o = act.clone();
            let read_o = async {
                if let Some(mut p) = stdout_pipe {
                    let mut buf = [0u8; 8192];
                    loop {
                        match p.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                o.extend_from_slice(&buf[..n]);
                                if let Ok(mut t) = act_o.lock() {
                                    *t = std::time::Instant::now();
                                }
                            }
                        }
                    }
                }
            };
            let read_e = async {
                if let Some(mut p) = stderr_pipe {
                    let mut buf = [0u8; 8192];
                    loop {
                        match p.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                e.extend_from_slice(&buf[..n]);
                                if let Ok(mut t) = act.lock() {
                                    *t = std::time::Instant::now();
                                }
                            }
                        }
                    }
                }
            };
            tokio::join!(read_o, read_e);
            (o, e)
        });
        let secs = self.timeout.as_secs();
        // With a stall window the command is stopped only after that long of
        // silence; otherwise after the total ceiling.
        let waited: Result<std::io::Result<std::process::ExitStatus>, ()> = match self.stall {
            None => tokio::time::timeout(self.timeout, child.wait()).await.map_err(|_| ()),
            Some(stall) => loop {
                tokio::select! {
                    s = child.wait() => break Ok(s),
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        let quiet = activity.lock().map(|t| t.elapsed()).unwrap_or_default();
                        if quiet > stall {
                            break Err(());
                        }
                    }
                }
            },
        };
        let status = match waited {
            Ok(result) => result.map_err(|e| AppError::Command(format!("git {} failed: {}", sub, e)))?,
            Err(_) => {
                #[cfg(windows)]
                if let Some(j) = &job {
                    j.terminate();
                }
                terminate_tree(&mut child).await;
                if let Some(w) = &writer {
                    w.abort();
                }
                // The pipes close as the tree dies; a short cap in case a
                // process outside both the group and the job holds one open.
                let (o, e) = tokio::time::timeout(Duration::from_secs(2), drain)
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .unwrap_or_default();
                let partial = strip_progress(&String::from_utf8_lossy(&e)).trim().to_string();
                let why = if let Some(stall) = self.stall {
                    format!("stopped: no progress for {}s", stall.as_secs())
                } else if sub == "lfs" {
                    format!("killed after {}s without finishing", secs)
                } else {
                    format!("timed out after {}s", secs)
                };
                let logged = if partial.is_empty() { why.clone() } else { format!("{}\n{}", why, partial) };
                crate::cmdlog::record(&where_, argv, -9, started.elapsed(), &String::from_utf8_lossy(&o), &logged);
                if let Some(stall) = self.stall {
                    let far = partial.lines().rev().find(|l| !l.trim().is_empty()).map(|l| format!(" It had got as far as: {}", l.trim())).unwrap_or_default();
                    return Err(AppError::Command(format!(
                        "git {} made no progress for {}s and was stopped — the remote may be unreachable, or the connection very slow.{}",
                        sub,
                        stall.as_secs(),
                        far
                    )));
                }
                return Err(AppError::Command(timeout_message(&sub, self.timeout, &partial)));
            }
        };

        let (stdout, stderr) = drain
            .await
            .map_err(|e| AppError::Command(format!("git {}: reading its output failed: {}", sub, e)))?;
        let code = status.code().unwrap_or(-1);
        let stderr = strip_progress(&String::from_utf8_lossy(&stderr)).trim().to_string();
        if self.log_output {
            crate::cmdlog::record(&where_, argv, code, started.elapsed(), &String::from_utf8_lossy(&stdout), &stderr);
        } else {
            crate::cmdlog::record(&where_, argv, code, started.elapsed(), "(output not logged)", &stderr);
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
pub(crate) async fn terminate_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let group = -(pid as i32);
        // SAFETY: a signal to a process group this process created.
        unsafe {
            libc::kill(group, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + KILL_GRACE;
        let left_on_its_own = tokio::time::timeout(KILL_GRACE, child.wait()).await.is_ok();
        // The leader going is not the group going: an inner `git clone` is
        // still removing what it had written when its parent exits, and a
        // SIGKILL at that moment leaves a half-deleted `.git/modules/<name>`
        // that makes the whole repository unreadable. The grace is the
        // group's. ESRCH (nobody left) is the good outcome.
        while std::time::Instant::now() < deadline && unsafe { libc::kill(group, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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
/// What a `--progress` transfer prints is a stream of `\r`-rewritten
/// counters. The last state of each line is kept and the counters dropped,
/// so an error message and the Activity tab read as they did before
/// progress was asked for; everything else git said stays.
pub fn strip_progress(stderr: &str) -> String {
    const NOISE: &[&str] = &[
        "Enumerating objects",
        "Counting objects",
        "Compressing objects",
        "Receiving objects",
        "Resolving deltas",
        "Writing objects",
        "Unpacking objects",
        "Updating files",
        "Checking out files",
        "Filtering content",
        "Total ",
    ];
    stderr
        .split('\n')
        .filter_map(|line| {
            let last = line.rsplit('\r').find(|s| !s.trim().is_empty()).unwrap_or("");
            let t = last.trim_start().trim_start_matches("remote: ").trim();
            if NOISE.iter().any(|n| t.starts_with(n)) {
                None
            } else {
                Some(last)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
    #[test]
    fn a_fetch_holds_back_housekeeping_and_keeps_what_arrives_as_a_pack() {
        let c = super::GitCmd::at(".").for_fetch();
        for kv in [("fetch.unpackLimit", "1"), ("maintenance.auto", "false"), ("gc.auto", "0")] {
            assert!(c.cfg.iter().any(|(k, v)| k == kv.0 && v == kv.1), "{:?} missing from {:?}", kv, c.cfg);
        }
        assert_eq!(c.stall, Some(super::net_stall()));
    }

    #[test]
    fn progress_counters_are_dropped_and_the_rest_kept() {
        let raw = "Receiving objects:  10% (1/10)\rReceiving objects: 100% (10/10), 1.00 MiB | 2.00 MiB/s, done.\nremote: Enumerating objects: 13, done.        \nremote: Total 13 (delta 0), reused 0 (delta 0)        \nremote: error: GH006: Protected branch update failed\nFrom https://example.test/r\n * branch            main -> FETCH_HEAD\nfatal: refusing";
        let got = super::strip_progress(raw);
        assert_eq!(got, "remote: error: GH006: Protected branch update failed\nFrom https://example.test/r\n * branch            main -> FETCH_HEAD\nfatal: refusing");
        assert_eq!(super::strip_progress("plain error"), "plain error");
        assert_eq!(super::strip_progress(""), "");
    }

    /// A transfer that keeps reporting is never cut short for taking longer
    /// than the ceiling; one that goes silent is stopped after the window.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stall_window_measures_silence_not_duration() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gitswitch-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let chatty = dir.join("chatty.sh");
        std::fs::write(&chatty, "#!/bin/sh\ni=0\nwhile [ $i -lt 7 ]; do printf 'Receiving objects: %s%%\\r' $i 1>&2; sleep 0.3; i=$((i+1)); done\nprintf 'done\\n'\n").unwrap();
        std::fs::set_permissions(&chatty, std::fs::Permissions::from_mode(0o755)).unwrap();
        let silent = dir.join("silent.sh");
        std::fs::write(&silent, "#!/bin/sh\nprintf 'Cloning into x...\\n' 1>&2\nsleep 6\n").unwrap();
        std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        // A window well above the script's own cadence, so a busy machine
        // cannot turn a reporting transfer into a silent one.
        let out = super::GitCmd::at(&dir).with_program(&chatty).args(["fetch"]).timeout(std::time::Duration::from_millis(500)).stall(std::time::Duration::from_secs(2)).run().await;
        let out = out.expect("a reporting transfer is not stopped");
        assert!(out.ok() && out.text().trim() == "done", "{:?}", out.stdout);
        assert!(started.elapsed() >= std::time::Duration::from_millis(1800), "it ran to the end: {:?}", started.elapsed());
        // The counters never reach the stored output.
        assert_eq!(out.stderr, "");

        let started = std::time::Instant::now();
        let err = super::GitCmd::at(&dir).with_program(&silent).args(["fetch"]).stall(std::time::Duration::from_secs(1)).run().await.err().expect("silence is stopped");
        let msg = err.to_string();
        assert!(msg.contains("made no progress for 1s and was stopped"), "{}", msg);
        assert!(msg.contains("It had got as far as: Cloning into x..."), "{}", msg);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "stopped after the window, not after the sleep: {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
    /// A command that answers while it reads — here an alias that echoes its
    /// input — given more than a pipe holds in both directions. Written
    /// first and read afterwards, neither side could ever move again.
    #[cfg(unix)]
    #[tokio::test]
    async fn input_is_written_while_output_is_read() {
        let dir = std::env::temp_dir().join(format!("gitswitch-stdin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        match crate::spawn::command("git").arg("init").arg("-q").arg(&dir).output() {
            Ok(o) if o.status.success() => {}
            _ => return,
        }
        let big = vec![b'x'; 8 * 1024 * 1024];
        let out = GitCmd::at(&dir)
            .cfg("alias.echo", "!cat")
            .args(["echo"])
            .stdin_bytes(big.clone())
            .private_output()
            .timeout(Duration::from_secs(60))
            .run()
            .await
            .expect("it must finish, not wait on a full pipe");
        assert!(out.ok(), "{}", out.stderr);
        assert_eq!(out.stdout.len(), big.len());
        // A child that stops reading early is an exit status, not an error from the write.
        let out = GitCmd::at(&dir)
            .cfg("alias.deaf", "!true")
            .args(["deaf"])
            .stdin_bytes(big)
            .private_output()
            .timeout(Duration::from_secs(60))
            .run()
            .await
            .expect("an unread input is not a failure to run");
        assert!(out.ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_family_commands_never_print_signature_checks() {
        for sub in ["log", "show", "stash", "reflog", "whatchanged"] {
            assert_eq!(parse_safe_cfg(sub), Some("log.showSignature=false"), "{}", sub);
        }
        for sub in ["status", "rebase", "fetch", "cat-file", "commit"] {
            assert_eq!(parse_safe_cfg(sub), None, "{}", sub);
        }
    }

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
