//! One hardened way to run git, used by every git process the app starts.
//!
//! It offers stdin (commit messages and NUL pathspecs never become
//! arguments), per-command `-c` overrides, timeouts (a GUI must not hang
//! forever on an unreachable remote), and the ability to inspect a non-zero
//! exit instead of turning it straight into an error string. The smaller
//! runners other modules keep (`git_history::git`, `repo_scan::git_text`, …)
//! are thin wrappers over `GitCmd`, blocking ones through `run_blocking`, so
//! the environment, the clock and the command log are the same everywhere.
//! A test below fails when a git process is started any other way.

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
    housekeeping_as(repo.as_ref(), false).await
}

/// `housekeeping` after the History fetch: the same commands, run as the
/// passive History fetch is (`GitCmd::history_fetch`), so `pre-auto-gc` and
/// the other hooks of a repository nobody opened on purpose never run.
pub async fn history_housekeeping(repo: impl AsRef<Path>) {
    housekeeping_as(repo.as_ref(), true).await
}

async fn housekeeping_as(repo: &Path, passive: bool) {
    let cmd = || if passive { GitCmd::at(repo).history_fetch() } else { GitCmd::at(repo) };
    let maintained = cmd().args(["maintenance", "run", "--auto", "--quiet"]).timeout(HOUSEKEEPING_TIMEOUT).run().await;
    if matches!(&maintained, Ok(o) if o.ok()) {
        return;
    }
    let _ = cmd().args(["gc", "--auto", "--quiet"]).timeout(HOUSEKEEPING_TIMEOUT).run().await;
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
        // Output always goes to a pipe, where git starts no pager anyway;
        // this makes it certain that a repository's `core.pager` never runs.
        ("GIT_PAGER", Some("cat")),
    ]
}

/// Global pathspec switches git reads from the environment. Removed from every
/// command GitSwitch runs (`GitCmd::prepare`), never from the person's own
/// terminal (`inherited_git_env_to_strip` is that list's).
pub(crate) const PATHSPEC_SWITCHES: [&str; 4] = ["GIT_LITERAL_PATHSPECS", "GIT_GLOB_PATHSPECS", "GIT_NOGLOB_PATHSPECS", "GIT_ICASE_PATHSPECS"];

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
    /// Variables this one command must not inherit, beyond those every
    /// command loses (`env_remove`).
    unset: Vec<String>,
    timeout: Duration,
    /// Set for a transfer that reports progress: the command is stopped only
    /// when it has said nothing for this long, never for taking long.
    stall: Option<Duration>,
    /// False for a command whose output is a credential (`credential fill`):
    /// the log keeps the command and its exit code, never what it printed.
    log_output: bool,
    /// The History fetch (`history_fetch`): a passive command, whatever its
    /// subcommand says.
    history_fetch: bool,
    /// The `-c` pairs `settle` worked out from the repository's own config
    /// (see `neutralising_cfg`, `credential_cfg`).
    neutral: Vec<(String, String)>,
    /// `settle` has run.
    settled: bool,
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
            unset: Vec::new(),
            timeout: READ_TIMEOUT,
            stall: None,
            log_output: true,
            history_fetch: false,
            neutral: Vec::new(),
            settled: false,
        }
    }

    /// `git …` with no `-C`: for a command that names its repository itself
    /// (`--git-dir`) or needs none (`--version`, `config --system`).
    pub fn without_repo() -> Self {
        Self { repo: None, ..Self::at(".") }
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

    /// One variable this command must not inherit even where the hardening
    /// keeps it — the push lock asks where the system scope lives with
    /// `GIT_CONFIG_NOSYSTEM` removed, which the test builds otherwise keep.
    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.unset.push(key.into());
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

    /// The History page's fetch (and its housekeeping): it runs on its own
    /// when "auto fetch" is on, so it is a passive command — no hooks, only
    /// the person's own credential helpers, no askpass — and it is refused
    /// when the repository's own config names a transport program
    /// (`transport_programs`). Every other fetch is the person's explicit
    /// action and keeps the repository's settings.
    pub fn history_fetch(mut self) -> Self {
        self.history_fetch = true;
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

    /// The child this command runs as, set up the way every git the app
    /// starts must be — `-C`, top-level flags, the `-c` pairs (the safety
    /// prelude first, the caller's own next, the `log` family's signature pin
    /// last), the command's own arguments (a passive diff with
    /// `--no-ext-diff --no-textconv` right after its subcommand), the
    /// hardened environment, both outputs piped and its whole process tree
    /// killable — and the same argv as text, for the command log. `run`
    /// spawns it; so does sparse.rs's progress reader, whose clock is its own
    /// (it calls `settled` first, as `run` does).
    pub(crate) fn prepare(&self) -> (tokio::process::Command, Vec<String>) {
        let shape = shape(&self.args, self.history_fetch);
        // What the repository's config says is only known once `settle` has
        // read it; without that a passive diff or status would run with the
        // repository's filters live.
        debug_assert!(self.settled || shape.read == Read::None, "prepare() before settle() on a passive command that needs the repository's config: {:?}", self.args);
        let mut cfg: Vec<String> = self.prelude(&shape).iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        cfg.extend(self.cfg.iter().map(|(k, v)| format!("{}={}", k, v)));
        if let Some(kv) = parse_safe_cfg(&self.subcommand()) {
            cfg.push(kv.to_string());
        }
        let mut args = self.args.clone();
        if let Some(at) = shape.diff_flags_at.filter(|&at| at <= args.len()) {
            args.splice(at..at, ["--no-ext-diff".to_string(), "--no-textconv".to_string()]);
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
        for kv in &cfg {
            argv.push("-c".into());
            argv.push(kv.clone());
        }
        argv.extend(args.iter().cloned());

        let mut cmd = crate::spawn::async_command(program);
        if let Some(dir) = &self.repo {
            cmd.arg("-C").arg(dir);
        }
        for flag in &self.top {
            cmd.arg(flag);
        }
        for kv in &cfg {
            cmd.arg("-c").arg(kv);
        }
        cmd.args(&args);

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
        for key in &self.unset {
            cmd.env_remove(key);
        }
        // Global pathspec switches a shell may export: the app marks its own
        // paths (`:(literal)`, `:/`, `--literal-pathspecs`), and git reads
        // `:(literal)` as part of the name under GIT_LITERAL_PATHSPECS=1, and
        // dies on `--literal-pathspecs` with GIT_GLOB_ or GIT_ICASE_PATHSPECS=1.
        // Always removed, set or not; a caller's own value (`env`) still comes last.
        for key in PATHSPEC_SWITCHES {
            cmd.env_remove(key);
        }
        // A passive read never takes the optional index lock, so it never
        // fights the person's terminal for it and never writes the index.
        if shape.class == Class::Passive {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
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
        (cmd, argv)
    }

    /// Run, returning the exit code and both streams. Only a failure to *spawn*
    /// or a timeout is an `Err`; a non-zero exit is for the caller to read.
    pub async fn run(mut self) -> Result<GitOutput, AppError> {
        // What this command may take from the repository: boxed, because it
        // may run git itself (the config read) and this future is awaited
        // deep inside other commands.
        Box::pin(self.settle()).await?;
        // A write may have changed the repository's config (`remote add`,
        // `lfs install`, `submodule init`, `config`…): whatever was read
        // before it is forgotten once it is over, however it ended.
        let _forget = self.forget_when_done();
        let sub = self.subcommand();
        let (mut cmd, argv) = self.prepare();
        let where_ = self.repo.clone().unwrap_or_else(|| PathBuf::from("."));
        let started = std::time::Instant::now();

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

    /// `run`, for code that is not async: it IS `run` — the same arguments,
    /// environment, clock, process-tree kill and log — on a runtime kept for
    /// this, with the calling thread waiting for the answer. Unlike
    /// `block_on` it works whether or not the caller is itself inside a
    /// runtime (a blocking helper called from an async command).
    pub fn run_blocking(self) -> Result<GitOutput, AppError> {
        let (tx, rx) = std::sync::mpsc::channel();
        blocking_runtime()?.spawn(async move {
            let _ = tx.send(self.run().await);
        });
        // A sender dropped without sending is a run that panicked.
        rx.recv().unwrap_or_else(|_| Err(AppError::Command("Failed to run git: it stopped before it answered".into())))
    }

    /// For a caller that spawns `prepare()`'s child itself (sparse.rs's
    /// progress reader): the prelude settled first, exactly as `run` does.
    pub(crate) async fn settled(mut self) -> Result<Self, AppError> {
        Box::pin(self.settle()).await?;
        Ok(self)
    }

    /// What `run` does once a write is over, for a caller that spawns
    /// `prepare()`'s child itself: the folder's cached reads (and those of
    /// the folders inside it) are forgotten when the guard is dropped, so a
    /// read cached while the command ran (a half-downloaded submodule) does
    /// not outlive it.
    pub(crate) fn forget_when_done(&self) -> ForgetAfterWrite {
        ForgetAfterWrite((shape(&self.args, self.history_fetch).class == Class::Write).then(|| self.repo.clone()))
    }

    /// The `-c` pairs this command gets before the caller's own (see the
    /// "safety prelude" section below for why each one exists).
    fn prelude(&self, shape: &Shape) -> Vec<(String, String)> {
        // Every command. The empty form, never `false`: git 2.34 reads an
        // empty value as unset and 2.36+ as false, while 2.35.1 and older run
        // a hook NAMED `false`.
        let mut p = vec![("core.fsmonitor".to_string(), String::new()), ("protocol.ext.allow".to_string(), "never".to_string())];
        if shape.class == Class::Passive && shape.hooks_pin {
            p.push(("core.hooksPath".to_string(), empty_hooks_dir().to_string()));
        }
        // A repository's own `diff.submodule=diff` makes a patch that touches
        // a submodule run `git diff` INSIDE it, without the flags this runner
        // adds and with that submodule's own diff drivers. `short` is git's
        // default (1.7.11+), and the command scope wins over the local one.
        if shape.class == Class::Passive && shape.diff_flags_at.is_some() {
            p.push(("diff.submodule".to_string(), "short".to_string()));
        }
        p.extend(self.neutral.iter().cloned());
        p
    }

    /// Work out what this command may take from the repository: nothing to
    /// do for a write (only the fixed pins) beyond forgetting the cached
    /// config; for a passive command that can reach a program-naming key,
    /// read the repository's config (cached briefly) and switch those keys
    /// off — or refuse the command when that cannot be done.
    async fn settle(&mut self) -> Result<(), AppError> {
        if self.settled {
            return Ok(());
        }
        self.settled = true;
        let shape = shape(&self.args, self.history_fetch);
        #[cfg(test)]
        test_support::record(self.repo.as_deref(), &self.args, shape.class);
        if shape.class == Class::Write {
            forget_repo_config(self.repo.as_deref());
            return Ok(());
        }
        if shape.read == Read::None {
            return Ok(());
        }
        let lead = self.args[..shape.sub_at.min(self.args.len())].to_vec();
        // A command that only reads objects needs the config only where git
        // could fetch a missing object on its own (a partial clone), and the
        // repository's own config files say whether it can be one. Anything
        // unusual about the folder, and the config is read. (A promisor
        // remote named in the person's own config is not looked for here:
        // that setting is theirs.)
        // Read off the async workers, like the gitlink check: a file read has
        // no clock (only regular files are read, but a slow disk still waits).
        if shape.read == Read::Objects && !shape.recurse && lead.is_empty() {
            let folder = self.repo.clone().unwrap_or_else(|| PathBuf::from("."));
            let ordinary = tokio::task::spawn_blocking(move || find_git_dirs(&folder).is_some_and(|dirs| !may_be_partial(&dirs))).await.unwrap_or(false);
            if ordinary {
                return Ok(());
            }
        }
        let read = repo_config(self.program.clone(), self.repo.clone(), lead.clone()).await;
        let sub = self.subcommand();
        match (shape.read, read) {
            (Read::Neutral | Read::Objects, Ok(entries)) => {
                let mut pairs = if shape.read == Read::Neutral { neutralising_cfg(&entries).map_err(|key| unswitchable(&sub, &key))? } else { Vec::new() };
                let mut partial = partial_clone(&entries);
                if partial {
                    let found = transport_programs(&entries);
                    if !found.is_empty() {
                        return Err(partial_refusal(&sub, &found));
                    }
                }
                if shape.recurse {
                    let only = pathspec_prefixes(&lead, &self.args[shape.sub_at.min(self.args.len())..]);
                    let theirs = submodule_filter_cfg(self.program.clone(), self.repo.clone(), lead, only).await.map_err(|why| {
                        AppError::Command(format!(
                            "GitSwitch did not run git {}: it reads the settings of this repository's submodules first, so that nothing they name runs on its own, and {}",
                            sub, why
                        ))
                    })?;
                    partial |= theirs.partial;
                    for pair in theirs.pairs {
                        if !pairs.contains(&pair) {
                            pairs.push(pair);
                        }
                    }
                }
                // A fetch git starts on its own for a missing object asks only
                // the person's own credential helpers, never an askpass.
                if partial {
                    pairs.extend(credential_cfg(Some(&entries)));
                }
                self.neutral = pairs;
            }
            // git cannot open that folder either: the command fails there the
            // same way, before it could fetch anything.
            (Read::Objects, Err(why)) if why.refused => {}
            (Read::Neutral | Read::Objects, Err(why)) => {
                return Err(AppError::Command(format!(
                    "GitSwitch did not run git {}: it reads this repository's own settings first, so that nothing they name runs on its own, and that read failed: {}",
                    sub, why
                )))
            }
            (Read::Credential, Ok(entries)) => self.neutral = credential_cfg(Some(&entries)),
            // Without the person's own helpers the lookup most likely finds
            // nothing, which is an answer; the repository's helpers never run.
            (Read::Credential, Err(_)) => self.neutral = credential_cfg(None),
            (Read::HistoryFetch, Ok(entries)) => {
                let found = transport_programs(&entries);
                if !found.is_empty() {
                    return Err(fetch_refusal(&found));
                }
                self.neutral = credential_cfg(Some(&entries));
            }
            (Read::HistoryFetch, Err(why)) => return Err(fetch_read_failed(&why)),
            (Read::None, _) => {}
        }
        Ok(())
    }
}

/// A partial clone whose own config names a transport program: a missing
/// object would make git fetch it with that program.
fn partial_refusal(sub: &str, keys: &[String]) -> AppError {
    AppError::Command(format!(
        "GitSwitch did not run git {}: this repository is a partial clone whose own settings name {}, which git would run to fetch a missing object on its own, and a command the app makes on its own never runs a program a repository names. Run it from a terminal, or move the setting into your profile.",
        sub,
        and_list(keys)
    ))
}

/// "a", "a and b", "a, b and c".
fn and_list(keys: &[String]) -> String {
    match keys {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
        [] => String::new(),
    }
}

/// A setting the repository names under a name `-c` cannot carry.
fn unswitchable(sub: &str, key: &str) -> AppError {
    AppError::Command(format!(
        "GitSwitch did not run git {}: this repository's own settings name a program under a name that cannot be switched off for one command ({}). Look at the repository's .git/config in a terminal.",
        sub, key
    ))
}

/// Forgets the cached config of a folder when dropped (see `run`).
pub(crate) struct ForgetAfterWrite(Option<Option<PathBuf>>);

impl Drop for ForgetAfterWrite {
    fn drop(&mut self) {
        if let Some(repo) = &self.0 {
            forget_repo_config(repo.as_deref());
        }
    }
}

// ---------------------------------------------------------------------------
// The safety prelude
//
// A git command the app starts ON ITS OWN — a page opening, window focus, a
// poll, a scan, a repository nobody picked, the push card's credential
// lookup, the History fetch — must never run a program that a repository's
// own config, attributes or hooks name: an extracted archive or a cloned
// folder carries its `.git/config` and `.git/hooks` with it.
//
// Every command gets `core.fsmonitor=` (empty), `protocol.ext.allow=never`
// and `GIT_PAGER=cat`. Beyond that, commands come in two classes (`shape`):
//
// * PASSIVE: an explicit allow-list of read-only subcommands. They get
//   `GIT_OPTIONAL_LOCKS=0`, an empty `core.hooksPath` the app owns, and, for
//   the diff family, `--no-ext-diff --no-textconv`. The ones that can reach a
//   program-naming key (status, the diff family, a signature format,
//   `credential fill`, the History fetch) also read the repository's config
//   and switch off what its local and worktree scopes name
//   (`neutralising_cfg`, `credential_cfg`, `transport_programs`).
// * WRITE: everything else, unknown commands included. Only the fixed pins:
//   the repository's filters, drivers, hooks, `core.sshCommand` and
//   credential helpers work as in a terminal, because they decide what gets
//   committed (a git-crypt clean filter encrypts — switched off, plaintext
//   would be committed) and which account pushes.
// ---------------------------------------------------------------------------

/// The prelude a command gets (see the section comment above).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Passive,
    Write,
}

/// What a passive command needs from the repository's config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    /// Nothing: the fixed pins are enough.
    None,
    /// Its filters, signature verifiers and diff drivers switched off (and,
    /// as for `Objects`, a partial clone's fetch kept from its programs).
    Neutral,
    /// It reads objects: in a partial clone a missing one makes git fetch it
    /// on its own, with the repository's transport settings. Refused when
    /// those name a program, otherwise only the person's own credential
    /// helpers. Read only when the repository's config files may make it a
    /// partial clone (`may_be_partial`).
    Objects,
    /// `credential fill`: only the person's own credential helpers, no askpass.
    Credential,
    /// The History fetch: as `Credential`, and refused when the repository
    /// names a transport program.
    HistoryFetch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shape {
    class: Class,
    read: Read,
    /// Where `--no-ext-diff --no-textconv` go in the arguments.
    diff_flags_at: Option<usize>,
    /// Gets the empty `core.hooksPath`. Not `config` and `rev-parse`: they run
    /// no hook, and they are how the app finds where hooks live
    /// (`rev-parse --git-path hooks`, `config --get core.hooksPath`), which
    /// the pin would answer with the app's own empty folder.
    hooks_pin: bool,
    /// Where the subcommand is in the arguments.
    sub_at: usize,
    /// git looks inside the work trees of the repository's submodules (to
    /// tell whether each has changes of its own): it runs `git status` in
    /// each, which reads THAT repository's config. The `-c` pairs reach
    /// those children (`GIT_CONFIG_PARAMETERS`), but the names of a
    /// submodule's filters are its own, so its config is read too
    /// (`submodule_filter_cfg`).
    recurse: bool,
}

/// Where the subcommand is in `args`, past git's own leading options; `None`
/// when an option comes first that this list does not know (the command is
/// then a write).
fn subcommand_at(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix" | "--config-env" | "--attr-source" => i += 2,
            "--no-optional-locks" | "--literal-pathspecs" | "--glob-pathspecs" | "--noglob-pathspecs" | "--icase-pathspecs" | "--no-pager" | "-P" | "--no-replace-objects" | "--bare" | "--no-lazy-fetch" | "--no-advice" | "--exec-path" => i += 1,
            "--version" | "-v" => return Some(i),
            _ if ["--git-dir=", "--work-tree=", "--namespace=", "--super-prefix=", "--config-env=", "--attr-source=", "--exec-path="].iter().any(|p| a.starts_with(p)) => i += 1,
            _ if a.starts_with('-') => return None,
            _ => return Some(i),
        }
    }
    None
}

/// The arguments that are not options, `-- …` included; `valued` lists the
/// options that take the next argument as their value.
fn positionals<'a>(rest: &[&'a str], valued: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        if a == "--" {
            out.extend(rest[i + 1..].iter().copied());
            break;
        }
        if valued.contains(&a) {
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            out.push(a);
        }
        i += 1;
    }
    out
}

/// A pretty format or option that makes git verify signatures.
fn asks_signature(arg: &str) -> bool {
    arg == "--show-signature" || arg.contains("%(signature") || arg.match_indices("%G").any(|(i, _)| arg[i + 2..].starts_with(['?', 'S', 'K', 'F', 'P', 'T', 'G']))
}

/// `git config …` that only reads.
fn config_reads(rest: &[&str]) -> bool {
    const WRITES: &[&str] = &["--unset", "--unset-all", "--add", "--replace-all", "--rename-section", "--remove-section", "-e", "--edit"];
    if rest.iter().any(|a| WRITES.contains(a)) {
        return false;
    }
    if rest.iter().any(|a| a.starts_with("--get") || *a == "-l" || *a == "--list") {
        return true;
    }
    let pos = positionals(rest, &["-f", "--file", "--blob", "--type", "--default", "--comment", "--value", "--url"]);
    // git 2.46+ spells reads as `config get …` / `config list`; before that,
    // one name alone is a read and a name with a value is a write. A name
    // always has a dot: a lone word is one of 2.46's other subcommands
    // (`config edit`), none of which only reads.
    matches!(pos.first(), Some(&"get") | Some(&"list")) || (pos.len() == 1 && pos[0].contains('.'))
}

/// The class of a git command, and what it needs (see the section comment).
/// `history_fetch` marks the History fetch and its housekeeping.
fn shape(args: &[String], history_fetch: bool) -> Shape {
    let write = |sub_at| Shape { class: Class::Write, read: Read::None, diff_flags_at: None, hooks_pin: false, sub_at, recurse: false };
    let Some(at) = subcommand_at(args) else { return write(args.len()) };
    let passive = |read, diff_flags_at| Shape { class: Class::Passive, read, diff_flags_at, hooks_pin: true, sub_at: at, recurse: false };
    if history_fetch {
        return passive(Read::HistoryFetch, None);
    }
    let rest: Vec<&str> = args[at + 1..].iter().map(String::as_str).collect();
    // The options git reads: nothing after `--` (paths) or `--end-of-options`
    // (revisions) is one. A path or a revision can come from the repository
    // (a submodule at the path `--cached`), and read as an option it would
    // switch a protection off. `positionals`, `first` and `config_reads`
    // keep the whole list.
    let opts = &rest[..rest.iter().position(|a| *a == "--" || *a == "--end-of-options").unwrap_or(rest.len())];
    let has = |f: &str| opts.contains(&f);
    let first = positionals(&rest, &[]).first().copied();
    let first_arg = rest.first().copied();
    // A command that reads objects but reaches no program-naming key still
    // needs to know whether the repository is a partial clone (`Objects`).
    let read_if = |reads: bool| if reads { Read::Neutral } else { Read::Objects };
    // An explicit `--textconv` or `--ext-diff` would undo the flags this
    // runner puts in front of it: the drivers themselves are switched off.
    let live_drivers = has("--textconv") || has("--ext-diff");
    let signature = opts.iter().any(|a| asks_signature(a));
    // With no format of its own, a log prints the repository's
    // `format.pretty`, which may ask for the signature.
    let own_format = opts.iter().any(|a| a.starts_with("--format") || a.starts_with("--pretty") || *a == "--oneline");
    // Whether git may look inside the submodules' work trees (`recurse`):
    // unless told to ignore them, or to ignore their work trees, a status or
    // a diff against the work tree does. The repository's own
    // `diff.ignoreSubmodules` or `submodule.<name>.ignore` cannot be relied on.
    let subs_ignored = opts.iter().any(|a| matches!(*a, "--ignore-submodules" | "--ignore-submodules=all" | "--ignore-submodules=dirty"));
    let recursing = |s: Shape, recurse: bool| Shape { recurse: recurse && !subs_ignored, ..s };
    match args[at].as_str() {
        "status" => recursing(passive(Read::Neutral, None), true),
        "diff" => recursing(passive(Read::Neutral, Some(at + 1)), !has("--cached") && !has("--staged") && !has("--no-index")),
        "diff-files" => recursing(passive(Read::Neutral, Some(at + 1)), true),
        "diff-index" => recursing(passive(Read::Neutral, Some(at + 1)), !has("--cached")),
        "diff-tree" | "range-diff" => passive(Read::Neutral, Some(at + 1)),
        "log" | "show" | "whatchanged" => passive(read_if(signature || live_drivers || !own_format), Some(at + 1)),
        "rev-list" | "for-each-ref" => passive(read_if(signature), None),
        // `--dirty` compares the work tree, through its clean filters.
        "describe" => {
            let dirty = opts.iter().any(|a| a.starts_with("--dirty") || a.starts_with("--broken"));
            recursing(passive(read_if(dirty), None), dirty)
        }
        "cat-file" => passive(read_if(has("--filters") || has("--textconv")), None),
        "grep" => passive(read_if(has("--textconv")), None),
        // `--modified` hashes stat-dirty files through their clean filters.
        // Every other form may read objects: a sparse index is expanded
        // first (its trees read), and so are the ignore rules and
        // `--with-tree`; in a partial clone a missing tree is fetched.
        "ls-files" => {
            let short = |c: char| opts.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(c));
            if has("--modified") || short('m') {
                passive(Read::Neutral, None)
            } else {
                passive(Read::Objects, None)
            }
        }
        // They read commits, trees or blobs (`check-attr`/`check-ignore`: the
        // rules of files only the index holds).
        "ls-tree" | "merge-base" | "name-rev" | "shortlog" | "check-ignore" | "check-attr" => passive(Read::Objects, None),
        "count-objects" | "var" | "--version" | "-v" | "version" => passive(Read::None, None),
        // A name reads no object; `HEAD~1`, `HEAD^{tree}` or `HEAD:path` do.
        "rev-parse" => Shape { hooks_pin: false, ..passive(if positionals(&rest, &[]).iter().any(|a| a.contains(['^', '~', ':'])) { Read::Objects } else { Read::None }, None) },
        "config" if config_reads(&rest) => Shape { hooks_pin: false, ..passive(Read::None, None) },
        "symbolic-ref" if !has("-d") && !has("--delete") && positionals(&rest, &["-m"]).len() <= 1 => passive(Read::None, None),
        // The stash list walks the stash's reflog, commit by commit.
        "stash" if first_arg == Some("list") => passive(Read::Objects, None),
        "stash" if first_arg == Some("show") => passive(Read::Neutral, Some(at + 2)),
        "submodule" if matches!(first, None | Some("status")) => passive(Read::Objects, None),
        "lfs" if first_arg == Some("version") => passive(Read::None, None),
        // git-lfs compares the work tree through git, and so its filters:
        // `lfs status`, and `lfs ls-files`, which runs `git diff-index HEAD`
        // against the work tree (and so a status in every submodule).
        "lfs" if matches!(first_arg, Some("status") | Some("ls-files")) => recursing(passive(Read::Neutral, None), true),
        "remote" if matches!(first, None | Some("get-url")) => passive(Read::None, None),
        "branch"
            if (has("--show-current") || has("--list") || has("-l"))
                && !rest.iter().any(|a| {
                    matches!(*a, "-d" | "-D" | "--delete" | "-m" | "-M" | "--move" | "-c" | "-C" | "--copy" | "-u" | "--unset-upstream" | "--edit-description" | "-f" | "--force" | "-t" | "--track" | "--no-track" | "--create-reflog" | "--set-upstream")
                        || a.starts_with("--set-upstream-to")
                        || a.starts_with("--track=")
                }) =>
        {
            // A list may sort or print by what the commits say.
            passive(if has("--list") || has("-l") { Read::Objects } else { Read::None }, None)
        }
        "worktree" if first_arg == Some("list") => passive(Read::None, None),
        "credential" if first_arg == Some("fill") => passive(Read::Credential, None),
        _ => write(at),
    }
}

/// The folder a passive command's `core.hooksPath` names: the app's own,
/// empty, so no hook of the repository's is found. Forward slashes, which
/// git reads the same on every platform. A folder that cannot be created is
/// still named: git finds no hook in a folder that does not exist either.
fn empty_hooks_dir() -> &'static str {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        // The test builds never touch the person's real data folder: the
        // suites name a sandbox one, cargo's own tests use the temp folder.
        #[cfg(test)]
        let base = Some(std::env::var_os("GITSWITCH_DATA_DIR").filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("com.gitswitch.app")).unwrap_or_else(|| std::env::temp_dir().join("gitswitch-test")));
        #[cfg(not(test))]
        let base = crate::paths::data_dir().map(|d| d.join("com.gitswitch.app"));
        hooks_dir_in(base)
    })
}

/// `empty_hooks_dir` for the app's own data folder, or for none (no HOME
/// and no passwd entry). Never a shared folder then: anyone on the machine
/// could create `/tmp/gitswitch/empty-hooks` first and put a hook in it,
/// which every passive command would run as the person. On unix the name
/// is `/dev/null`: git looks for `<hooksPath>/<hook>` (`find_hook`, an
/// `access(X_OK)` that fails with ENOTDIR below something that is not a
/// folder) and runs nothing. Windows has a per-user temp folder.
fn hooks_dir_in(base: Option<PathBuf>) -> String {
    let dir = match base {
        Some(b) => b.join("empty-hooks"),
        #[cfg(unix)]
        None => return "/dev/null".to_string(),
        #[cfg(not(unix))]
        None => std::env::temp_dir().join("gitswitch").join("empty-hooks"),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().replace('\\', "/")
}

/// One entry of `git config --list --show-scope -z`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConfigEntry {
    pub scope: String,
    /// As git lists it: section and name in lower case, subsection as written.
    pub key: String,
    /// `None` for a bare `key` line (boolean true).
    pub value: Option<String>,
    /// The key's bytes were UTF-8. A subsection may hold any bytes, and one
    /// that is not UTF-8 cannot be named again with `-c`: decoded, it names
    /// another filter or driver (and Windows passes arguments as UTF-16).
    pub key_utf8: bool,
}

/// `<scope>NUL<key>LF<value>NUL`, or `<scope>NUL<key>NUL` for a key with no
/// value: the `-z` form of `config --list --show-scope` (git 2.26+).
pub(crate) fn parse_config_list(raw: &[u8]) -> Vec<ConfigEntry> {
    let fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    fields
        .chunks(2)
        .filter(|pair| pair.len() == 2 && !pair[0].is_empty())
        .map(|pair| {
            let raw_key = pair[1].split(|b| *b == b'\n').next().unwrap_or_default();
            let kv = String::from_utf8_lossy(pair[1]);
            let (key, value) = match kv.split_once('\n') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (kv.to_string(), None),
            };
            ConfigEntry { scope: String::from_utf8_lossy(pair[0]).to_string(), key, value, key_utf8: std::str::from_utf8(raw_key).is_ok() }
        })
        .collect()
}

/// The person's own scopes. Local and worktree are the repository's, and so
/// are the files their `include.path`/`includeIf` pull in: git lists those
/// under the scope of the file that included them.
fn trusted_scope(scope: &str) -> bool {
    matches!(scope, "system" | "global" | "command")
}

/// git-lfs's own filter values, as `git lfs install` writes them (the
/// `--skip` forms are `install --skip-smudge`). They run git-lfs and nothing
/// else, and the app runs `git lfs install --local` itself.
const LFS_FILTER: &[(&str, &str)] = &[
    ("clean", "git-lfs clean -- %f"),
    ("smudge", "git-lfs smudge -- %f"),
    ("smudge", "git-lfs smudge --skip -- %f"),
    ("process", "git-lfs filter-process"),
    ("process", "git-lfs filter-process --skip"),
];

/// The `-c` pairs that switch off what a repository's own config (local and
/// worktree scope) names for a passive status or diff to run:
///
/// * `filter.<name>.clean`/`.smudge`/`.process`: empty, and
///   `filter.<name>.required=false` (a required filter with no program is a
///   fatal error). git-lfs's own values are kept, unless the repository also
///   configures an LFS extension (`lfs.extension.*`), a program git-lfs's
///   clean would run.
/// * `gpg.program`/`gpg.openpgp.program`, `gpg.ssh.program`,
///   `gpg.x509.program`: the person's own value, else git's default name. Not
///   empty: git runs an empty program name, and every signature would read as
///   "cannot check".
/// * `gpg.ssh.defaultKeyCommand`: the global value, else empty (only signing
///   reads it, which no passive command does).
/// * `diff.<name>.command`/`.textconv`: empty. `--no-ext-diff --no-textconv`
///   already keep a diff from them; with the empty value any other use fails
///   instead of running something.
///
/// `Err(key)` when such a key cannot be passed with `-c`: an `=` in its
/// name, or a filter or diff driver whose name is not UTF-8.
pub(crate) fn neutralising_cfg(entries: &[ConfigEntry]) -> Result<Vec<(String, String)>, String> {
    if let Some(e) = entries.iter().find(|e| !trusted_scope(&e.scope) && !e.key_utf8 && (e.key.starts_with("filter.") || e.key.starts_with("diff."))) {
        return Err(e.key.clone());
    }
    // The repository's last word on each key, in order of first appearance.
    let mut local: Vec<(&str, Option<&str>)> = Vec::new();
    for e in entries.iter().filter(|e| !trusted_scope(&e.scope)) {
        match local.iter_mut().find(|(k, _)| *k == e.key) {
            Some(slot) => slot.1 = e.value.as_deref(),
            None => local.push((&e.key, e.value.as_deref())),
        }
    }
    let has_local = |key: &str| local.iter().any(|(k, _)| *k == key);
    let trusted_last = |keys: &[&str], scopes: &[&str]| entries.iter().filter(|e| scopes.contains(&e.scope.as_str()) && keys.contains(&e.key.as_str())).filter_map(|e| e.value.clone()).next_back();
    let lfs_extension = local.iter().any(|(k, _)| k.starts_with("lfs.extension."));

    let mut out: Vec<(String, String)> = Vec::new();
    let mut emptied: Vec<&str> = Vec::new();
    for (key, value) in &local {
        let Some((name, kind)) = key.strip_prefix("filter.").and_then(|r| r.rsplit_once('.')) else { continue };
        if !matches!(kind, "clean" | "smudge" | "process") {
            continue;
        }
        let lfs_own = name == "lfs" && !lfs_extension && value.is_some_and(|v| LFS_FILTER.contains(&(kind, v)));
        if lfs_own {
            continue;
        }
        out.push((key.to_string(), String::new()));
        if !emptied.contains(&name) {
            emptied.push(name);
        }
    }
    // git-lfs's filter is usually defined in the person's own config (`git
    // lfs install`, or Git for Windows' system config), and its clean runs
    // the repository's LFS extensions all the same: switched off whatever
    // scope defines it.
    if lfs_extension {
        for kind in ["clean", "smudge", "process"] {
            let key = format!("filter.lfs.{}", kind);
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, String::new()));
            }
        }
        if !emptied.contains(&"lfs") {
            emptied.push("lfs");
        }
    }
    for name in emptied {
        out.push((format!("filter.{}.required", name), "false".to_string()));
    }

    const TRUSTED: &[&str] = &["system", "global", "command"];
    if has_local("gpg.program") || has_local("gpg.openpgp.program") {
        // Both keys set the same program (the last one git reads wins), so
        // both get the same value.
        let v = trusted_last(&["gpg.program", "gpg.openpgp.program"], TRUSTED).unwrap_or_else(|| "gpg".to_string());
        out.push(("gpg.program".to_string(), v.clone()));
        out.push(("gpg.openpgp.program".to_string(), v));
    }
    for (key, default) in [("gpg.ssh.program", "ssh-keygen"), ("gpg.x509.program", "gpgsm")] {
        if has_local(key) {
            out.push((key.to_string(), trusted_last(&[key], TRUSTED).unwrap_or_else(|| default.to_string())));
        }
    }
    if has_local("gpg.ssh.defaultkeycommand") {
        out.push(("gpg.ssh.defaultKeyCommand".to_string(), trusted_last(&["gpg.ssh.defaultkeycommand"], &["global"]).unwrap_or_default()));
    }

    for (key, _) in &local {
        if key.strip_prefix("diff.").and_then(|r| r.rsplit_once('.')).is_some_and(|(_, kind)| kind == "command" || kind == "textconv") {
            out.push((key.to_string(), String::new()));
        }
    }

    if let Some((key, _)) = out.iter().find(|(k, _)| k.contains('=')) {
        return Err(key.clone());
    }
    Ok(out)
}

/// The `-c` pairs for `credential fill` and the History fetch: the helper
/// list cleared (`credential.helper=` empties it, git 2.9+), then every helper
/// entry of the person's own scopes put back in its order and under its own
/// key (`credential.<url>.helper` still only answers for its URL), then
/// `core.askPass=` (git skips an empty askpass). The hardened environment
/// removes `GIT_ASKPASS`, which would otherwise let a repository's
/// `core.askPass` take its place. With no config read (`None`) the helpers
/// are only cleared. A helper key with `=` in it cannot be passed with `-c`
/// and is left out.
pub(crate) fn credential_cfg(entries: Option<&[ConfigEntry]>) -> Vec<(String, String)> {
    let mut out = vec![("credential.helper".to_string(), String::new())];
    for e in entries.unwrap_or_default().iter().filter(|e| trusted_scope(&e.scope)) {
        let helper = e.key.starts_with("credential.") && e.key.ends_with(".helper");
        if let (true, false, Some(v)) = (helper, e.key.contains('='), &e.value) {
            out.push((e.key.clone(), v.clone()));
        }
    }
    out.push(("core.askPass".to_string(), String::new()));
    out
}

/// The keys of a repository's own config (local or worktree scope) that make
/// a fetch run a program and that cannot be overridden from outside: git runs
/// an empty `core.sshCommand`, `uploadpack` or `vcs` as a program name, and
/// `core.gitProxy` takes its first match. The History fetch is refused when
/// any is set.
pub(crate) fn transport_programs(entries: &[ConfigEntry]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| !trusted_scope(&e.scope)) {
        let k = e.key.as_str();
        let named = match k {
            "core.sshcommand" => Some("core.sshCommand".to_string()),
            "core.gitproxy" => Some("core.gitProxy".to_string()),
            "core.alternaterefscommand" => Some("core.alternateRefsCommand".to_string()),
            "lfs.standalonetransferagent" => Some("lfs.standaloneTransferAgent".to_string()),
            _ if k.starts_with("remote.") && [".uploadpack", ".receivepack", ".vcs"].iter().any(|s| k.ends_with(s)) => Some(k.to_string()),
            _ if k.starts_with("lfs.customtransfer.") => Some(k.to_string()),
            _ => None,
        };
        if let Some(n) = named.filter(|n| !found.contains(n)) {
            found.push(n);
        }
    }
    found
}

/// git fetches a missing object on its own (promisor-remote.c) in a
/// repository with a promisor remote: `extensions.partialClone`, or
/// `remote.<name>.promisor`. Any scope and any value counts (one more read
/// than needed costs nothing; one fewer would let the fetch run).
pub(crate) fn partial_clone(entries: &[ConfigEntry]) -> bool {
    entries.iter().any(|e| e.key == "extensions.partialclone" || (e.key.starts_with("remote.") && e.key.ends_with(".promisor")))
}

/// Where git keeps a folder's repository: the folder it finds (`git_dir`,
/// per work tree: the index, `config.worktree`) and the one it shares
/// (`common_dir`: `config`, the objects).
#[derive(Debug, PartialEq)]
pub(crate) struct GitDirs {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// A file's text, read only when it is a regular file: opening a FIFO (an
/// archive can carry one in `.git`) waits for a writer that never comes,
/// with no clock to stop it — git, asked instead, runs on one. `Ok(None)`:
/// there is nothing at that path.
fn regular_file(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => std::fs::read(path).map(Some),
        Ok(_) => Err(std::io::Error::other("not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The repository git finds from `folder`, by the same walk up to the first
/// `.git` (a folder, or a file saying `gitdir: …`). `None` when anything is
/// out of the ordinary — that `.git` is not a repository git would accept
/// (git would walk on past an empty `.git` folder), cannot be read, or is
/// neither a folder nor a regular file, nor is its `commondir` — so the
/// callers ask git instead of guessing.
pub(crate) fn find_git_dirs(folder: &Path) -> Option<GitDirs> {
    let start = if folder.is_absolute() { folder.to_path_buf() } else { std::env::current_dir().ok()?.join(folder) };
    let mut at: &Path = &start;
    loop {
        let dot = at.join(".git");
        if let Ok(meta) = std::fs::metadata(&dot) {
            let git_dir = if meta.is_dir() {
                dot
            } else {
                let text = String::from_utf8(regular_file(&dot).ok()??).ok()?;
                let rel = text.lines().next()?.strip_prefix("gitdir:")?.trim();
                if rel.is_empty() {
                    return None;
                }
                at.join(rel)
            };
            let common_dir = match regular_file(&git_dir.join("commondir")) {
                Ok(Some(bytes)) => git_dir.join(String::from_utf8(bytes).ok()?.lines().next()?.trim()),
                Ok(None) => git_dir.clone(),
                Err(_) => return None,
            };
            let accepted = git_dir.join("HEAD").is_file() && common_dir.join("objects").is_dir() && common_dir.join("refs").is_dir();
            return accepted.then_some(GitDirs { git_dir, common_dir });
        }
        at = at.parent()?;
    }
}

/// Whether the repository's own config files could make it a partial clone,
/// or pull in another file that could: a word search, in any case, for
/// `promisor`, `partialclone` and `include` in `config` and
/// `config.worktree` (the local and worktree scopes). Config names are ASCII
/// and never broken across lines, so a file without those words sets none of
/// those keys. A file that cannot be read, or is not a regular file, counts
/// as "could".
pub(crate) fn may_be_partial(dirs: &GitDirs) -> bool {
    for file in [dirs.common_dir.join("config"), dirs.git_dir.join("config.worktree"), dirs.common_dir.join("config.worktree")] {
        match regular_file(&file) {
            Ok(Some(bytes)) => {
                let low = bytes.to_ascii_lowercase();
                if [&b"promisor"[..], b"partialclone", b"include"].iter().any(|w| low.windows(w.len()).any(|x| x == *w)) {
                    return true;
                }
            }
            Ok(None) => {}
            Err(_) => return true,
        }
    }
    false
}

/// Whether the index may record a gitlink: git writes an entry's mode as
/// four bytes, and a gitlink's (0160000) is `00 00 E0 00`. Not found in the
/// index (nor in a split index's shared part), there is none; found, the
/// index is listed to be sure. Read in pieces, as an index can be large. A
/// file that cannot be read, or is not a regular file (opening a FIFO would
/// wait for good), counts as "may".
pub(crate) fn index_may_hold_gitlinks(dirs: &GitDirs) -> bool {
    const GITLINK: [u8; 4] = [0, 0, 0xE0, 0];
    let holds = |path: &Path| -> std::io::Result<bool> {
        use std::io::Read;
        if !std::fs::metadata(path)?.is_file() {
            return Ok(true);
        }
        let mut f = std::fs::File::open(path)?;
        let mut buf = vec![0u8; 1 << 16];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                return Ok(false);
            }
            carry.extend_from_slice(&buf[..n]);
            if carry.windows(4).any(|w| w == GITLINK) {
                return Ok(true);
            }
            let keep = carry.len().min(3);
            carry.drain(..carry.len() - keep);
        }
    };
    let mut files = vec![dirs.git_dir.join("index")];
    for dir in [&dirs.git_dir, &dirs.common_dir] {
        match std::fs::read_dir(dir) {
            Ok(list) => files.extend(list.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("sharedindex.")).map(|e| e.path())),
            Err(_) => return true,
        }
    }
    files.iter().any(|f| match holds(f) {
        Ok(found) => found,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    })
}

fn fetch_refusal(keys: &[String]) -> AppError {
    AppError::Command(format!(
        "GitSwitch did not fetch: this repository's own settings name {}, which a fetch would run, and a fetch the app makes on its own never runs a program a repository names. Fetch from a terminal, or move the setting into your profile.",
        and_list(keys)
    ))
}

fn fetch_read_failed(why: &ReadFailed) -> AppError {
    AppError::Command(format!("GitSwitch did not fetch: it reads this repository's own settings first, and that read failed: {}", why))
}

/// The History fetch's own check, made before any remote is tried, so a
/// refusal is said once rather than once per remote.
pub async fn check_history_fetch(repo: &Path) -> Result<(), AppError> {
    match repo_config(None, Some(repo.to_path_buf()), Vec::new()).await {
        Ok(entries) => match transport_programs(&entries) {
            found if found.is_empty() => Ok(()),
            found => Err(fetch_refusal(&found)),
        },
        Err(why) => Err(fetch_read_failed(&why)),
    }
}

/// Why a read of a repository failed. `refused`: git ran and said no — the
/// folder is not a repository git can open (any command there fails the
/// same way). Otherwise git could not be run, or was stopped on its clock.
#[derive(Clone, Debug)]
pub(crate) struct ReadFailed {
    why: String,
    refused: bool,
}

impl std::fmt::Display for ReadFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl ReadFailed {
    fn from_run(run: Result<GitOutput, AppError>, what: &str) -> Result<GitOutput, ReadFailed> {
        match run {
            Ok(out) if out.ok() => Ok(out),
            Ok(out) if out.stderr.is_empty() => Err(ReadFailed { why: format!("git {} exited with code {}", what, out.code), refused: true }),
            Ok(out) => Err(ReadFailed { why: out.stderr, refused: true }),
            Err(e) => Err(ReadFailed { why: e.to_string(), refused: false }),
        }
    }
}

type ConfigRead = Result<std::sync::Arc<Vec<ConfigEntry>>, ReadFailed>;
/// The submodule paths a folder's index records (gitlinks), relative to it.
type GitlinkRead = Result<std::sync::Arc<Vec<String>>, ReadFailed>;
/// The `-c` pairs for the submodules' filters, or why they cannot be known.
type SubmoduleCfg = Result<SubmoduleCheck, String>;

/// What the submodules a recursing command looks inside need from it.
#[derive(Debug, Default)]
struct SubmoduleCheck {
    /// Their filters, switched off.
    pairs: Vec<(String, String)>,
    /// One of them is a partial clone (whose own settings name no
    /// transport program): a fetch there must ask only the person's helpers.
    partial: bool,
}
type ConfigKey = (Option<PathBuf>, Option<PathBuf>, Vec<String>);

/// One cached read: when it was started, and the read itself, which
/// concurrent first callers share.
struct ConfigSlot<T> {
    at: std::time::Instant,
    cell: std::sync::Arc<tokio::sync::OnceCell<T>>,
}

type ReadCache<T> = std::sync::Mutex<std::collections::HashMap<ConfigKey, ConfigSlot<T>>>;

/// How long a repository's config read is reused: one page load's worth of
/// commands, not long enough to matter when the person edits the config.
const REPO_CONFIG_TTL: Duration = Duration::from_secs(5);

fn config_cache() -> &'static ReadCache<ConfigRead> {
    static CACHE: std::sync::OnceLock<ReadCache<ConfigRead>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn gitlink_cache() -> &'static ReadCache<GitlinkRead> {
    static CACHE: std::sync::OnceLock<ReadCache<GitlinkRead>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn lock<T>(cache: &'static ReadCache<T>) -> std::sync::MutexGuard<'static, std::collections::HashMap<ConfigKey, ConfigSlot<T>>> {
    cache.lock().unwrap_or_else(|p| p.into_inner())
}

/// `read`, done once per key for a few seconds; concurrent first calls share it.
async fn cached_read<T, F>(cache: &'static ReadCache<T>, key: ConfigKey, read: impl FnOnce() -> F) -> T
where
    T: Clone,
    F: std::future::Future<Output = T>,
{
    let cell = {
        let mut map = lock(cache);
        map.retain(|_, slot| slot.at.elapsed() < REPO_CONFIG_TTL);
        map.entry(key).or_insert_with(|| ConfigSlot { at: std::time::Instant::now(), cell: Default::default() }).cell.clone()
    };
    cell.get_or_init(read).await.clone()
}

/// The repository's config with scopes, read once per folder for a few
/// seconds (keyed by the git binary, the folder and any leading options such
/// as `--git-dir`). Concurrent first calls share one read.
async fn repo_config(program: Option<PathBuf>, repo: Option<PathBuf>, lead: Vec<String>) -> ConfigRead {
    let key = (program.clone(), repo.clone(), lead.clone());
    cached_read(config_cache(), key, || read_repo_config(program, repo, lead)).await
}

/// The gitlinks of a folder's index, read once per folder for a few seconds,
/// like its config.
async fn gitlinks(program: Option<PathBuf>, dir: Option<PathBuf>) -> GitlinkRead {
    let key = (program.clone(), dir.clone(), Vec::new());
    cached_read(gitlink_cache(), key, || read_gitlinks(program, dir)).await
}

/// `ls-files -s -z :/`: every index entry from the top of the work tree
/// (paths relative to the folder, `../` included), the gitlinks kept. The
/// `:/` needs pathspec magic, which an inherited `GIT_LITERAL_PATHSPECS`
/// would switch off. The listing is the whole index, so it stays out of the
/// command log. A boxed future, for the same reason as the config read.
fn read_gitlinks(program: Option<PathBuf>, dir: Option<PathBuf>) -> std::pin::Pin<Box<dyn std::future::Future<Output = GitlinkRead> + Send>> {
    Box::pin(async move {
        // An index with no gitlink in it has nothing to list: no process is
        // spent on the (usual) repository without submodules.
        let folder = dir.clone().unwrap_or_else(|| PathBuf::from("."));
        let none_there = tokio::task::spawn_blocking(move || find_git_dirs(&folder).is_some_and(|dirs| !index_may_hold_gitlinks(&dirs))).await.unwrap_or(false);
        if none_there {
            return Ok(std::sync::Arc::new(Vec::new()));
        }
        let mut cmd = match &dir {
            Some(d) => GitCmd::at(d),
            None => GitCmd::without_repo(),
        };
        if let Some(p) = program {
            cmd = cmd.with_program(p);
        }
        let run = cmd.env("GIT_LITERAL_PATHSPECS", "0").args(["ls-files", "-s", "-z", ":/"]).private_output().run().await;
        let out = ReadFailed::from_run(run, "ls-files")?;
        Ok(std::sync::Arc::new(crate::submodules::parse_gitlinks(&out.stdout).into_iter().map(|(_, path)| path).collect()))
    })
}

/// More submodules than this, counted down every level, and a recursing
/// passive command is refused rather than left half-checked.
const MAX_SUBMODULES: usize = 512;
/// Submodules inside submodules, this deep at most.
const MAX_SUBMODULE_DEPTH: usize = 16;

/// The `-c` pairs that switch off the filters of every submodule git would
/// look inside (`Shape::recurse`), nested ones included. git looks inside a
/// gitlink of the index whose folder holds a `.git` — mapped in
/// `.gitmodules` or not — and runs `git status` there with `GIT_DIR=.git`;
/// that child reads the submodule's own config, and gets the `-c` pairs of
/// the command that started it. Only the filters matter there (fsmonitor
/// and the hooks folder are pinned for every repository by the prelude, and
/// status runs no diff driver or signature check). A submodule git cannot
/// open (its config read is refused) is skipped: git's own status fails
/// there the same way, before any filter. `Err` says why the command cannot
/// be checked.
/// `only`: the paths the command is limited to (`pathspec_prefixes`); a
/// top-level submodule none of them reaches is not looked inside by git
/// either. `None`: every one.
fn submodule_filter_cfg(program: Option<PathBuf>, repo: Option<PathBuf>, lead: Vec<String>, only: Option<Vec<String>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = SubmoduleCfg> + Send>> {
    Box::pin(async move {
        // Options that point git at another repository or work tree would
        // make the folder's own listing the wrong one.
        if lead.iter().any(|a| a == "--bare" || ["--git-dir", "--work-tree"].iter().any(|o| a == o || a.starts_with(&format!("{}=", o)))) {
            return Err("this command names its repository in a way that check does not follow.".to_string());
        }
        let mut out = SubmoduleCheck::default();
        // Each folder once. A submodule folder git cannot open as a
        // repository of its own (an empty `.git` folder) lists its
        // superproject's index again, `../` paths and all.
        let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        visited.insert(canonical(repo.as_deref().unwrap_or(Path::new("."))));
        let mut level: Vec<Option<PathBuf>> = vec![repo];
        let mut seen = 0usize;
        for depth in 0..=MAX_SUBMODULE_DEPTH {
            if level.is_empty() {
                return Ok(out);
            }
            // The downloaded submodules of this level's folders, four
            // listings at a time (as the submodule list itself reads them).
            let mut subs: Vec<(String, PathBuf)> = Vec::new();
            for chunk in level.chunks(4) {
                let tasks: Vec<_> = chunk
                    .iter()
                    .cloned()
                    .map(|dir| {
                        let program = program.clone();
                        tokio::spawn(async move { (dir.clone(), gitlinks(program, dir).await) })
                    })
                    .collect();
                for task in tasks {
                    let (dir, links) = task.await.map_err(|e| format!("listing them failed: {}", e))?;
                    let links = match links {
                        Ok(links) => links,
                        // git cannot read that index either: nothing of it
                        // is looked inside.
                        Err(e) if e.refused => continue,
                        Err(e) => return Err(format!("listing them failed: {}", e)),
                    };
                    let base = dir.unwrap_or_else(|| PathBuf::from("."));
                    for rel in links.iter() {
                        // Only the top folder may sit below its work tree's
                        // top (`:/` lists the rest as `../`).
                        if depth > 0 && rel.starts_with("../") {
                            continue;
                        }
                        if depth == 0 && only.as_ref().is_some_and(|only| !only.iter().any(|p| pathspec_reaches(p, rel))) {
                            continue;
                        }
                        let sub = base.join(rel);
                        if sub.join(".git").exists() && visited.insert(canonical(&sub)) {
                            subs.push((rel.clone(), sub));
                        }
                    }
                }
            }
            seen += subs.len();
            if seen > MAX_SUBMODULES {
                return Err(format!("it has more than {} downloaded submodules, more than it checks.", MAX_SUBMODULES));
            }
            let mut next: Vec<Option<PathBuf>> = Vec::new();
            for chunk in subs.chunks(4) {
                let tasks: Vec<_> = chunk
                    .iter()
                    .cloned()
                    .map(|(rel, sub)| {
                        let program = program.clone();
                        tokio::spawn(async move {
                            let read = repo_config(program, Some(sub.clone()), Vec::new()).await;
                            (rel, sub, read)
                        })
                    })
                    .collect();
                for task in tasks {
                    let (rel, sub, read) = task.await.map_err(|e| format!("reading them failed: {}", e))?;
                    match read {
                        Ok(entries) => {
                            let pairs = neutralising_cfg(&entries).map_err(|key| format!("{} names a program under a name that cannot be switched off for one command ({}).", rel, key))?;
                            out.pairs.extend(pairs.into_iter().filter(|(k, _)| k.starts_with("filter.")));
                            // git's status in a submodule that is a partial
                            // clone may fetch there, with ITS settings.
                            if partial_clone(&entries) {
                                let found = transport_programs(&entries);
                                if !found.is_empty() {
                                    return Err(format!(
                                        "its submodule {} is a partial clone whose own settings name {}, which git would run to fetch a missing object on its own. Run it from a terminal, or move the setting into your profile.",
                                        rel,
                                        and_list(&found)
                                    ));
                                }
                                out.partial = true;
                            }
                            next.push(Some(sub));
                        }
                        Err(e) if e.refused => {}
                        Err(e) => return Err(format!("reading those of {} failed: {}", rel, e)),
                    }
                }
            }
            level = next;
        }
        if level.is_empty() {
            Ok(out)
        } else {
            Err(format!("its submodules are nested more than {} deep, deeper than it checks.", MAX_SUBMODULE_DEPTH))
        }
    })
}

/// The paths after a command's `--`, when each is a plain path: a diff of one
/// file looks inside no submodule but the ones it names. `None` (every
/// submodule) without a `--`, or when one could match more than its own
/// text: pathspec magic other than `:(literal)`, or a wildcard (unless
/// `--literal-pathspecs` comes first).
fn pathspec_prefixes(lead: &[String], command: &[String]) -> Option<Vec<String>> {
    let literal = lead.iter().any(|a| a == "--literal-pathspecs");
    let after = command.iter().position(|a| a == "--")?;
    let mut out = Vec::new();
    for spec in &command[after + 1..] {
        let (plain, is_literal) = match spec.strip_prefix(":(literal)") {
            Some(rest) => (rest, true),
            None => (spec.as_str(), literal),
        };
        if plain.starts_with(':') || plain.contains('\\') || (!is_literal && plain.contains(['*', '?', '['])) {
            return None;
        }
        out.push(plain.trim_start_matches("./").trim_end_matches('/').to_string());
    }
    (!out.is_empty()).then_some(out)
}

/// Whether a plain path given to a command may reach a submodule's folder:
/// it names the submodule, a folder it is in, or something inside it. Any
/// case, and with no folder boundary required — erring towards looking.
fn pathspec_reaches(spec: &str, gitlink: &str) -> bool {
    let (spec, gitlink) = (spec.to_lowercase(), gitlink.to_lowercase());
    spec.is_empty() || spec == "." || gitlink.starts_with(&spec) || spec.starts_with(&gitlink)
}

/// The read itself: `config --list --show-scope --includes -z` (git 2.26+),
/// with the fixed pins like every command. Its output can hold secrets
/// (tokens in URLs, inline helpers), so it is kept out of the command log.
/// A boxed future: it runs `GitCmd::run`, which awaits this.
fn read_repo_config(program: Option<PathBuf>, repo: Option<PathBuf>, lead: Vec<String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ConfigRead> + Send>> {
    Box::pin(async move {
        let mut cmd = match &repo {
            Some(r) => GitCmd::at(r),
            None => GitCmd::without_repo(),
        };
        if let Some(p) = program {
            cmd = cmd.with_program(p);
        }
        let run = cmd.args(lead).args(["config", "--list", "--show-scope", "--includes", "-z"]).private_output().run().await;
        let out = ReadFailed::from_run(run, "config")?;
        Ok(std::sync::Arc::new(parse_config_list(&out.stdout)))
    })
}

/// Forget what was read for a folder (every folder for a command with
/// none), after the app may have changed its config.
fn forget_repo_config(repo: Option<&Path>) {
    fn forget<T>(cache: &'static ReadCache<T>, repo: Option<&Path>) {
        let mut map = lock(cache);
        match repo {
            // The folders inside it too: a write in a superproject reaches
            // its submodules (a download, an update).
            Some(r) => map.retain(|(_, folder, _), _| !folder.as_deref().is_some_and(|f| f.starts_with(r))),
            None => map.clear(),
        }
    }
    // The index too: a write may add, remove or move a submodule.
    forget(config_cache(), repo);
    forget(gitlink_cache(), repo);
}

/// Where `run_blocking`'s commands run. Its own, because the caller's thread
/// may belong to another runtime that it is blocking; two workers, because
/// each command is mostly waiting on its pipes.
fn blocking_runtime() -> Result<&'static tokio::runtime::Runtime, AppError> {
    static RT: std::sync::OnceLock<Option<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("gitswitch-git").enable_all().build().ok())
        .as_ref()
        .ok_or_else(|| AppError::Command("Failed to run git: no runtime to run it on".into()))
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

/// Whether an error is `run` stopping a command on its clock (the overall
/// timeout or the stall window), not git answering. A caller that reads a
/// failure as "nothing there" must not read a stop that way: a History page
/// whose walk was stopped is not a repository without commits.
pub fn was_stopped(e: &AppError) -> bool {
    let AppError::Command(m) = e else { return false };
    (m.starts_with("git ") && (m.contains(" was stopped after ") || m.contains(" made no progress for ")))
        || m.starts_with("The large-file download was stopped after ")
}

/// Whether an error is the safety prelude declining to run a command (a
/// partial clone whose own settings name a transport program, a setting
/// that cannot be switched off, a read of the settings that failed), not
/// git answering. Like a stop on the clock, it is not "nothing there": a
/// History page that was refused is not a repository without commits.
pub fn was_refused(e: &AppError) -> bool {
    let AppError::Command(m) = e else { return false };
    m.starts_with("GitSwitch did not run git ") || m.starts_with("GitSwitch did not fetch")
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

/// A full object id (40 hex digits for SHA-1, 64 for SHA-256) and nothing
/// else. An id read back from the repository (a commit's header lines, a
/// saved state, a field of `git log` output) goes to git as a revision only
/// when it passes: anything else could be read as an option (`--output=<file>`).
pub fn is_object_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// For the other modules' tests: proof that a runner goes through `GitCmd`.
#[cfg(test)]
pub(crate) mod test_support {
    /// Arguments that make git print the environment its children get (an
    /// alias needs no repository, so any existing folder will do).
    pub(crate) const SHOW_ENV: [&str; 3] = ["-c", "alias.gitswitch-env=!env", "gitswitch-env"];

    /// `env` output carries what `hardened_env` sets. Only the names are
    /// reported, never the environment itself: it may hold a token.
    pub(crate) fn assert_hardened(env: &str) {
        for kv in ["GIT_TERMINAL_PROMPT=0", "SSH_ASKPASS_REQUIRE=never", "GIT_EDITOR=true", "LC_ALL=C"] {
            assert!(env.lines().any(|l| l.trim() == kv), "{} is missing: this git did not run through GitCmd", kv);
        }
    }

    /// One command as `settle` classed it.
    #[derive(Clone, Debug, serde::Serialize)]
    pub(crate) struct Ran {
        pub repo: Option<std::path::PathBuf>,
        pub args: Vec<String>,
        pub class: super::Class,
    }

    fn ran() -> std::sync::MutexGuard<'static, Vec<Ran>> {
        static RAN: std::sync::OnceLock<std::sync::Mutex<Vec<Ran>>> = std::sync::OnceLock::new();
        RAN.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Kept for the tests (and the probe's `PROBE_CLASS_FILE`): what ran,
    /// where, and as which class. Bounded, as a test binary runs thousands.
    pub(crate) fn record(repo: Option<&std::path::Path>, args: &[String], class: super::Class) {
        let mut r = ran();
        if r.len() < 50_000 {
            r.push(Ran { repo: repo.map(|p| p.to_path_buf()), args: args.to_vec(), class });
        }
    }

    /// Every command recorded in `dir` or a folder inside it.
    pub(crate) fn ran_under(dir: &std::path::Path) -> Vec<Ran> {
        ran().iter().filter(|r| r.repo.as_deref().is_some_and(|p| p.starts_with(dir))).cloned().collect()
    }

    /// Every command recorded so far.
    pub(crate) fn ran_all() -> Vec<Ran> {
        ran().clone()
    }
}

#[cfg(test)]
mod tests {
    /// The lines of a source file that end up in the app: `#[cfg(test)]`
    /// items (and `#[cfg(all(test, …))]` ones) and whole-line comments left
    /// out. An item ends on its own line when that line ends it, otherwise
    /// at the `}` at the attribute's own indentation.
    fn production_lines(src: &str) -> Vec<(usize, &str)> {
        let lines: Vec<&str> = src.lines().collect();
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let t = lines[i].trim_start();
            if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test") {
                let at = indent(lines[i]);
                let mut j = i;
                let mut item = t[t.find(")]").map_or(t.len(), |p| p + 2)..].trim();
                while (item.is_empty() || item.starts_with("#[")) && j + 1 < lines.len() {
                    j += 1;
                    item = lines[j].trim();
                }
                if item.ends_with(';') || item.ends_with('}') || item.ends_with(',') {
                    i = j + 1;
                } else {
                    let mut k = j + 1;
                    while k < lines.len() && !(indent(lines[k]) == at && lines[k].trim_start().starts_with('}')) {
                        k += 1;
                    }
                    i = k + 1;
                }
                continue;
            }
            if !t.starts_with("//") {
                out.push((i + 1, lines[i]));
            }
            i += 1;
        }
        out
    }

    /// The lines where a production line starts git itself: a process
    /// constructor (`spawn::command`, `spawn::async_command`,
    /// `Command::new`) or `spawn::program` given `"git"`, `"git.exe"` or
    /// `system_git()`, whatever path is written in front, across line
    /// breaks. `GitCmd::with_program` is the runner and is not counted.
    fn direct_git_spawns(src: &str) -> Vec<usize> {
        let mut flat = String::new();
        let mut at: Vec<usize> = Vec::new();
        for (n, line) in production_lines(src) {
            for ch in line.chars().filter(|c| !c.is_whitespace()) {
                flat.push(ch);
                at.extend(std::iter::repeat_n(n, ch.len_utf8()));
            }
        }
        let mut found = Vec::new();
        for head in ["command(", "Command::new(", "program("] {
            for (i, _) in flat.match_indices(head) {
                if flat[..i].ends_with("with_") {
                    continue;
                }
                // `with_program(crate::spawn::program("git"))`: the runner
                // given git found the way a terminal finds it, not a spawn.
                if head == "program(" && flat[..i].trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == ':' || c == '&').ends_with("with_program(") {
                    continue;
                }
                let mut arg = flat[i + head.len()..].trim_start_matches('&');
                while let Some(p) = arg.find("::").filter(|&p| p > 0 && arg[..p].chars().all(|c| c.is_alphanumeric() || c == '_')) {
                    arg = &arg[p + 2..];
                }
                if arg.starts_with("\"git\"") || arg.starts_with("\"git.exe\"") || arg.starts_with("system_git(") {
                    found.push(at[i]);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }

    #[test]
    fn the_scan_sees_a_direct_git_spawn_and_leaves_tests_and_the_runner_alone() {
        let src = [
            "fn a() {",                                                                     // 1
            "    let o = crate::spawn::command(\"git\").arg(\"status\").output();",          // 2
            "}",                                                                            // 3
            "fn b() {",                                                                     // 4
            "    let o = std::process::Command::new(",                                      // 5
            "        crate::push_lock::system_git(),",                                      // 6
            "    );",                                                                       // 7
            "    let p = crate::spawn::async_command(\"git.exe\");",                        // 8
            "    let q = crate::spawn::async_command(crate::spawn::program(\"git\"));",      // 9
            "}",                                                                            // 10
            "fn c() {",                                                                     // 11
            "    let o = crate::git_exec::GitCmd::without_repo().with_program(system_git());", // 12
            "    let s = crate::spawn::command(\"ssh\");",                                  // 13
            "    let d = crate::spawn::async_command(crate::spawn::program(bin));",         // 14
            "    #[cfg(test)]",                                                             // 15
            "    if let Some(x) = y {",                                                     // 16
            "        crate::spawn::command(\"git\");",                                      // 17
            "    }",                                                                        // 18
            "}",                                                                            // 19
            "// crate::spawn::command(\"git\") in a comment",                               // 20
            "#[cfg(all(test, windows))]",                                                   // 21
            "fn helper() { crate::spawn::command(\"git\"); }",                              // 22
            "#[cfg(test)]",                                                                 // 23
            "mod tests {",                                                                  // 24
            "    fn t() {",                                                                 // 25
            "        crate::spawn::command(\"git\").arg(\"init\");",                        // 26
            "    }",                                                                        // 27
            "}",                                                                            // 28
            "fn d() { let _ = std::process::Command::new(\"git\"); }",                      // 29
            "fn e() { let _ = GitCmd::without_repo().with_program(crate::spawn::program(\"git\")); }", // 30
            "fn f() { let _ = GitCmd::without_repo().with_program(&spawn::program(\"git\")); }", // 31
        ]
        .join("\n");
        assert_eq!(direct_git_spawns(&src), vec![2, 5, 8, 9, 29]);
    }

    /// Every git process the app starts goes through `GitCmd`: its hardened
    /// environment, its clock, its command log, and whatever is pinned there
    /// next. Exceptions live outside this crate's `src` (the elevated
    /// lock-helper, the git-shim that runs the person's own commands) or are
    /// compiled into the test binary only (`probe.rs`).
    #[test]
    fn every_git_the_app_starts_goes_through_gitcmd() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("the sources").flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 20, "the sources were found: {}", files.len());
        let mut found = Vec::new();
        for f in &files {
            let rel = f.strip_prefix(&root).unwrap_or(f);
            if rel == std::path::Path::new("git_exec.rs") || rel == std::path::Path::new("probe.rs") {
                continue;
            }
            let src = std::fs::read_to_string(f).expect("a source file");
            found.extend(direct_git_spawns(&src).into_iter().map(|n| format!("{}:{}", rel.display(), n)));
        }
        assert!(found.is_empty(), "git started outside git_exec::GitCmd (use GitCmd, `run_blocking` from code that is not async): {:?}", found);
    }

    /// `run_blocking` is `run`: the hardened environment, the command log,
    /// and the clock that stops a command that never ends.
    #[test]
    fn the_blocking_entry_point_is_run_itself() {
        let dir = std::env::temp_dir();
        let before = crate::cmdlog::since(0, None).last().map_or(0, |e| e.id);
        // An alias of this test's own: other tests run SHOW_ENV at the same
        // time, and their log entries must not stand in for this one.
        let out = super::GitCmd::at(&dir).args(["-c", "alias.gitswitch-blocking-log=!env", "gitswitch-blocking-log"]).run_blocking().expect("git runs");
        assert!(out.ok(), "{}", out.stderr);
        super::test_support::assert_hardened(&out.text());
        assert!(
            crate::cmdlog::since(before, None).iter().any(|e| e.argv.iter().any(|a| a == "gitswitch-blocking-log")),
            "it is in the command log like any other command"
        );

        let started = std::time::Instant::now();
        let err = super::GitCmd::at(&dir)
            .cfg("alias.gitswitch-hang", "!sleep 30")
            .args(["gitswitch-hang"])
            .timeout(std::time::Duration::from_millis(500))
            .run_blocking()
            .err()
            .expect("a command that never ends is stopped");
        assert!(err.to_string().contains("was stopped after"), "{}", err);
        assert!(started.elapsed() < std::time::Duration::from_secs(15), "stopped on the clock: {:?}", started.elapsed());
    }

    /// A stop on the clock is told apart from git's own answer, whatever
    /// git happened to print.
    #[test]
    fn a_stop_on_the_clock_is_told_apart_from_a_failure() {
        let stopped = |m: String| super::was_stopped(&crate::error::AppError::Command(m));
        assert!(stopped(super::timeout_message("rev-list", super::LOCAL_TIMEOUT, "")));
        assert!(stopped(super::timeout_message("fetch", super::NET_TIMEOUT, "Receiving objects")));
        assert!(stopped(super::timeout_message("lfs", super::LFS_TIMEOUT, "")));
        assert!(stopped("git fetch made no progress for 30s and was stopped — the remote may be unreachable, or the connection very slow.".into()));
        assert!(!stopped("fatal: your current branch 'main' does not have any commits yet".into()));
        assert!(!stopped("git log failed: fatal: bad revision 'x'".into()));
        assert!(!super::was_stopped(&crate::error::AppError::Config("git rev-list was stopped after 60s".into())));
    }

    /// The prelude's own refusals are told apart from git's answer too, each
    /// of the ways it words one.
    #[test]
    fn a_refusal_of_the_prelude_is_told_apart_from_a_failure() {
        let keys = vec!["core.sshCommand".to_string()];
        let why = super::ReadFailed { why: "git config was stopped after 30s".into(), refused: false };
        for e in [super::partial_refusal("rev-list", &keys), super::unswitchable("status", "filter.\u{fffd}.clean"), super::fetch_refusal(&keys), super::fetch_read_failed(&why)] {
            assert!(super::was_refused(&e), "{}", e);
            assert!(!super::was_stopped(&e), "{}", e);
        }
        assert!(!super::was_refused(&crate::error::AppError::Command("fatal: your current branch 'main' does not have any commits yet".into())));
        assert!(!super::was_refused(&crate::error::AppError::Command(super::timeout_message("rev-list", super::LOCAL_TIMEOUT, ""))));
        assert!(!super::was_refused(&crate::error::AppError::Config("GitSwitch did not run git log".into())));
    }

    /// A blocking helper called from an async command: no panic (as
    /// `block_on` inside a runtime would), no wait on itself.
    #[tokio::test]
    async fn the_blocking_entry_point_works_inside_a_runtime() {
        let out = super::GitCmd::without_repo().arg("--version").run_blocking().expect("git runs");
        assert!(out.text().starts_with("git version"), "{}", out.text());
    }

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

    #[test]
    fn only_a_full_object_id_counts_as_one() {
        assert!(is_object_id(&"a".repeat(40)) && is_object_id(&"0123456789abcdefABCDEF".repeat(3)[..64]));
        for bad in [
            String::new(),
            "abc1234".into(),
            "a".repeat(39),
            "a".repeat(41),
            "a".repeat(65),
            "--output=canary.txt".into(),
            format!("-{}", "a".repeat(39)),
            format!("{} ", "a".repeat(39)),
            "g".repeat(40),
            // 40 bytes, but not 40 ASCII digits.
            format!("é{}", "a".repeat(38)),
        ] {
            assert!(!is_object_id(&bad), "{bad:?} is not an object id");
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

    // ---- the safety prelude --------------------------------------------

    fn sh(args: &[&str]) -> Shape {
        shape(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), false)
    }

    #[test]
    fn reads_and_writes_are_told_apart_including_the_lookalikes() {
        let passive = |a: &[&str]| assert_eq!(sh(a).class, Class::Passive, "{:?} only reads", a);
        let write = |a: &[&str]| assert_eq!(sh(a).class, Class::Write, "{:?} is a write", a);
        // config: a read, or a value to set
        passive(&["config", "--get", "user.email"]);
        write(&["config", "user.email", "a@b.test"]);
        passive(&["config", "--show-origin", "user.email"]);
        passive(&["config", "--local", "-z", "--get-regexp", "^remote"]);
        passive(&["config", "-f", ".gitmodules", "--list"]);
        passive(&["config", "get", "user.email"]);
        write(&["config", "set", "user.email", "a@b.test"]);
        write(&["config", "--local", "--unset", "core.hooksPath"]);
        write(&["config", "-f", ".gitmodules", "submodule.a.url", "x"]);
        // 2.46's subcommands: a lone word is not a name.
        write(&["config", "edit"]);
        write(&["config", "unset", "user.email"]);
        // stash list / show against push and the rest
        passive(&["stash", "list", "--format=%gd"]);
        passive(&["stash", "show", "-p", "stash@{0}"]);
        write(&["stash", "push", "-m", "x"]);
        write(&["stash"]);
        write(&["stash", "drop"]);
        // symbolic-ref: a name alone reads, a value or --delete writes
        passive(&["symbolic-ref", "HEAD"]);
        passive(&["symbolic-ref", "-q", "--short", "HEAD"]);
        write(&["symbolic-ref", "HEAD", "refs/heads/x"]);
        write(&["symbolic-ref", "-m", "why", "HEAD", "refs/heads/x"]);
        write(&["symbolic-ref", "--delete", "HEAD"]);
        // submodule status / update
        passive(&["submodule", "status"]);
        passive(&["submodule"]);
        write(&["submodule", "update", "--init"]);
        write(&["submodule", "--quiet", "foreach", "true"]);
        // lfs ls-files / pull
        passive(&["lfs", "ls-files", "-l"]);
        passive(&["lfs", "version"]);
        write(&["lfs", "pull"]);
        write(&["lfs", "install", "--local"]);
        // branch --list / a new branch
        passive(&["branch", "--list"]);
        passive(&["branch", "--show-current"]);
        write(&["branch", "x"]);
        write(&["branch", "-D", "x"]);
        write(&["branch", "--list", "-d", "x"]);
        write(&["branch", "--set-upstream-to=origin/main"]);
        // remote: the list and get-url; nothing that changes or calls out
        passive(&["remote"]);
        passive(&["remote", "-v"]);
        passive(&["remote", "get-url", "--push", "origin"]);
        write(&["remote", "add", "o", "https://example.test/r"]);
        write(&["remote", "show", "origin"]);
        passive(&["credential", "fill"]);
        write(&["credential", "approve"]);
        passive(&["worktree", "list"]);
        write(&["worktree", "add", "x"]);
        for a in [&["status"][..], &["--no-optional-locks", "status"], &["diff", "--numstat"], &["log", "-1"], &["rev-parse", "HEAD"], &["--version"], &["ls-files", "-s"], &["cat-file", "--batch"], &["for-each-ref"]] {
            passive(a);
        }
        // Everything else, unknown commands and aliases included, is a write.
        for a in [&["fetch", "origin"][..], &["commit", "-m", "x"], &["add", "."], &["checkout", "x"], &["merge-tree", "--write-tree", "a", "b"], &["frobnicate"], &["-c", "alias.x=!sh", "x"], &["--frobnicate", "status"], &[]] {
            write(a);
        }
        // The History fetch is passive whatever its subcommand.
        let fetch: Vec<String> = ["fetch", "--progress", "--", "origin"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shape(&fetch, true).class, Class::Passive);
        assert_eq!(shape(&fetch, true).read, Read::HistoryFetch);
    }

    #[test]
    fn a_passive_command_reads_the_config_only_where_a_program_can_be_reached() {
        assert_eq!(sh(&["status", "--porcelain"]).read, Read::Neutral);
        assert_eq!(sh(&["diff", "--cached", "--numstat"]).read, Read::Neutral);
        assert_eq!(sh(&["diff-tree", "-r", "a", "b"]).read, Read::Neutral);
        assert_eq!(sh(&["stash", "show", "-p"]).read, Read::Neutral);
        // A format of its own: only whether a partial clone could fetch.
        assert_eq!(sh(&["log", "-1", "--format=%H %s"]).read, Read::Objects);
        assert_eq!(sh(&["log", "--no-show-signature", "-1", "--pretty=format:%H"]).read, Read::Objects);
        assert_eq!(sh(&["log", "--oneline", "-1"]).read, Read::Objects);
        // No format of its own: the repository's `format.pretty` may ask
        // for the signature (hostile.sh shows it running gpg.program).
        assert_eq!(sh(&["log", "-1"]).read, Read::Neutral);
        assert_eq!(sh(&["show", "-s", "HEAD"]).read, Read::Neutral);
        assert_eq!(sh(&["show", "-s", "--format=%H%x1f%G?%x1f%s"]).read, Read::Neutral);
        assert_eq!(sh(&["log", "-1", "--show-signature"]).read, Read::Neutral);
        assert_eq!(sh(&["rev-list", "-1", "--format=%GK", "HEAD"]).read, Read::Neutral);
        assert_eq!(sh(&["show", "--textconv", "HEAD"]).read, Read::Neutral);
        // Every listing may read trees: a sparse index is expanded first.
        for a in [&["ls-files", "-s"][..], &["ls-files", "-s", "-z", ":/"], &["ls-files", "-z"], &["ls-files", "-z", "-t"], &["ls-files", "-u", "-z"], &["ls-files", "-s", "--", "x"], &["ls-files", "-o", "--exclude-standard"]] {
            assert_eq!(sh(a).read, Read::Objects, "{:?}", a);
        }
        assert_eq!(sh(&["ls-files", "-zm"]).read, Read::Neutral);
        // A path named like an option is a path.
        assert_eq!(sh(&["ls-files", "-s", "--", "-m"]).read, Read::Objects);
        // Text after `--` or `--end-of-options` is never an option to git:
        // a path or revision from the repository switches nothing off.
        assert_eq!(sh(&["log", "--end-of-options", "--oneline"]).read, Read::Neutral);
        assert_eq!(sh(&["log", "-1", "--", "--format=%H"]).read, Read::Neutral);
        assert_eq!(sh(&["log", "--oneline", "--end-of-options", "HEAD"]).read, Read::Objects);
        assert_eq!(sh(&["cat-file", "-p", "--", "--filters"]).read, Read::Objects);
        assert_eq!(sh(&["describe", "--end-of-options", "--dirty"]).read, Read::Objects);
        // Objects read, no program-naming key reached.
        for a in [&["rev-list", "--count", "HEAD"][..], &["for-each-ref", "--format=%(refname)"], &["cat-file", "--batch"], &["ls-tree", "HEAD"], &["merge-base", "a", "b"], &["describe", "--tags"], &["stash", "list"], &["shortlog", "-s"], &["branch", "--list"], &["submodule", "status"], &["rev-parse", "HEAD~1"], &["rev-parse", "--verify", "HEAD^{commit}"], &["rev-parse", "HEAD:a.txt"], &["check-attr", "-a", "x"]] {
            assert_eq!(sh(a).read, Read::Objects, "{:?}", a);
        }
        // Nothing read at all.
        for a in [&["rev-parse", "HEAD"][..], &["rev-parse", "--show-toplevel"], &["rev-parse", "--git-path", "hooks"], &["config", "--get", "user.email"], &["symbolic-ref", "HEAD"], &["remote", "-v"], &["branch", "--show-current"], &["worktree", "list"], &["--version"], &["lfs", "version"]] {
            assert_eq!(sh(a).read, Read::None, "{:?}", a);
        }
        // `lfs ls-files` diffs the work tree through git: its filters, and
        // a status in every submodule.
        assert_eq!(sh(&["lfs", "ls-files", "--json"]).read, Read::Neutral);
        assert!(sh(&["lfs", "ls-files", "--json"]).recurse);
        assert_eq!(sh(&["describe", "--dirty"]).read, Read::Neutral);
        assert_eq!(sh(&["credential", "fill"]).read, Read::Credential);
        // Where the diff flags go: right after the subcommand.
        assert_eq!(sh(&["diff", "--numstat"]).diff_flags_at, Some(1));
        assert_eq!(sh(&["--no-optional-locks", "diff"]).diff_flags_at, Some(2));
        assert_eq!(sh(&["stash", "show", "-p"]).diff_flags_at, Some(2));
        assert_eq!(sh(&["status"]).diff_flags_at, None);
        // Looking inside the submodules' work trees: a status or a diff
        // against the work tree, unless told to ignore them.
        for a in [&["status", "--porcelain"][..], &["status", "--ignore-submodules=untracked"], &["status", "--ignore-submodules=none"], &["diff", "--numstat"], &["diff", "HEAD", "--", "x"], &["diff-files", "-q"], &["diff-index", "HEAD"], &["describe", "--dirty"], &["lfs", "status"]] {
            assert!(sh(a).recurse, "{:?} looks inside submodules", a);
        }
        // A submodule at the path `--cached` (or any option's name) is a path.
        for a in [&["diff", "--no-color", "--ignore-submodules=none", "--", "--cached"][..], &["diff", "--", "--ignore-submodules"], &["diff", "--", "--staged", "--no-index"], &["diff-index", "HEAD", "--", "--cached"], &["status", "--porcelain", "--", "--ignore-submodules=all"]] {
            assert!(sh(a).recurse, "{:?} looks inside submodules", a);
        }
        for a in [&["status", "--ignore-submodules=all"][..], &["status", "--ignore-submodules"], &["status", "--ignore-submodules=dirty"], &["diff", "--cached"], &["diff", "--staged", "--numstat"], &["diff", "--no-index", "--", "a", "b"], &["diff-index", "--cached", "HEAD"], &["diff-tree", "-r", "a", "b"], &["stash", "show", "-p"], &["log", "-1"], &["describe"], &["commit", "-m", "x"]] {
            assert!(!sh(a).recurse, "{:?} does not look inside submodules", a);
        }
        // The hook pin, but not where the app asks where hooks live.
        assert!(sh(&["status"]).hooks_pin);
        assert!(!sh(&["rev-parse", "--git-path", "hooks"]).hooks_pin);
        assert!(!sh(&["config", "--get", "core.hooksPath"]).hooks_pin);
    }

    #[test]
    fn the_config_listing_is_read_with_its_scopes() {
        let raw = b"global\0user.name\nAnn\0local\0filter.x.clean\n/tmp/x\0local\0z.flag\0command\0core.fsmonitor\n\0";
        let got = parse_config_list(raw);
        let e = |scope: &str, key: &str, value: Option<&str>| ConfigEntry { scope: scope.into(), key: key.into(), value: value.map(Into::into), key_utf8: true };
        assert_eq!(got, vec![e("global", "user.name", Some("Ann")), e("local", "filter.x.clean", Some("/tmp/x")), e("local", "z.flag", None), e("command", "core.fsmonitor", Some(""))]);
        assert!(parse_config_list(b"").is_empty());
    }

    fn entry(scope: &str, key: &str, value: &str) -> ConfigEntry {
        ConfigEntry { scope: scope.into(), key: key.into(), value: Some(value.into()), key_utf8: true }
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn what_the_repository_names_is_switched_off_and_the_rest_is_kept() {
        let entries = vec![
            entry("global", "filter.crypt.clean", "/usr/bin/crypt clean"),
            entry("global", "gpg.program", "/opt/gpg2"),
            entry("global", "gpg.ssh.defaultkeycommand", "ssh-add -L"),
            entry("local", "filter.lfs.clean", "git-lfs clean -- %f"),
            entry("local", "filter.lfs.smudge", "git-lfs smudge -- %f"),
            entry("local", "filter.lfs.process", "git-lfs filter-process"),
            entry("local", "filter.lfs.required", "true"),
            entry("local", "filter.evil.clean", "/tmp/evil-clean"),
            entry("worktree", "filter.evil.process", "/tmp/evil-process"),
            entry("local", "gpg.openpgp.program", "/tmp/evil-gpg"),
            entry("local", "gpg.ssh.program", "/tmp/evil-ssh"),
            entry("local", "gpg.x509.program", "/tmp/evil-x509"),
            entry("local", "gpg.ssh.defaultkeycommand", "/tmp/evil-key"),
            entry("local", "diff.evil.textconv", "/tmp/evil-textconv"),
            entry("local", "diff.evil.command", "/tmp/evil-diff"),
            entry("local", "diff.external", "/tmp/evil-external"),
            entry("command", "core.fsmonitor", ""),
        ];
        let got = neutralising_cfg(&entries).expect("all expressible");
        assert_eq!(
            got,
            pairs(&[
                ("filter.evil.clean", ""),
                ("filter.evil.process", ""),
                ("filter.evil.required", "false"),
                ("gpg.program", "/opt/gpg2"),
                ("gpg.openpgp.program", "/opt/gpg2"),
                ("gpg.ssh.program", "ssh-keygen"),
                ("gpg.x509.program", "gpgsm"),
                ("gpg.ssh.defaultKeyCommand", "ssh-add -L"),
                ("diff.evil.textconv", ""),
                ("diff.evil.command", ""),
            ])
        );
        // The repository's last word counts: a hostile value it later
        // replaced with git-lfs's own is git-lfs's own.
        let replaced = vec![entry("local", "filter.lfs.clean", "/tmp/x"), entry("local", "filter.lfs.clean", "git-lfs clean -- %f"), entry("local", "filter.lfs.smudge", "git-lfs smudge --skip -- %f")];
        assert_eq!(neutralising_cfg(&replaced), Ok(Vec::new()));
        // git-lfs's values with something else in the "lfs" name are not git-lfs's.
        let odd = vec![entry("local", "filter.lfs.process", "git-lfs filter-process; /tmp/x")];
        assert_eq!(neutralising_cfg(&odd), Ok(pairs(&[("filter.lfs.process", ""), ("filter.lfs.required", "false")])));
        // An LFS extension is a program git-lfs's clean runs: then git-lfs's
        // own filter is switched off too.
        let ext = vec![entry("local", "filter.lfs.clean", "git-lfs clean -- %f"), entry("local", "lfs.extension.foo.clean", "/tmp/x %f")];
        assert_eq!(neutralising_cfg(&ext), Ok(pairs(&[("filter.lfs.clean", ""), ("filter.lfs.smudge", ""), ("filter.lfs.process", ""), ("filter.lfs.required", "false")])));
        // ... and so it is when git-lfs's filter is the person's own (`git lfs
        // install`, Git for Windows' system config), as it usually is.
        let theirs = vec![entry("system", "filter.lfs.process", "git-lfs filter-process"), entry("system", "filter.lfs.required", "true"), entry("local", "lfs.extension.x.clean", "/tmp/x %f")];
        assert_eq!(neutralising_cfg(&theirs), Ok(pairs(&[("filter.lfs.clean", ""), ("filter.lfs.smudge", ""), ("filter.lfs.process", ""), ("filter.lfs.required", "false")])));
        // Without an extension, the person's git-lfs is left alone.
        assert_eq!(neutralising_cfg(&theirs[..2]), Ok(Vec::new()));
        // Defaults when the person's own config names no program.
        let bare = vec![entry("local", "gpg.program", "/tmp/x"), entry("local", "gpg.ssh.defaultkeycommand", "/tmp/y")];
        assert_eq!(neutralising_cfg(&bare), Ok(pairs(&[("gpg.program", "gpg"), ("gpg.openpgp.program", "gpg"), ("gpg.ssh.defaultKeyCommand", "")])));
        // Nothing local, nothing to do.
        assert_eq!(neutralising_cfg(&[entry("global", "filter.x.clean", "/usr/bin/x"), entry("system", "diff.y.textconv", "z")]), Ok(Vec::new()));
        // A name `-c` cannot carry is refused, never passed half-read.
        assert_eq!(neutralising_cfg(&[entry("local", "filter.a=b.clean", "/tmp/x")]), Err("filter.a=b.clean".to_string()));
        // So is a filter or driver name that is not UTF-8: decoded, it would
        // name another filter, and git would run this one.
        let bytes = parse_config_list(b"local\0filter.\xff.clean\n/tmp/x\0");
        assert!(!bytes[0].key_utf8);
        assert!(neutralising_cfg(&bytes).is_err(), "{:?}", bytes);
        assert!(neutralising_cfg(&parse_config_list(b"worktree\0diff.\xfe.textconv\n/tmp/x\0")).is_err());
        // In the person's own scopes it is theirs, and left alone.
        assert_eq!(neutralising_cfg(&parse_config_list(b"global\0filter.\xff.clean\n/usr/bin/x\0")), Ok(Vec::new()));
        // Any UTF-8 name is passed as it is.
        let accented = parse_config_list("local\0filter.caf\u{e9}.clean\n/tmp/x\0".as_bytes());
        assert!(accented[0].key_utf8);
        assert_eq!(neutralising_cfg(&accented), Ok(pairs(&[("filter.caf\u{e9}.clean", ""), ("filter.caf\u{e9}.required", "false")])));
    }

    /// A command limited to plain paths looks inside only the submodules
    /// they reach; anything that could match more checks every one.
    #[test]
    fn only_the_submodules_a_plain_path_reaches_are_checked() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pathspec_prefixes(&[], &v(&["diff", "--no-color", "--", "a.txt"])), Some(v(&["a.txt"])));
        assert_eq!(pathspec_prefixes(&[], &v(&["diff", "A", "B", "--", ":(literal)dir/x*", "./lib/"])), Some(v(&["dir/x*", "lib"])));
        assert_eq!(pathspec_prefixes(&v(&["--literal-pathspecs"]), &v(&["diff", "--", "we[i]rd?"])), Some(v(&["we[i]rd?"])));
        for a in [&["status", "--porcelain"][..], &["diff", "--", "*.txt"], &["diff", "--", ":(glob)**/x"], &["diff", "--", ":/x"], &["diff", "--", "a\\b"], &["diff", "--"]] {
            assert_eq!(pathspec_prefixes(&[], &v(a)), None, "{:?}", a);
        }
        assert!(pathspec_reaches("lib", "lib"));
        assert!(pathspec_reaches("vendor", "vendor/lib"));
        assert!(pathspec_reaches("vendor/lib/x.c", "vendor/lib"));
        assert!(pathspec_reaches("LIB", "lib"));
        assert!(pathspec_reaches("", "lib") && pathspec_reaches(".", "lib"));
        assert!(!pathspec_reaches("a.txt", "lib"));
        assert!(!pathspec_reaches("src/x", "vendor/lib"));
    }

    #[test]
    fn a_partial_clone_is_told_by_its_promisor_settings() {
        assert!(partial_clone(&[entry("local", "extensions.partialclone", "origin")]));
        assert!(partial_clone(&[entry("worktree", "remote.origin.promisor", "true")]));
        assert!(partial_clone(&[entry("global", "remote.origin.promisor", "true")]));
        assert!(!partial_clone(&[entry("local", "remote.origin.url", "https://example.test/r"), entry("local", "remote.origin.partialclonefilter", "blob:none")]));
        let msg = partial_refusal("diff", &["core.sshCommand".to_string()]).to_string();
        assert!(msg.contains("partial clone") && msg.contains("core.sshCommand") && msg.contains("terminal") && msg.contains("profile"), "{}", msg);
    }

    /// The repository found from a folder, the shortcut that tells a
    /// repository cannot be a partial clone, and the one that tells its
    /// index holds no gitlink, against real git: each says "ask git" when
    /// unsure.
    #[test]
    fn the_folder_shortcuts_find_the_repository_and_never_guess() {
        let dir = std::env::temp_dir().join(format!("gitswitch-prelude-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("r");
        std::fs::create_dir_all(repo.join("deep").join("er")).expect("a folder");
        assert!(fixture_git(&repo, &["init", "-q"]).status.success());
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        assert!(fixture_git(&repo, &["add", "-A"]).status.success());
        assert!(fixture_git(&repo, &["commit", "-qm", "a"]).status.success());
        let dirs = find_git_dirs(&repo.join("deep").join("er")).expect("found from a folder inside");
        assert_eq!(dirs, GitDirs { git_dir: repo.join(".git"), common_dir: repo.join(".git") });
        assert!(!may_be_partial(&dirs));
        assert!(!index_may_hold_gitlinks(&dirs));
        // A promisor setting, or an include that could hold one.
        assert!(fixture_git(&repo, &["config", "remote.origin.Promisor", "true"]).status.success());
        assert!(may_be_partial(&dirs));
        assert!(fixture_git(&repo, &["config", "--unset", "remote.origin.promisor"]).status.success());
        assert!(!may_be_partial(&dirs));
        assert!(fixture_git(&repo, &["config", "include.path", "other.cfg"]).status.success());
        assert!(may_be_partial(&dirs));
        assert!(fixture_git(&repo, &["config", "--unset", "include.path"]).status.success());
        // A gitlink in the index.
        let head = String::from_utf8(fixture_git(&repo, &["rev-parse", "HEAD"]).stdout).unwrap();
        assert!(fixture_git(&repo, &["update-index", "--add", "--cacheinfo", &format!("160000,{},link", head.trim())]).status.success());
        assert!(index_may_hold_gitlinks(&dirs));
        // A linked work tree: its own folder, the shared one behind it.
        let wt = dir.join("wt");
        assert!(fixture_git(&repo, &["worktree", "add", "-q", "--detach", &wt.to_string_lossy()]).status.success());
        let wdirs = find_git_dirs(&wt).expect("a linked work tree");
        assert_eq!(wdirs.common_dir.canonicalize().unwrap(), repo.join(".git").canonicalize().unwrap());
        assert!(wdirs.git_dir.join("index").is_file(), "{:?}", wdirs);
        // An empty `.git` folder is not a repository git would take: no guess.
        std::fs::create_dir_all(repo.join("deep").join(".git")).unwrap();
        assert_eq!(find_git_dirs(&repo.join("deep").join("er")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder shortcuts read a file only when it is a regular one;
    /// anything else gets the cautious answer, so git (on its clock) decides.
    #[test]
    fn the_folder_shortcuts_read_only_regular_files() {
        let dir = std::env::temp_dir().join(format!("gitswitch-prelude-odd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // What git takes for a repository: HEAD, objects and refs.
        let gd = dir.join("r").join(".git");
        std::fs::create_dir_all(gd.join("objects")).unwrap();
        std::fs::create_dir_all(gd.join("refs")).unwrap();
        std::fs::write(gd.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let dirs = find_git_dirs(&dir.join("r")).expect("a repository");
        assert!(!may_be_partial(&dirs) && !index_may_hold_gitlinks(&dirs), "no config, no index: nothing to find");
        // A folder where the file should be.
        for name in ["config", "index", "commondir"] {
            std::fs::create_dir_all(gd.join(name)).unwrap();
            match name {
                "config" => assert!(may_be_partial(&dirs), "a folder named config"),
                "index" => assert!(index_may_hold_gitlinks(&dirs), "a folder named index"),
                _ => assert_eq!(find_git_dirs(&dir.join("r")), None, "a folder named commondir"),
            }
            std::fs::remove_dir_all(gd.join(name)).unwrap();
        }
        // A FIFO (a tar archive can carry one): opening it would wait for a
        // writer for good. Each answer must come back at once.
        #[cfg(unix)]
        {
            let fifo = |p: &Path| {
                let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {:?}", p);
            };
            let within = |what: &'static str, f: Box<dyn FnOnce() -> bool + Send>| {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(f());
                });
                rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or_else(|_| panic!("{} waited on a FIFO", what))
            };
            let r = dir.join("r");
            for (name, cautious) in [("config", true), ("index", true), ("commondir", false)] {
                fifo(&gd.join(name));
                let (gd2, r2) = (GitDirs { git_dir: gd.clone(), common_dir: gd.clone() }, r.clone());
                let answer = match name {
                    "config" => within("may_be_partial", Box::new(move || may_be_partial(&gd2))),
                    "index" => within("index_may_hold_gitlinks", Box::new(move || index_may_hold_gitlinks(&gd2))),
                    _ => within("find_git_dirs", Box::new(move || find_git_dirs(&r2).is_some())),
                };
                assert_eq!(answer, cautious, "a FIFO named {}", name);
                std::fs::remove_file(gd.join(name)).unwrap();
            }
            // A `.git` that is a FIFO: no repository is guessed.
            let w = dir.join("w");
            std::fs::create_dir_all(&w).unwrap();
            fifo(&w.join(".git"));
            assert!(!within("find_git_dirs on a .git FIFO", Box::new(move || find_git_dirs(&w).is_some())));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_persons_own_credential_helpers_are_put_back_in_their_order() {
        let entries = vec![
            entry("system", "credential.helper", "manager"),
            entry("global", "credential.helper", ""),
            entry("global", "credential.https://example.test.helper", "store"),
            entry("local", "credential.helper", "!/tmp/evil"),
            entry("worktree", "credential.https://example.test.helper", "/tmp/evil2"),
            entry("global", "credential.usehttppath", "true"),
            entry("global", "credential.https://a=b.helper", "skipped"),
        ];
        assert_eq!(
            credential_cfg(Some(&entries)),
            pairs(&[("credential.helper", ""), ("credential.helper", "manager"), ("credential.helper", ""), ("credential.https://example.test.helper", "store"), ("core.askPass", "")])
        );
        assert_eq!(credential_cfg(None), pairs(&[("credential.helper", ""), ("core.askPass", "")]));
    }

    #[test]
    fn the_history_fetch_is_refused_over_a_transport_program_the_repository_names() {
        let entries = vec![
            entry("global", "core.sshcommand", "ssh -i ~/.ssh/work"),
            entry("local", "core.sshcommand", "/tmp/evil"),
            entry("local", "remote.origin.uploadpack", "/tmp/evil"),
            entry("local", "remote.a.b.vcs", "evil"),
            entry("worktree", "lfs.customtransfer.x.path", "/tmp/evil"),
            entry("local", "core.gitproxy", "/tmp/evil"),
            entry("local", "remote.origin.url", "https://example.test/r"),
        ];
        let found = transport_programs(&entries);
        assert_eq!(found, vec!["core.sshCommand", "remote.origin.uploadpack", "remote.a.b.vcs", "lfs.customtransfer.x.path", "core.gitProxy"]);
        let msg = fetch_refusal(&found).to_string();
        assert!(msg.contains("core.sshCommand, remote.origin.uploadpack, remote.a.b.vcs, lfs.customtransfer.x.path and core.gitProxy"), "{}", msg);
        assert!(msg.contains("terminal") && msg.contains("profile"), "{}", msg);
        // The profile's own (global) ssh command is not a reason.
        assert!(transport_programs(&entries[..1]).is_empty());
    }

    /// L2: an inherited pathspec switch never reaches git; a caller's own
    /// value still does. (Never set in this process: other tests run plain git.)
    #[test]
    fn inherited_pathspec_switches_never_reach_git() {
        let env_of = |c: GitCmd| -> Vec<(String, Option<String>)> {
            let (cmd, _) = c.prepare();
            cmd.as_std().get_envs().map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string()))).collect()
        };
        let env = env_of(GitCmd::at("r").args(["rev-parse", "HEAD"]));
        for key in PATHSPEC_SWITCHES {
            assert!(env.contains(&(key.to_string(), None)), "{key} is removed: {env:?}");
        }
        let env = env_of(GitCmd::at("r").args(["rev-parse", "HEAD"]).env("GIT_LITERAL_PATHSPECS", "0"));
        assert!(env.contains(&("GIT_LITERAL_PATHSPECS".to_string(), Some("0".to_string()))), "{env:?}");
    }

    #[test]
    fn the_prelude_lands_where_git_reads_it() {
        let argv_env = |c: GitCmd| {
            let (cmd, argv) = c.prepare();
            let env: Vec<(String, Option<String>)> = cmd.as_std().get_envs().map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string()))).collect();
            (argv[1..].to_vec(), env)
        };
        let hooks = format!("core.hooksPath={}", empty_hooks_dir());
        let mut status = GitCmd::at("r").args(["status", "--porcelain"]);
        status.settled = true;
        status.neutral = pairs(&[("filter.evil.clean", "")]);
        let (argv, env) = argv_env(status);
        assert_eq!(argv, vec!["-C", "r", "-c", "core.fsmonitor=", "-c", "protocol.ext.allow=never", "-c", hooks.as_str(), "-c", "filter.evil.clean=", "status", "--porcelain"]);
        assert!(env.contains(&("GIT_OPTIONAL_LOCKS".into(), Some("0".into()))), "{:?}", env);
        assert!(env.contains(&("GIT_PAGER".into(), Some("cat".into()))), "{:?}", env);

        let mut diff = GitCmd::at("r").args(["diff", "--numstat"]);
        diff.settled = true;
        let (argv, _) = argv_env(diff);
        assert!(argv.ends_with(&["diff".into(), "--no-ext-diff".into(), "--no-textconv".into(), "--numstat".into()]));
        // A passive diff never runs a diff inside a submodule.
        assert!(argv.contains(&"diff.submodule=short".to_string()), "{:?}", argv);
        let mut show = GitCmd::at("r").args(["stash", "show", "-p"]);
        show.settled = true;
        assert!(argv_env(show).0.ends_with(&["stash".into(), "show".into(), "--no-ext-diff".into(), "--no-textconv".into(), "-p".into()]));

        // A caller's own `-c` comes after the prelude, the signature pin last.
        let mut fill = GitCmd::at("r").cfg("credential.interactive", "false").args(["credential", "fill"]);
        fill.settled = true;
        fill.neutral = credential_cfg(None);
        let (argv, _) = argv_env(fill);
        let at = |kv: &str| argv.iter().position(|a| a == kv).unwrap_or_else(|| panic!("{} missing from {:?}", kv, argv));
        assert!(at("credential.helper=") < at("core.askPass=") && at("core.askPass=") < at("credential.interactive=false"), "{:?}", argv);

        // A write: the two fixed pins and nothing of the passive class.
        let (argv, env) = argv_env(GitCmd::at("r").args(["commit", "-m", "x"]));
        assert_eq!(argv, vec!["-C", "r", "-c", "core.fsmonitor=", "-c", "protocol.ext.allow=never", "commit", "-m", "x"]);
        assert!(!env.iter().any(|(k, _)| k == "GIT_OPTIONAL_LOCKS"), "{:?}", env);
        assert!(env.contains(&("GIT_PAGER".into(), Some("cat".into()))), "{:?}", env);
        let mut log = GitCmd::at("r").args(["log", "-1", "--format=%H"]);
        log.settled = true;
        let (argv, _) = argv_env(log);
        assert_eq!(argv.last().map(String::as_str), Some("--format=%H"));
        assert!(argv.contains(&"log.showSignature=false".to_string()) && argv.contains(&"--no-textconv".to_string()), "{:?}", argv);
    }

    /// A marker program for the tests below: it notes that it ran, as
    /// `name`, in `mark`; `transform` is the shell text it ends with (a
    /// filter must answer with the content).
    fn marker(dir: &Path, mark: &Path, name: &str, transform: &str) -> String {
        let p = dir.join(format!("prog-{}.sh", name));
        let mark = mark.to_string_lossy().replace('\\', "/");
        std::fs::write(&p, format!("#!/bin/sh\necho \"{} $*\" >> '{}'\n{}\n", name, mark, transform)).expect("a marker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        p.to_string_lossy().replace('\\', "/")
    }

    fn plain_git(dir: &Path, args: &[&str]) -> std::process::Output {
        crate::spawn::command("git").arg("-C").arg(dir).args(args).env("GIT_OPTIONAL_LOCKS", "0").output().expect("git runs")
    }

    fn fired(mark: &Path) -> String {
        std::fs::read_to_string(mark).unwrap_or_default()
    }

    /// The empty hooks folder: the app's own data folder; without one, never
    /// a folder another user could create first (unix: `/dev/null`, below
    /// which git finds no hook; Windows: the per-user temp folder).
    #[test]
    fn without_a_data_folder_the_hooks_pin_names_no_shared_folder() {
        let base = std::env::temp_dir().join(format!("gitswitch-hooks-in-{}", std::process::id()));
        let got = hooks_dir_in(Some(base.clone()));
        assert_eq!(got, base.join("empty-hooks").to_string_lossy().replace('\\', "/"));
        assert!(base.join("empty-hooks").is_dir());
        let _ = std::fs::remove_dir_all(&base);
        #[cfg(unix)]
        assert_eq!(hooks_dir_in(None), "/dev/null");
        #[cfg(windows)]
        assert!(Path::new(&hooks_dir_in(None)).starts_with(std::env::temp_dir()), "{}", hooks_dir_in(None));
    }

    /// git finds no hook below `/dev/null`, and still does its work.
    #[cfg(unix)]
    #[test]
    fn git_runs_no_hook_below_dev_null() {
        let dir = std::env::temp_dir().join(format!("gitswitch-hooks-devnull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        assert!(fixture_git(&dir, &["init", "-q"]).status.success());
        let mark = dir.join("MARK");
        let hook = dir.join(".git").join("hooks").join("reference-transaction");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, format!("#!/bin/sh\necho hook >> '{}'\n", mark.to_string_lossy())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let with = |pin: &str| {
            let _ = std::fs::remove_file(&mark);
            let out = plain_git(&dir, &["-c", pin, "-c", "user.name=x", "-c", "user.email=x@x.test", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "x"]);
            (out.status.success(), fired(&mark))
        };
        let hooks = format!("core.hooksPath={}", hook.parent().unwrap().to_string_lossy());
        // How often git runs reference-transaction for one commit depends on its
        // version (CI's git ran it 7 times), so the control only asks that it ran.
        let (made, fired_by) = with(&hooks);
        assert!(made && fired_by.contains("hook"), "control: the hook runs from its own folder: {:?}", fired_by);
        assert_eq!(with("core.hooksPath=/dev/null"), (true, String::new()), "no hook below /dev/null, and the commit is made");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Includes keep the scope of the file that pulled them in: a filter a
    /// local `include.path` (or `includeIf`) names is the repository's, and is
    /// switched off.
    #[tokio::test]
    async fn a_filter_a_local_include_pulls_in_is_the_repositorys() {
        let dir = std::env::temp_dir().join(format!("gitswitch-prelude-include-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        assert!(plain_git(&dir, &["init", "-q"]).status.success());
        let git_dir = dir.join(".git");
        std::fs::write(git_dir.join("inc.cfg"), "[filter \"inc\"]\n\tclean = /tmp/from-include\n").unwrap();
        std::fs::write(git_dir.join("incif.cfg"), "[gpg]\n\tprogram = /tmp/from-includeif\n").unwrap();
        // `gitdir:**/<folder>/` covers `<folder>/.git` wherever the temp folder
        // really is: on CI it is reached through an 8.3 short name (Windows) or a
        // symlink (/var is /private/var on macOS), which a full-path rule misses.
        let gitdir_rule = format!("**/{}", dir.file_name().unwrap().to_string_lossy());
        let mut config = std::fs::read_to_string(git_dir.join("config")).unwrap();
        config.push_str(&format!("[include]\n\tpath = inc.cfg\n[includeIf \"gitdir:{}/\"]\n\tpath = incif.cfg\n", gitdir_rule));
        std::fs::write(git_dir.join("config"), config).unwrap();
        let entries = repo_config(None, Some(dir.clone()), Vec::new()).await.expect("the config reads");
        let scope_of = |key: &str| entries.iter().find(|e| e.key == key).map(|e| e.scope.clone());
        assert_eq!(scope_of("filter.inc.clean").as_deref(), Some("local"));
        assert_eq!(scope_of("gpg.program").as_deref(), Some("local"));
        let pairs = neutralising_cfg(&entries).expect("expressible");
        assert!(pairs.contains(&("filter.inc.clean".to_string(), String::new())), "{:?}", pairs);
        assert!(pairs.iter().any(|(k, v)| k == "gpg.program" && v != "/tmp/from-includeif"), "{:?}", pairs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// git with no hook of the person's and the file transport allowed: for
    /// building the fixtures below (a real `~/.gitconfig` may name a hooks
    /// folder whose guards would refuse these commits).
    fn fixture_git(dir: &Path, args: &[&str]) -> std::process::Output {
        let hooks = format!("core.hooksPath={}", dir.join("no-hooks").to_string_lossy().replace('\\', "/"));
        let mut all: Vec<&str> = vec!["-c", &hooks, "-c", "protocol.file.allow=always", "-c", "user.name=x", "-c", "user.email=x@x.test", "-c", "commit.gpgsign=false"];
        all.extend_from_slice(args);
        crate::spawn::command("git").arg("-C").arg(dir).args(&all).env("GIT_OPTIONAL_LOCKS", "0").output().expect("git runs")
    }

    /// A passive status or diff in a superproject makes git run `git status`
    /// inside each downloaded submodule, which reads that submodule's own
    /// config: its filters (and a nested submodule's, and a gitlink's that
    /// `.gitmodules` never maps) are switched off too. Told to ignore the
    /// submodules, the command does not read them at all.
    #[tokio::test]
    async fn a_submodules_own_filters_are_switched_off_too() {
        let dir = std::env::temp_dir().join(format!("gitswitch-prelude-submodules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        let mark = dir.join("MARK");
        let source = |name: &str, filter: &str| {
            let src = dir.join(name);
            std::fs::create_dir_all(&src).unwrap();
            assert!(fixture_git(&src, &["init", "-q"]).status.success());
            std::fs::write(src.join(format!("{}.dat", name)), "data\n").unwrap();
            std::fs::write(src.join(".gitattributes"), format!("*.dat filter={}\n", filter)).unwrap();
            assert!(fixture_git(&src, &["add", "-A"]).status.success());
            assert!(fixture_git(&src, &["commit", "-qm", name]).status.success());
            src
        };
        let nest_src = source("nest", "nestevil");
        let sub_src = source("sub", "subevil");
        assert!(fixture_git(&sub_src, &["submodule", "add", "-q", &nest_src.to_string_lossy(), "nest"]).status.success());
        assert!(fixture_git(&sub_src, &["commit", "-qm", "nest"]).status.success());
        let top = dir.join("top");
        std::fs::create_dir_all(&top).unwrap();
        assert!(fixture_git(&top, &["init", "-q"]).status.success());
        std::fs::write(top.join("a.txt"), "a\n").unwrap();
        assert!(fixture_git(&top, &["add", "-A"]).status.success());
        assert!(fixture_git(&top, &["commit", "-qm", "a"]).status.success());
        assert!(fixture_git(&top, &["submodule", "add", "-q", &sub_src.to_string_lossy(), "sub"]).status.success());
        assert!(fixture_git(&top, &["commit", "-qm", "sub"]).status.success());
        assert!(fixture_git(&top, &["submodule", "update", "--init", "--recursive", "-q"]).status.success());
        // A gitlink no `.gitmodules` maps, with a clone behind it: git looks
        // inside it all the same.
        assert!(fixture_git(&dir, &["clone", "-q", &sub_src.to_string_lossy(), &top.join("planted").to_string_lossy()]).status.success());
        let planted_head = String::from_utf8(fixture_git(&top.join("planted"), &["rev-parse", "HEAD"]).stdout).unwrap();
        assert!(fixture_git(&top, &["update-index", "--add", "--cacheinfo", &format!("160000,{},planted", planted_head.trim())]).status.success());
        for (repo, filter, name) in [(top.join("sub"), "subevil", "subclean"), (top.join("sub").join("nest"), "nestevil", "nestclean"), (top.join("planted"), "subevil", "plantedclean")] {
            let prog = marker(&dir, &mark, name, "cat");
            assert!(plain_git(&repo, &["config", &format!("filter.{}.clean", filter), &prog]).status.success());
        }
        // The same bytes again (checkout may have written CRLF): a size that
        // changed tells git the file changed without reading it, and then no
        // filter runs, pinned or not.
        let stat_dirty = || {
            std::thread::sleep(Duration::from_millis(1100));
            for f in [top.join("sub").join("sub.dat"), top.join("sub").join("nest").join("nest.dat"), top.join("planted").join("sub.dat")] {
                let same = std::fs::read(&f).unwrap();
                std::fs::write(&f, same).unwrap();
            }
        };
        stat_dirty();
        let _ = std::fs::remove_file(&mark);

        let path = top.to_string_lossy().to_string();
        let status = crate::git_status::repo_status(&path).await;
        assert!(status.is_ok(), "{:?}", status.err());
        let out = GitCmd::at(&top).args(["status", "--porcelain"]).run().await.expect("status runs");
        assert!(out.ok(), "{}", out.stderr);
        let diff = GitCmd::at(&top).args(["diff", "--numstat"]).run().await.expect("diff runs");
        assert!(diff.ok(), "{}", diff.stderr);
        assert_eq!(fired(&mark), "", "a submodule's own filter ran");

        // Told to ignore the submodules, the command reads none of them.
        let mut ignoring = GitCmd::at(&top).args(["status", "--porcelain", "--ignore-submodules=all"]);
        Box::pin(ignoring.settle()).await.expect("settles");
        assert!(!ignoring.neutral.iter().any(|(k, _)| k.starts_with("filter.subevil")), "{:?}", ignoring.neutral);
        let mut looking = GitCmd::at(&top).args(["status", "--porcelain"]);
        Box::pin(looking.settle()).await.expect("settles");
        for key in ["filter.subevil.clean", "filter.subevil.required", "filter.nestevil.clean", "filter.nestevil.required"] {
            assert!(looking.neutral.iter().any(|(k, _)| k == key), "{} missing from {:?}", key, looking.neutral);
        }

        // The positive control: plain git runs all three.
        stat_dirty();
        plain_git(&top, &["status", "--porcelain"]);
        let said = fired(&mark);
        for name in ["subclean", "nestclean", "plantedclean"] {
            assert!(said.lines().any(|l| l.starts_with(name)), "plain git did not run {}: {}", name, said);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The passive entry points of the Repository page and the History tab,
    /// on a repository whose own config names a program for every key they
    /// can reach: every git they start is passive, and nothing the
    /// repository names runs. Plain git afterwards fires each marker, or the
    /// test would prove nothing.
    #[tokio::test]
    async fn passive_entry_points_run_passive_commands_and_nothing_the_repository_names() {
        let dir = std::env::temp_dir().join(format!("gitswitch-prelude-hostile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("r");
        std::fs::create_dir_all(&repo).expect("a folder");
        let mark = dir.join("MARK");
        assert!(plain_git(&repo, &["init", "-q"]).status.success());
        for (k, v) in [("user.name", "Lab"), ("user.email", "lab@example.test"), ("commit.gpgsign", "false")] {
            assert!(plain_git(&repo, &["config", k, v]).status.success());
        }
        // A clean filter that changes the content (a git-crypt stand-in):
        // committed through it, the blob differs from the file on disk.
        std::fs::write(repo.join(".gitattributes"), "*.dat filter=evil\n*.conv diff=evil\n").unwrap();
        let clean = marker(&dir, &mark, "clean", "sed 's/^/X/'");
        assert!(plain_git(&repo, &["config", "filter.evil.clean", &clean]).status.success());
        std::fs::write(repo.join("a.dat"), "aaaa\n").unwrap();
        std::fs::write(repo.join("c.conv"), "c\n").unwrap();
        assert!(fixture_git(&repo, &["add", "-A"]).status.success());
        assert!(fixture_git(&repo, &["commit", "-qm", "one"]).status.success());
        // A commit that claims a signature, for `%G?`.
        let raw = String::from_utf8(plain_git(&repo, &["cat-file", "commit", "HEAD"]).stdout).unwrap();
        let (head, body) = raw.split_once("\n\n").unwrap();
        let signed = format!("{}\ngpgsig -----BEGIN PGP SIGNATURE-----\n iQEzBAABCAAdFiEE\n -----END PGP SIGNATURE-----\n\n{}", head, body);
        let mut hash_object = crate::spawn::command("git").arg("-C").arg(&repo).args(["hash-object", "-t", "commit", "-w", "--stdin"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        {
            use std::io::Write;
            hash_object.stdin.take().unwrap().write_all(signed.as_bytes()).unwrap();
        }
        let hash = String::from_utf8(hash_object.wait_with_output().unwrap().stdout).unwrap().trim().to_string();
        assert_eq!(hash.len(), 40, "the signed-looking commit was written");
        for (k, name) in [("core.fsmonitor", "fsmonitor"), ("filter.evil.smudge", "smudge"), ("diff.evil.textconv", "textconv"), ("diff.evil.command", "diffcmd"), ("diff.external", "external"), ("gpg.program", "gpg")] {
            let prog = marker(&dir, &mark, name, if name == "smudge" { "cat" } else { "exit 1" });
            assert!(plain_git(&repo, &["config", k, &prog]).status.success());
        }
        let hook = repo.join(".git").join("hooks").join("post-index-change");
        std::fs::write(&hook, format!("#!/bin/sh\necho \"hook-post-index-change\" >> '{}'\n", mark.to_string_lossy().replace('\\', "/"))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
        }
        // Stat-dirty with the same bytes: git must read it again, through
        // its clean filter.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(repo.join("a.dat"), "aaaa\n").unwrap();
        std::fs::write(repo.join("c.conv"), "c\nd\n").unwrap();
        let _ = std::fs::remove_file(&mark);

        let path = repo.to_string_lossy().to_string();
        let status = crate::git_status::repo_status(&path).await;
        assert!(status.is_ok(), "{:?}", status.err());
        for file in ["a.dat", "c.conv"] {
            let diff = crate::git_status::file_diff(&path, file, false, false).await;
            assert!(diff.is_ok(), "{}: {:?}", file, diff.err());
        }
        let page = crate::git_history::history_page(&path, "HEAD", 0, 50, None, None, false, false);
        assert!(page.is_ok(), "{:?}", page.err());
        let detail = crate::git_history::commit_detail(&path, &hash);
        assert!(detail.is_ok(), "{:?}", detail.err());
        assert_eq!(fired(&mark), "", "a program the repository names ran");
        let writes: Vec<Vec<String>> = test_support::ran_under(&repo).into_iter().filter(|r| r.class == Class::Write).map(|r| r.args).collect();
        assert!(writes.is_empty(), "passive entry points ran writes: {:?}", writes);

        // The positive controls: plain git runs every one of them.
        plain_git(&repo, &["status", "--porcelain"]);
        plain_git(&repo, &["log", "-1", "--format=%G?", &hash]);
        plain_git(&repo, &["diff", "--", "c.conv"]);
        crate::spawn::command("git").arg("-C").arg(&repo).args(["diff", "--quiet"]).output().expect("git runs");
        let said = fired(&mark);
        for name in ["fsmonitor", "clean", "gpg", "diffcmd", "hook-post-index-change"] {
            assert!(said.lines().any(|l| l.starts_with(name)), "plain git did not run {}: {}", name, said);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
