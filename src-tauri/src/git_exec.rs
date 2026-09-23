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
    timeout: Duration,
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
            timeout: READ_TIMEOUT,
        }
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
        let mut cmd = tokio::process::Command::new(self.program.clone().unwrap_or_else(|| PathBuf::from("git")));
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

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match &self.stdin {
            Some(_) => cmd.stdin(Stdio::piped()),
            None => cmd.stdin(Stdio::null()),
        };
        // If the timeout fires we drop the child future; without this the git
        // process would outlive it.
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| AppError::Command(format!("Failed to run git: {}", e)))?;

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

        let secs = self.timeout.as_secs();
        let out = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| AppError::Command(format!("git {} failed: {}", sub, e)))?,
            Err(_) => {
                return Err(AppError::Command(format!(
                    "git {} timed out after {}s — the remote may be unreachable, or another git process is holding a lock",
                    sub, secs
                )))
            }
        };

        Ok(GitOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
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

/// A git remote name we are willing to pass to git. Rejects anything that could
/// be read as an option or a path.
pub fn validate_remote_name(name: &str) -> Result<(), AppError> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.contains(['/', '\\', ' ', '\t', '\n', ':', '?', '*', '[', '~', '^'])
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
        for bad in ["", "--upload-pack=x", "-o", "a/b", "a b", "a..b", "a:b"] {
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
}
