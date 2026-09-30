//! The pure part of GitSwitch's push lock, shared by the app and by the
//! privileged lock helper.
//!
//! Nothing here reads a file, spawns a process or looks at the environment.
//! The helper runs these validators and renderers as root, so each function is
//! small, total and unit-tested; the app runs the same renderers to know
//! exactly what the helper *should* have written, which is how tampering is
//! measured rather than guessed.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Bumped only when a job or registry changes shape incompatibly. The helper
/// refuses a job with another schema (exit 2) and a registry newer than itself
/// (exit 7).
pub const SCHEMA: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The URL scheme pushes get rewritten to. git has no such remote helper, so
/// the push fails before anything leaves the machine — unless our own helper
/// of that name is installed, whose only job is to explain the refusal.
pub const BLOCK_SCHEME: &str = "gitswitch-push-blocked://";
pub const REMOTE_HELPER_NAME: &str = "git-remote-gitswitch-push-blocked";
pub const HELPER_BIN_NAME: &str = "gitswitch-lock-helper";
pub const MARKER_BEGIN: &str =
    "# >>> GitSwitch push lock (managed; remove only via GitSwitch or its lock helper) >>>";
pub const MARKER_END: &str = "# <<< GitSwitch push lock <<<";
pub const MAX_JOB_BYTES: u64 = 64 * 1024;
pub const MAX_OPS: usize = 200;
pub const MAX_LOCKS: usize = 500;
pub const MAX_PREFIXES: usize = 64;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PREFIX_BYTES: usize = 2048;
pub const MAX_LABEL_CHARS: usize = 200;
/// The release public key the Tauri updater trusts (`plugins.updater.pubkey`
/// in tauri.conf.json): base64 of a whole minisign public-key file. Helper
/// upgrades are anchored to it; a test in the app crate keeps the two in step.
pub const RELEASE_PUBKEY_B64: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZFMTg4N0QwRDhEMjkxN0UKUldSK2tkTFkwSWNZL2pab0ZSbjIxemg3VnRUTTd6K0U1VGFROVBsNGRkMnpjWHRaaGxPdkVybmIK";

/// Process exit codes of the helper. The app maps them to sentences.
pub mod exit {
    /// Applied, or nothing needed doing (`changed` is empty).
    pub const OK: i32 = 0;
    /// Bad invocation, unreadable job, schema mismatch.
    pub const USAGE: i32 = 2;
    /// Refused by validation: hash mismatch, unmanaged path, squatted directory, bad signature.
    pub const REFUSED: i32 = 3;
    /// An I/O failure while applying; the result lists what did apply.
    pub const IO: i32 = 4;
    /// Not running with administrator rights.
    pub const NOT_ELEVATED: i32 = 6;
    /// The registry was written by a newer helper.
    pub const OUTDATED: i32 = 7;
}

// ---------------------------------------------------------------------------
// Platform and layout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub const fn current() -> Option<Platform> {
        if cfg!(target_os = "macos") {
            Some(Platform::Macos)
        } else if cfg!(target_os = "linux") {
            Some(Platform::Linux)
        } else if cfg!(target_os = "windows") {
            Some(Platform::Windows)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }
}

/// Where everything lives. Every path is admin-owned in real use; `root`
/// prefixes them all in the debug-only test mode so the suites can exercise
/// the helper without a prompt. Destinations never come from a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub platform: Platform,
    pub registry_dir: PathBuf,
    pub locks_json: PathBuf,
    pub include_file: PathBuf,
    pub stanza_dir: PathBuf,
    pub audit_log: PathBuf,
    pub backups_dir: PathBuf,
    pub helper_path: PathBuf,
    pub remote_helper_path: PathBuf,
    /// Fixed on macOS and Linux; on Windows derived from Git for Windows'
    /// InstallPath (`HKLM\SOFTWARE\GitForWindows`).
    pub system_gitconfig: PathBuf,
    pub polkit_policy: Option<PathBuf>,
}

/// Strip the leading `/` or `C:/` so a path can be re-rooted for tests.
fn relative_form(p: &str) -> String {
    let n = norm(p);
    if let Some(rest) = n.strip_prefix('/') {
        return rest.to_string();
    }
    let b = n.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        return format!("{}/{}", &n[..1], &n[3..]);
    }
    n
}

impl Layout {
    pub fn new(platform: Platform, root: Option<&Path>, windows_install_path: Option<&str>) -> Layout {
        let (registry_dir, helper, remote_helper, system_gitconfig, polkit): (String, String, String, String, Option<String>) =
            match platform {
                Platform::Macos => (
                    "/etc/gitswitch".into(),
                    "/Library/PrivilegedHelperTools/com.gitswitch.lock-helper".into(),
                    format!("/usr/local/bin/{}", REMOTE_HELPER_NAME),
                    "/etc/gitconfig".into(),
                    None,
                ),
                Platform::Linux => (
                    "/etc/gitswitch".into(),
                    "/usr/local/lib/gitswitch/lock-helper".into(),
                    format!("/usr/local/bin/{}", REMOTE_HELPER_NAME),
                    "/etc/gitconfig".into(),
                    Some("/usr/share/polkit-1/actions/com.gitswitch.lock-helper.policy".into()),
                ),
                Platform::Windows => {
                    let install = norm(windows_install_path.unwrap_or("C:/Program Files/Git"));
                    (
                        "C:/ProgramData/GitSwitch/lock".into(),
                        format!("C:/ProgramData/GitSwitch/lock/bin/{}.exe", HELPER_BIN_NAME),
                        format!("{}/mingw64/libexec/git-core/{}.exe", install, REMOTE_HELPER_NAME),
                        format!("{}/etc/gitconfig", install),
                        None,
                    )
                }
            };
        let at = |s: &str| -> PathBuf {
            match root {
                Some(r) => r.join(relative_form(s)),
                None => PathBuf::from(s),
            }
        };
        let registry = at(&registry_dir);
        Layout {
            platform,
            locks_json: registry.join("locks.json"),
            include_file: registry.join("locks.gitconfig"),
            stanza_dir: registry.join("locks.d"),
            audit_log: registry.join("audit.log"),
            backups_dir: registry.join("backups"),
            registry_dir: registry,
            helper_path: at(&helper),
            remote_helper_path: at(&remote_helper),
            system_gitconfig: at(&system_gitconfig),
            polkit_policy: polkit.map(|p| at(&p)),
        }
    }

    pub fn stanza_path(&self, id: &str) -> PathBuf {
        self.stanza_dir.join(stanza_file_name(id))
    }
}

pub fn stanza_file_name(id: &str) -> String {
    format!("{}.gitconfig", id)
}

/// Forward-slash form of a path for config values (the same rule as the app's
/// `paths::norm`: a backslash inside a git-config value is an escape).
pub fn path_str(p: &Path) -> String {
    norm(&p.to_string_lossy())
}

// ---------------------------------------------------------------------------
// Paths, patterns, ids
// ---------------------------------------------------------------------------

/// Normalise for comparison and for config values: forward slashes, no
/// trailing separator. Identical to the app's `paths::norm`.
pub fn norm(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    let trimmed = slashed.trim_end_matches('/');
    if trimmed.is_empty() {
        slashed
    } else {
        trimmed.to_string()
    }
}

pub fn is_absolute_norm(p: &str) -> bool {
    if p.starts_with('/') {
        return true;
    }
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

/// A `gitdir/i:` pattern for a work tree: trailing slash so git appends `**`
/// and matches the repository, its linked worktrees and anything nested.
pub fn pattern_for(repo_norm: &str) -> String {
    format!("{}/", norm(repo_norm))
}

/// Every pattern a lock needs. A `--separate-git-dir` repository's gitdir is
/// not under its work tree, so it gets the exact-gitdir forms too.
pub fn patterns_for(repo: &str, gitdir: &str) -> Vec<String> {
    let repo = norm(repo);
    let gitdir = norm(gitdir);
    let mut v = vec![pattern_for(&repo)];
    if gitdir != format!("{}/.git", repo) && !gitdir.starts_with(&format!("{}/", repo)) {
        v.push(gitdir.clone());
        v.push(format!("{}/", gitdir));
    }
    v
}

/// Deterministic per repository, so re-locking is idempotent and file names
/// are stable: the first 16 hex digits of SHA-256 over the normalised path.
pub fn lock_id(repo_norm: &str) -> String {
    sha256_hex(norm(repo_norm).as_bytes())[..16].to_string()
}

/// Repositories compare like `gitdir/i:` matches them: case-insensitively.
pub fn same_repo(a: &str, b: &str) -> bool {
    norm(a).eq_ignore_ascii_case(&norm(b))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// RFC 3339 UTC from seconds since the epoch, without a date library (the
/// helper runs as root and links as little as possible).
pub fn rfc3339_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, m, s)
}

/// `a >= b` for dotted numeric versions ("0.2.10" > "0.2.9").
pub fn version_at_least(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (*a.get(i).unwrap_or(&0), *b.get(i).unwrap_or(&0));
        if x != y {
            return x > y;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Validation — the boundary the helper enforces with root privileges
// ---------------------------------------------------------------------------

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// An absolute, already-normalised path that is safe to write into a git
/// config file both as a quoted value and inside `[includeIf "gitdir/i:…"]`.
pub fn validate_path(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("path is empty".into());
    }
    if p.len() > MAX_PATH_BYTES {
        return Err(format!("path longer than {} bytes", MAX_PATH_BYTES));
    }
    if !is_absolute_norm(p) {
        return Err(format!("path is not absolute: {}", p));
    }
    if p != norm(p) {
        return Err(format!("path is not normalised (backslash or trailing slash): {}", p));
    }
    if p.contains("//") || p.contains("/./") || p.contains("/../") || p.ends_with("/.") || p.ends_with("/..") {
        return Err(format!("path contains a redundant or parent component: {}", p));
    }
    if has_control(p) {
        return Err("path contains a control character".into());
    }
    if p.chars().any(|c| matches!(c, '"' | '\\' | ';' | '#' | '[' | ']')) {
        return Err(format!("path contains a character git config cannot carry safely: {}", p));
    }
    if p.trim() != p {
        return Err("path has leading or trailing whitespace".into());
    }
    Ok(())
}

/// A `pushInsteadOf` prefix. Written double-quoted, so only the quote and the
/// escape character are forbidden; whitespace never appears in a URL prefix.
pub fn validate_prefix(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("prefix is empty".into());
    }
    if p.len() > MAX_PREFIX_BYTES {
        return Err(format!("prefix longer than {} bytes", MAX_PREFIX_BYTES));
    }
    if has_control(p) || p.chars().any(char::is_whitespace) {
        return Err("prefix contains whitespace or a control character".into());
    }
    if p.contains('"') || p.contains('\\') {
        return Err("prefix contains a quote or backslash".into());
    }
    Ok(())
}

/// Which system gitconfig paths the helper will ever edit, per platform.
/// Windows is checked against the resolved InstallPath by the helper itself.
pub fn system_gitconfig_allowed(platform: Platform, path: &str) -> bool {
    let path = norm(path);
    match platform {
        Platform::Macos => path == "/etc/gitconfig",
        Platform::Windows => path.ends_with("/etc/gitconfig") && is_absolute_norm(&path),
        Platform::Linux => {
            let Some(dir) = path.strip_suffix("/gitconfig") else { return false };
            if matches!(dir, "/etc" | "/usr/etc" | "/usr/local/etc" | "/home/linuxbrew/.linuxbrew/etc") {
                return true;
            }
            let one_component = |rest: &str| !rest.is_empty() && !rest.contains('/') && rest != "." && rest != "..";
            if let Some(rest) = dir.strip_prefix("/opt/").and_then(|r| r.strip_suffix("/etc")) {
                return one_component(rest);
            }
            if let Some(rest) = dir.strip_prefix("/nix/store/").and_then(|r| r.strip_suffix("/etc")) {
                return one_component(rest);
            }
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Job — what the app asks the helper to do, written to a file the user owns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub schema: u32,
    /// Fresh per job; the result must echo it.
    pub nonce: String,
    pub app_version: String,
    pub platform: Platform,
    /// The system gitconfig the app resolved; the helper accepts it only if
    /// `system_gitconfig_allowed` and, on Windows, only under the InstallPath.
    pub system_gitconfig: String,
    #[serde(default)]
    pub requested_by_uid: Option<u32>,
    #[serde(default)]
    pub requested_by_name: Option<String>,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Op {
    /// Install the helper from the running executable and create the registry.
    Bootstrap,
    Lock {
        repo: String,
        gitdir: String,
        block_prefixes: Vec<String>,
        label: String,
    },
    Unlock {
        repo: String,
    },
    /// Regenerate every managed file from the registry.
    RepairSystem,
    InstallRemoteHelper,
    /// Replace the installed helper with `source` after verifying `sig`
    /// (minisign) against the release key. Without a signature only a
    /// `--bootstrap` run of the source itself may do this.
    UpgradeHelper {
        source: String,
        #[serde(default)]
        sig: Option<String>,
        version: String,
    },
    /// Remove the marker block, the registry, the remote helper and the helper.
    UninstallAll,
}

impl Op {
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Bootstrap => "bootstrap",
            Op::Lock { .. } => "lock",
            Op::Unlock { .. } => "unlock",
            Op::RepairSystem => "repair-system",
            Op::InstallRemoteHelper => "install-remote-helper",
            Op::UpgradeHelper { .. } => "upgrade-helper",
            Op::UninstallAll => "uninstall-all",
        }
    }

    pub fn repo(&self) -> Option<&str> {
        match self {
            Op::Lock { repo, .. } | Op::Unlock { repo } => Some(repo),
            _ => None,
        }
    }
}

fn valid_nonce(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Everything the helper checks before it touches a file. Paths, prefixes and
/// labels are validated character by character; destinations are never taken
/// from the job, so this is about what gets *written into* managed files.
pub fn validate_job(job: &Job, platform: Platform) -> Result<(), String> {
    if job.schema != SCHEMA {
        return Err(format!("job schema {} is not {}", job.schema, SCHEMA));
    }
    if !valid_nonce(&job.nonce) {
        return Err("job nonce must be 1-64 alphanumeric characters or dashes".into());
    }
    if job.platform != platform {
        return Err(format!("job is for {} but this helper runs on {}", job.platform.as_str(), platform.as_str()));
    }
    if job.ops.is_empty() {
        return Err("job has no operations".into());
    }
    if job.ops.len() > MAX_OPS {
        return Err(format!("job has more than {} operations", MAX_OPS));
    }
    validate_path(&job.system_gitconfig).map_err(|e| format!("system_gitconfig: {}", e))?;
    if !system_gitconfig_allowed(platform, &job.system_gitconfig) {
        return Err(format!("{} is not a system gitconfig this helper manages", job.system_gitconfig));
    }
    let (mut locks, mut unlocks, mut uninstall) = (0, 0, 0);
    for op in &job.ops {
        match op {
            Op::Lock { repo, gitdir, block_prefixes, label } => {
                locks += 1;
                validate_path(repo).map_err(|e| format!("lock repo: {}", e))?;
                validate_path(gitdir).map_err(|e| format!("lock gitdir: {}", e))?;
                if same_repo(repo, gitdir) {
                    return Err("gitdir must differ from the repository path".into());
                }
                if block_prefixes.is_empty() {
                    return Err(format!("lock for {} has no prefixes", repo));
                }
                if block_prefixes.len() > MAX_PREFIXES {
                    return Err(format!("lock for {} has more than {} prefixes", repo, MAX_PREFIXES));
                }
                for p in block_prefixes {
                    validate_prefix(p).map_err(|e| format!("prefix {:?}: {}", p, e))?;
                }
                if label.chars().count() > MAX_LABEL_CHARS || has_control(label) {
                    return Err("label is too long or contains a control character".into());
                }
            }
            Op::Unlock { repo } => {
                unlocks += 1;
                validate_path(repo).map_err(|e| format!("unlock repo: {}", e))?;
            }
            Op::UpgradeHelper { source, sig, version } => {
                validate_path(source).map_err(|e| format!("upgrade source: {}", e))?;
                if let Some(s) = sig {
                    validate_path(s).map_err(|e| format!("upgrade signature: {}", e))?;
                }
                if version.is_empty() || version.len() > 32 || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    return Err("upgrade version must be dotted digits".into());
                }
            }
            Op::UninstallAll => uninstall += 1,
            Op::Bootstrap | Op::RepairSystem | Op::InstallRemoteHelper => {}
        }
    }
    if locks > 0 && unlocks > 0 {
        return Err("a job may lock or unlock, never both — the prompt cannot show which".into());
    }
    if uninstall > 0 && job.ops.len() > 1 {
        return Err("uninstall-all must be the only operation in its job".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registry — the admin-owned source of truth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelperInfo {
    pub path: String,
    pub version: String,
    pub sha256: String,
    pub installed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteHelperInfo {
    pub path: String,
    /// `false` when the directory is user-writable (Homebrew's /usr/local/bin
    /// on Intel Macs) — the helper installs anyway and Doctor says so.
    pub dir_admin_owned: bool,
    pub installed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockEntry {
    pub id: String,
    pub repo: String,
    pub gitdir: String,
    pub patterns: Vec<String>,
    pub block_prefixes: Vec<String>,
    pub label: String,
    pub locked_at: String,
    #[serde(default)]
    pub locked_by_uid: Option<u32>,
    #[serde(default)]
    pub locked_by_name: Option<String>,
    pub app_version: String,
}

/// Tolerant of unknown fields on purpose: a registry written by a newer helper
/// is rejected by its `schema`, not by an extra key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Registry {
    pub schema: u32,
    pub platform: Platform,
    pub created_at: String,
    pub updated_at: String,
    pub system_gitconfig: String,
    pub include_file: String,
    pub stanza_dir: String,
    #[serde(default)]
    pub helper: Option<HelperInfo>,
    #[serde(default)]
    pub remote_helper: Option<RemoteHelperInfo>,
    #[serde(default)]
    pub locks: Vec<LockEntry>,
}

impl Registry {
    pub fn new(layout: &Layout, system_gitconfig: &str, now: &str) -> Registry {
        Registry {
            schema: SCHEMA,
            platform: layout.platform,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            system_gitconfig: norm(system_gitconfig),
            include_file: path_str(&layout.include_file),
            stanza_dir: path_str(&layout.stanza_dir),
            helper: None,
            remote_helper: None,
            locks: Vec::new(),
        }
    }

    pub fn find(&self, repo: &str) -> Option<usize> {
        self.locks.iter().position(|l| same_repo(&l.repo, repo))
    }

    pub fn stanza_path(&self, entry: &LockEntry) -> String {
        format!("{}/{}", self.stanza_dir, stanza_file_name(&entry.id))
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        s.push('\n');
        s
    }
}

/// One thing the helper did. Collected into the result and the audit log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub detail: String,
}

/// Apply the registry-level effect of every op. Pure: file-level ops are
/// recorded here and performed by the helper afterwards, so this can be tested
/// without a filesystem. Fails on the lock cap; never panics.
pub fn apply_ops(reg: &mut Registry, job: &Job, now: &str) -> Result<Vec<Change>, String> {
    let mut changes = Vec::new();
    for op in &job.ops {
        match op {
            Op::Lock { repo, gitdir, block_prefixes, label } => {
                let entry = LockEntry {
                    id: lock_id(repo),
                    repo: norm(repo),
                    gitdir: norm(gitdir),
                    patterns: patterns_for(repo, gitdir),
                    block_prefixes: block_prefixes.clone(),
                    label: label.clone(),
                    locked_at: now.to_string(),
                    locked_by_uid: job.requested_by_uid,
                    locked_by_name: job.requested_by_name.clone(),
                    app_version: job.app_version.clone(),
                };
                match reg.find(repo) {
                    Some(i) => {
                        // The existing entry keeps its id, spelling and lock
                        // time: `gitdir/i:` matches case-insensitively, so a
                        // re-lock spelled differently is the same lock and must
                        // not rename its stanza file. Only what enforcement
                        // depends on — the prefixes and the gitdir — is refreshed.
                        let old = &reg.locks[i];
                        let same_gitdir = same_repo(&old.gitdir, &entry.gitdir);
                        let same = same_gitdir && old.block_prefixes == entry.block_prefixes;
                        let (gitdir, patterns) = if same_gitdir {
                            (old.gitdir.clone(), old.patterns.clone())
                        } else {
                            (entry.gitdir.clone(), patterns_for(&old.repo, &entry.gitdir))
                        };
                        let kept = LockEntry {
                            id: old.id.clone(),
                            repo: old.repo.clone(),
                            locked_at: old.locked_at.clone(),
                            gitdir,
                            patterns,
                            ..entry
                        };
                        reg.locks[i] = kept;
                        changes.push(Change {
                            op: "lock".into(),
                            repo: Some(norm(repo)),
                            detail: if same { "already locked; unchanged".into() } else { "lock updated".into() },
                        });
                    }
                    None => {
                        if reg.locks.len() >= MAX_LOCKS {
                            return Err(format!("registry already holds {} locks", MAX_LOCKS));
                        }
                        reg.locks.push(entry);
                        changes.push(Change { op: "lock".into(), repo: Some(norm(repo)), detail: "locked".into() });
                    }
                }
            }
            Op::Unlock { repo } => match reg.find(repo) {
                Some(i) => {
                    reg.locks.remove(i);
                    changes.push(Change { op: "unlock".into(), repo: Some(norm(repo)), detail: "unlocked".into() });
                }
                None => changes.push(Change {
                    op: "unlock".into(),
                    repo: Some(norm(repo)),
                    detail: "was not locked; unchanged".into(),
                }),
            },
            Op::UninstallAll => {
                let n = reg.locks.len();
                reg.locks.clear();
                changes.push(Change { op: "uninstall-all".into(), repo: None, detail: format!("{} lock(s) removed", n) });
            }
            Op::Bootstrap | Op::RepairSystem | Op::InstallRemoteHelper | Op::UpgradeHelper { .. } => {}
        }
    }
    reg.updated_at = now.to_string();
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Result — what the helper hands back
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobResult {
    pub schema: u32,
    pub nonce: String,
    pub ok: bool,
    pub helper_version: String,
    pub changed: Vec<Change>,
    pub errors: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub registry_sha256: Option<String>,
}

impl JobResult {
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        s.push('\n');
        s
    }
}

// ---------------------------------------------------------------------------
// Rendering — the exact bytes of every managed file
// ---------------------------------------------------------------------------

fn quoted(v: &str) -> String {
    // Validation forbids `"` and `\`, so quoting needs no escaping; quoting
    // keeps `;` and `#` literal and preserves the value byte for byte.
    format!("\"{}\"", v)
}

/// One lock's config, included only for its own gitdir. `gitswitch.locked`
/// comes from this admin-owned file, so nothing local can unset it.
pub fn render_stanza(entry: &LockEntry) -> String {
    let mut s = String::new();
    s.push_str(&format!("# GitSwitch push lock for {}\n", quoted(&entry.repo)));
    s.push_str("# Managed by the GitSwitch lock helper. Do not edit: changes need an administrator\n");
    s.push_str("# (GitSwitch -> Changes -> Push access -> Unlock).\n");
    s.push_str("[gitswitch]\n");
    s.push_str("\tlocked = true\n");
    s.push_str(&format!("\tlockId = {}\n", entry.id));
    s.push_str(&format!("\tlockedRepo = {}\n", quoted(&entry.repo)));
    s.push_str(&format!("\tlockedAt = {}\n", entry.locked_at));
    s.push_str(&format!("[url {}]\n", quoted(BLOCK_SCHEME)));
    for p in &entry.block_prefixes {
        s.push_str(&format!("\tpushInsteadOf = {}\n", quoted(p)));
    }
    s
}

/// The include file: one `includeIf` per pattern, ordered so output is stable.
pub fn render_include_file(reg: &Registry) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# GitSwitch push locks - generated from {}/locks.json. Do not edit: changes need an administrator.\n",
        reg.stanza_dir.rsplit_once('/').map(|(d, _)| d).unwrap_or(&reg.stanza_dir)
    ));
    let mut lines: Vec<(String, String)> = Vec::new();
    for l in &reg.locks {
        for p in &l.patterns {
            lines.push((p.clone(), reg.stanza_path(l)));
        }
    }
    lines.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
    for (pattern, path) in lines {
        s.push_str(&format!("[includeIf \"gitdir/i:{}\"]\n\tpath = {}\n", pattern, path));
    }
    s
}

/// The marker block the helper puts in the system gitconfig.
pub fn marker_block(include_file: &str) -> String {
    format!("{}\n[include]\n\tpath = {}\n{}\n", MARKER_BEGIN, norm(include_file), MARKER_END)
}

/// Drop our block, keeping every other byte. A begin marker with no end marker
/// (a damaged file) drops to EOF rather than leaving half a block behind.
pub fn remove_marker_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut inside = false;
    for line in content.split_inclusive('\n') {
        if !inside && line.contains(MARKER_BEGIN) {
            inside = true;
            continue;
        }
        if inside {
            if line.contains(MARKER_END) {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Replace or append our block. Foreign content is preserved; the only byte
/// ever added outside the block is a newline when the file lacked a final one.
pub fn upsert_marker_block(content: &str, include_file: &str) -> String {
    let mut base = remove_marker_block(content);
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }
    base.push_str(&marker_block(include_file));
    base
}

/// The `path =` inside our block, if the block is present.
pub fn marker_block_include_path(content: &str) -> Option<String> {
    let mut inside = false;
    for line in content.lines() {
        if line.contains(MARKER_BEGIN) {
            inside = true;
            continue;
        }
        if line.contains(MARKER_END) {
            return None;
        }
        if inside {
            if let Some(v) = line.trim().strip_prefix("path") {
                if let Some(v) = v.trim_start().strip_prefix('=') {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// The pattern → stanza path pairs an include file carries (for measurement).
pub fn parse_include_file(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pattern: Option<String> = None;
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("[includeIf \"gitdir/i:") {
            pattern = rest.strip_suffix("\"]").map(|s| s.to_string());
        } else if let Some(p) = pattern.take() {
            if let Some(v) = t.strip_prefix("path").and_then(|v| v.trim_start().strip_prefix('=')) {
                out.push((p, v.trim().to_string()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The words a refused push prints — for people and for agents
// ---------------------------------------------------------------------------

pub mod text {
    pub const LOCKED_HEAD: &str = "GitSwitch: push refused.";
    pub const LOCKED_WHO: &str = "This repository's owner locked pushes from this machine in GitSwitch.";
    pub const LOCKED_ASK: &str = "Do not work around this; ask the owner to unlock it, which requires their administrator password. Fetch and pull still work.";
    pub const LOCKED_UNLOCK: &str = "Unlock: GitSwitch -> Changes -> Push access -> Unlock (administrator prompt).";
    /// A machine-readable last line for agents that parse stderr.
    pub const AGENT_TRAILER: &str = "gitswitch: lock=on policy=ask-owner";
    pub const GUARDRAIL_HEAD: &str = "GitSwitch: push blocked for this repository.";
    pub const GUARDRAIL_WHERE: &str = "Push access is turned off in GitSwitch (guard-rail). Turn it back on there: Changes -> Push access.";
}

/// What the remote helper (and the hook) print when a repository is locked.
/// No git command appears anywhere in it: the old hook printed the very
/// command that disabled it.
pub fn locked_refusal(repo: Option<&str>, remote: Option<&str>, url: Option<&str>) -> String {
    let mut s = format!("{}\n{}\n", text::LOCKED_HEAD, text::LOCKED_WHO);
    if let Some(r) = repo {
        s.push_str(&format!("  repository: {}\n", r));
    }
    match (remote, url) {
        (Some(n), Some(u)) => s.push_str(&format!("  remote:     {}  ({})\n", n, u)),
        (Some(n), None) => s.push_str(&format!("  remote:     {}\n", n)),
        (None, Some(u)) => s.push_str(&format!("  remote:     {}\n", u)),
        (None, None) => {}
    }
    s.push_str(&format!("{}\n{}\n{}\n", text::LOCKED_ASK, text::LOCKED_UNLOCK, text::AGENT_TRAILER));
    s
}

/// A profile-level block: the rewrite came from a GitSwitch profile include.
pub fn profile_refusal(remote: Option<&str>) -> String {
    let mut s = String::from("GitSwitch: push blocked for this folder.");
    if let Some(n) = remote {
        s.push_str(&format!("\n  remote: {}", n));
    }
    s.push_str("\nA GitSwitch profile covering this folder has push access turned off. Change it in GitSwitch -> Profiles.\n");
    s
}

/// The polkit action that lets `pkexec` run the installed helper on Linux.
/// `auth_admin`, not `auth_admin_keep`: every job prompts.
pub fn render_polkit_policy(helper_path: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE policyconfig PUBLIC \"-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN\" \"http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd\">\n\
<policyconfig>\n\
  <vendor>GitSwitch</vendor>\n\
  <action id=\"com.gitswitch.lock-helper\">\n\
    <description>Change a GitSwitch push lock</description>\n\
    <message>GitSwitch needs an administrator to change a push lock.</message>\n\
    <defaults>\n\
      <allow_any>auth_admin</allow_any>\n\
      <allow_inactive>auth_admin</allow_inactive>\n\
      <allow_active>auth_admin</allow_active>\n\
    </defaults>\n\
    <annotate key=\"org.freedesktop.policykit.exec.path\">{}</annotate>\n\
  </action>\n\
</policyconfig>\n",
        helper_path
    )
}

/// The guard-rail variant: the block is the user's own setting.
pub fn guardrail_refusal(remote: Option<&str>) -> String {
    let mut s = String::from(text::GUARDRAIL_HEAD);
    if let Some(n) = remote {
        s.push_str(&format!("\n  remote: {}", n));
    }
    s.push('\n');
    s.push_str(text::GUARDRAIL_WHERE);
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// Signatures — helper upgrades are anchored to the release key
// ---------------------------------------------------------------------------

/// Verify a minisign signature (`.sig` file text) over `data` with a public
/// key given as base64 of the whole key file, the way tauri.conf.json stores it.
pub fn verify_minisign(pubkey_file_b64: &str, data: &[u8], signature_text: &str) -> Result<(), String> {
    use base64::Engine;
    let key_file = base64::engine::general_purpose::STANDARD
        .decode(pubkey_file_b64.trim())
        .map_err(|e| format!("public key is not base64: {}", e))?;
    let key_text = String::from_utf8(key_file).map_err(|_| "public key file is not UTF-8".to_string())?;
    let pk = minisign_verify::PublicKey::decode(&key_text).map_err(|e| format!("public key: {}", e))?;
    let sig = minisign_verify::Signature::decode(signature_text).map_err(|e| format!("signature: {}", e))?;
    pk.verify(data, &sig, false).map_err(|e| format!("signature does not verify: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_op(repo: &str) -> Op {
        Op::Lock {
            repo: repo.into(),
            gitdir: format!("{}/.git", repo),
            block_prefixes: vec!["git@github.com:".into(), "https://github.com/".into(), "git@github.com:o/r.git".into()],
            label: "r".into(),
        }
    }

    fn job(ops: Vec<Op>) -> Job {
        Job {
            schema: SCHEMA,
            nonce: "abc-123".into(),
            app_version: "0.1.0".into(),
            platform: Platform::Macos,
            system_gitconfig: "/etc/gitconfig".into(),
            requested_by_uid: Some(501),
            requested_by_name: Some("me".into()),
            ops,
        }
    }

    fn layout() -> Layout {
        Layout::new(Platform::Macos, None, None)
    }

    #[test]
    fn layout_paths_are_admin_owned_locations_and_root_is_applied_everywhere() {
        let l = layout();
        assert_eq!(l.registry_dir, PathBuf::from("/etc/gitswitch"));
        assert_eq!(l.locks_json, PathBuf::from("/etc/gitswitch/locks.json"));
        assert_eq!(l.helper_path, PathBuf::from("/Library/PrivilegedHelperTools/com.gitswitch.lock-helper"));
        assert_eq!(l.remote_helper_path, PathBuf::from("/usr/local/bin/git-remote-gitswitch-push-blocked"));
        assert_eq!(l.system_gitconfig, PathBuf::from("/etc/gitconfig"));
        let r = Layout::new(Platform::Macos, Some(Path::new("/tmp/root")), None);
        assert_eq!(r.locks_json, PathBuf::from("/tmp/root/etc/gitswitch/locks.json"));
        assert_eq!(r.system_gitconfig, PathBuf::from("/tmp/root/etc/gitconfig"));
        assert_eq!(r.helper_path, PathBuf::from("/tmp/root/Library/PrivilegedHelperTools/com.gitswitch.lock-helper"));
        let w = Layout::new(Platform::Windows, Some(Path::new("/tmp/root")), Some(r"D:\Tools\Git"));
        assert_eq!(w.system_gitconfig, PathBuf::from("/tmp/root/D/Tools/Git/etc/gitconfig"));
        assert_eq!(w.registry_dir, PathBuf::from("/tmp/root/C/ProgramData/GitSwitch/lock"));
        let w2 = Layout::new(Platform::Windows, None, Some(r"C:\Program Files\Git"));
        assert_eq!(w2.remote_helper_path, PathBuf::from("C:/Program Files/Git/mingw64/libexec/git-core/git-remote-gitswitch-push-blocked.exe"));
        assert!(Layout::new(Platform::Linux, None, None).polkit_policy.is_some());
    }

    #[test]
    fn patterns_cover_the_work_tree_and_a_separate_gitdir() {
        assert_eq!(patterns_for("/Users/me/work/argos", "/Users/me/work/argos/.git"), vec!["/Users/me/work/argos/"]);
        assert_eq!(
            patterns_for("/Users/me/work/argos", "/Users/me/gitdirs/argos"),
            vec!["/Users/me/work/argos/", "/Users/me/gitdirs/argos", "/Users/me/gitdirs/argos/"]
        );
        // A nested gitdir (a submodule-style layout) is already covered by the tree pattern.
        assert_eq!(patterns_for("/r", "/r/sub/.git"), vec!["/r/"]);
        assert_eq!(pattern_for(r"C:\Users\me\repo"), "C:/Users/me/repo/");
    }

    #[test]
    fn ids_are_stable_and_repos_compare_case_insensitively() {
        assert_eq!(lock_id("/a/b"), lock_id("/a/b/"));
        assert_eq!(lock_id("/a/b").len(), 16);
        assert_ne!(lock_id("/a/b"), lock_id("/a/c"));
        assert!(same_repo("/Users/Me/Repo", "/users/me/repo/"));
        assert!(!same_repo("/a/b", "/a/bc"));
    }

    #[test]
    fn time_and_version_helpers() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert!(version_at_least("0.2.10", "0.2.9"));
        assert!(version_at_least("0.2.0", "0.2.0"));
        assert!(!version_at_least("0.1.9", "0.2.0"));
        assert!(version_at_least("1.0", "0.9.9"));
    }

    #[test]
    fn path_validation_rejects_everything_that_could_escape_a_config_value() {
        assert!(validate_path("/Users/me/work/argos").is_ok());
        assert!(validate_path("/Users/me/with space/repo").is_ok());
        assert!(validate_path("C:/Users/me/repo").is_ok());
        assert!(validate_path("/Users/me/ünïcode").is_ok());
        for bad in [
            "", "relative/path", "./x", "/a/b/", "/a//b", "/a/./b", "/a/../b", "/a/..", r"C:\Users\me",
            "/a\"b", "/a\\b", "/a;b", "/a#b", "/a[b]", "/a\nb", "/a\tb", " /a", "/a ", "C:relative",
        ] {
            assert!(validate_path(bad).is_err(), "{:?} should be rejected", bad);
        }
        let long = format!("/{}", "x".repeat(MAX_PATH_BYTES));
        assert!(validate_path(&long).is_err());
    }

    #[test]
    fn prefix_validation() {
        for good in ["git@github.com:", "https://github.com/", "gh-work:o/r.git", "org-1@github.com:E/a.git", "/srv/git/repo.git"] {
            assert!(validate_prefix(good).is_ok(), "{}", good);
        }
        for bad in ["", "a b", "a\"b", "a\\b", "a\nb"] {
            assert!(validate_prefix(bad).is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn system_gitconfig_allowlist_per_platform() {
        assert!(system_gitconfig_allowed(Platform::Macos, "/etc/gitconfig"));
        assert!(!system_gitconfig_allowed(Platform::Macos, "/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig"));
        assert!(!system_gitconfig_allowed(Platform::Macos, "/Users/me/.gitconfig"));
        for ok in ["/etc/gitconfig", "/usr/etc/gitconfig", "/usr/local/etc/gitconfig", "/opt/homebrew/etc/gitconfig", "/nix/store/abc-git/etc/gitconfig", "/home/linuxbrew/.linuxbrew/etc/gitconfig"] {
            assert!(system_gitconfig_allowed(Platform::Linux, ok), "{}", ok);
        }
        for bad in ["/home/me/.gitconfig", "/opt/etc/gitconfig", "/opt/a/b/etc/gitconfig", "/etc/gitconfig.d/x", "/tmp/etc/gitconfig", "/nix/store/../etc/gitconfig"] {
            assert!(!system_gitconfig_allowed(Platform::Linux, bad), "{}", bad);
        }
        assert!(system_gitconfig_allowed(Platform::Windows, "C:/Program Files/Git/etc/gitconfig"));
        assert!(!system_gitconfig_allowed(Platform::Windows, "C:/Users/me/.gitconfig"));
    }

    #[test]
    fn job_validation_enforces_shape_caps_and_the_lock_unlock_split() {
        assert!(validate_job(&job(vec![lock_op("/Users/me/r")]), Platform::Macos).is_ok());
        assert!(validate_job(&job(vec![Op::Unlock { repo: "/Users/me/r".into() }]), Platform::Macos).is_ok());
        let both = job(vec![lock_op("/Users/me/r"), Op::Unlock { repo: "/Users/me/q".into() }]);
        assert!(validate_job(&both, Platform::Macos).unwrap_err().contains("never both"));
        let mut wrong_platform = job(vec![lock_op("/Users/me/r")]);
        wrong_platform.platform = Platform::Linux;
        assert!(validate_job(&wrong_platform, Platform::Macos).is_err());
        let mut bad_schema = job(vec![lock_op("/Users/me/r")]);
        bad_schema.schema = 99;
        assert!(validate_job(&bad_schema, Platform::Macos).is_err());
        let mut bad_sys = job(vec![lock_op("/Users/me/r")]);
        bad_sys.system_gitconfig = "/Users/me/.gitconfig".into();
        assert!(validate_job(&bad_sys, Platform::Macos).is_err());
        let mut bad_nonce = job(vec![lock_op("/Users/me/r")]);
        bad_nonce.nonce = "../x".into();
        assert!(validate_job(&bad_nonce, Platform::Macos).is_err());
        assert!(validate_job(&job(vec![]), Platform::Macos).is_err());
        let many = job((0..=MAX_OPS).map(|i| lock_op(&format!("/r{}", i))).collect());
        assert!(validate_job(&many, Platform::Macos).is_err());
        let mixed_uninstall = job(vec![Op::UninstallAll, Op::RepairSystem]);
        assert!(validate_job(&mixed_uninstall, Platform::Macos).is_err());
        assert!(validate_job(&job(vec![Op::UninstallAll]), Platform::Macos).is_ok());
        let no_prefix = job(vec![Op::Lock { repo: "/r".into(), gitdir: "/r/.git".into(), block_prefixes: vec![], label: "x".into() }]);
        assert!(validate_job(&no_prefix, Platform::Macos).is_err());
        let bad_prefix = job(vec![Op::Lock { repo: "/r".into(), gitdir: "/r/.git".into(), block_prefixes: vec!["a\"b".into()], label: "x".into() }]);
        assert!(validate_job(&bad_prefix, Platform::Macos).is_err());
        let bad_repo = job(vec![Op::Lock { repo: "relative".into(), gitdir: "/r/.git".into(), block_prefixes: vec!["a:".into()], label: "x".into() }]);
        assert!(validate_job(&bad_repo, Platform::Macos).is_err());
        let up = job(vec![Op::UpgradeHelper { source: "/Applications/GitSwitch.app/Contents/MacOS/gitswitch-lock-helper".into(), sig: None, version: "0.2.0".into() }]);
        assert!(validate_job(&up, Platform::Macos).is_ok());
        let bad_up = job(vec![Op::UpgradeHelper { source: "/x".into(), sig: None, version: "0.2.0; rm".into() }]);
        assert!(validate_job(&bad_up, Platform::Macos).is_err());
    }

    #[test]
    fn unknown_job_fields_are_rejected_and_ops_are_tagged() {
        let raw = r#"{"schema":1,"nonce":"n","app_version":"0","platform":"macos","system_gitconfig":"/etc/gitconfig","ops":[{"op":"lock","repo":"/r","gitdir":"/r/.git","block_prefixes":["a:"],"label":"l"}],"extra":1}"#;
        assert!(serde_json::from_str::<Job>(raw).is_err());
        let raw = r#"{"schema":1,"nonce":"n","app_version":"0","platform":"macos","system_gitconfig":"/etc/gitconfig","ops":[{"op":"unlock","repo":"/r"},{"op":"repair-system"}]}"#;
        let j: Job = serde_json::from_str(raw).unwrap();
        assert_eq!(j.ops[0].kind(), "unlock");
        assert_eq!(j.ops[1], Op::RepairSystem);
        let round: Job = serde_json::from_str(&serde_json::to_string(&job(vec![lock_op("/r")])).unwrap()).unwrap();
        assert_eq!(round.ops[0].repo(), Some("/r"));
    }

    #[test]
    fn apply_ops_is_idempotent_and_reports_what_changed() {
        let l = layout();
        let mut reg = Registry::new(&l, "/etc/gitconfig", "T0");
        let ch = apply_ops(&mut reg, &job(vec![lock_op("/Users/me/r")]), "T1").unwrap();
        assert_eq!(ch[0].detail, "locked");
        assert_eq!(reg.locks.len(), 1);
        assert_eq!(reg.locks[0].locked_at, "T1");
        assert_eq!(reg.locks[0].locked_by_uid, Some(501));
        // Re-locking the same repo, even spelled differently, changes nothing.
        let ch = apply_ops(&mut reg, &job(vec![lock_op("/Users/ME/r")]), "T2").unwrap();
        assert!(ch[0].detail.contains("unchanged"));
        assert_eq!(reg.locks.len(), 1);
        assert_eq!(reg.locks[0].locked_at, "T1", "the original lock time survives a re-lock");
        assert_eq!(reg.updated_at, "T2");
        // New prefixes are an update.
        let mut op = lock_op("/Users/me/r");
        if let Op::Lock { block_prefixes, .. } = &mut op {
            block_prefixes.push("gh-work:".into());
        }
        let ch = apply_ops(&mut reg, &job(vec![op]), "T3").unwrap();
        assert_eq!(ch[0].detail, "lock updated");
        assert!(reg.locks[0].block_prefixes.contains(&"gh-work:".to_string()));
        assert_eq!(reg.locks[0].repo, "/Users/me/r", "the first spelling is kept");
        assert_eq!(reg.locks[0].id, lock_id("/Users/me/r"), "so the stanza file name is stable");
        let ch = apply_ops(&mut reg, &job(vec![Op::Unlock { repo: "/Users/me/r".into() }]), "T4").unwrap();
        assert_eq!(ch[0].detail, "unlocked");
        assert!(reg.locks.is_empty());
        let ch = apply_ops(&mut reg, &job(vec![Op::Unlock { repo: "/Users/me/r".into() }]), "T5").unwrap();
        assert!(ch[0].detail.contains("not locked"));
        assert_eq!(reg.locks.len(), 0);
        let ch = apply_ops(&mut reg, &job(vec![Op::RepairSystem]), "T6").unwrap();
        assert!(ch.is_empty(), "file-level ops are the helper's to record");
    }

    #[test]
    fn stanza_is_exactly_what_git_needs_and_never_prints_a_command() {
        let entry = LockEntry {
            id: "abc".into(),
            repo: "/Users/me/with space/argos".into(),
            gitdir: "/Users/me/with space/argos/.git".into(),
            patterns: vec!["/Users/me/with space/argos/".into()],
            block_prefixes: vec!["git@github.com:".into(), "org-1@github.com:E/argos.git".into()],
            label: "argos".into(),
            locked_at: "2026-09-23T09:15:00Z".into(),
            locked_by_uid: Some(501),
            locked_by_name: None,
            app_version: "0.1.0".into(),
        };
        let s = render_stanza(&entry);
        assert!(s.contains("[gitswitch]\n\tlocked = true\n\tlockId = abc\n\tlockedRepo = \"/Users/me/with space/argos\"\n"));
        assert!(s.contains("[url \"gitswitch-push-blocked://\"]\n\tpushInsteadOf = \"git@github.com:\"\n\tpushInsteadOf = \"org-1@github.com:E/argos.git\"\n"));
        assert!(!s.contains("\tinsteadOf"), "a plain insteadOf would break fetch");
        assert!(!s.contains("git config"));
        assert!(!s.contains('\\'));
    }

    #[test]
    fn include_file_lists_patterns_shortest_first_and_round_trips() {
        let l = layout();
        let mut reg = Registry::new(&l, "/etc/gitconfig", "T");
        apply_ops(&mut reg, &job(vec![lock_op("/Users/me/work/argos"), lock_op("/Users/me/x")]), "T").unwrap();
        let inc = render_include_file(&reg);
        assert!(inc.starts_with("# GitSwitch push locks - generated from /etc/gitswitch/locks.json."));
        let parsed = parse_include_file(&inc);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "/Users/me/x/");
        assert_eq!(parsed[1].0, "/Users/me/work/argos/");
        assert_eq!(parsed[1].1, format!("/etc/gitswitch/locks.d/{}.gitconfig", lock_id("/Users/me/work/argos")));
        assert!(inc.contains("[includeIf \"gitdir/i:/Users/me/x/\"]\n\tpath = /etc/gitswitch/locks.d/"));
        // Rendering twice gives identical bytes.
        assert_eq!(inc, render_include_file(&reg));
    }

    #[test]
    fn marker_block_is_upserted_and_removed_without_touching_foreign_bytes() {
        let foreign = "[core]\n\tautocrlf = input\n# a comment\n[user]\n\tname = x\n";
        let once = upsert_marker_block(foreign, "/etc/gitswitch/locks.gitconfig");
        assert!(once.starts_with(foreign));
        assert!(once.ends_with(&marker_block("/etc/gitswitch/locks.gitconfig")));
        assert_eq!(marker_block_include_path(&once).as_deref(), Some("/etc/gitswitch/locks.gitconfig"));
        // Upserting again replaces rather than duplicates.
        let twice = upsert_marker_block(&once, "/etc/gitswitch/other.gitconfig");
        assert_eq!(twice.matches(MARKER_BEGIN).count(), 1);
        assert_eq!(marker_block_include_path(&twice).as_deref(), Some("/etc/gitswitch/other.gitconfig"));
        // Removal gives the foreign content back byte for byte.
        assert_eq!(remove_marker_block(&twice), foreign);
        assert_eq!(remove_marker_block(foreign), foreign);
        // A file without a trailing newline gains exactly one.
        let no_nl = "[core]\n\tx = 1";
        assert_eq!(remove_marker_block(&upsert_marker_block(no_nl, "/i")), format!("{}\n", no_nl));
        // An empty file becomes just the block, and removal empties it again.
        assert_eq!(remove_marker_block(&upsert_marker_block("", "/i")), "");
        // Content after our block survives.
        let after = format!("{}[alias]\n\tco = checkout\n", marker_block("/i"));
        assert_eq!(remove_marker_block(&after), "[alias]\n\tco = checkout\n");
        // A damaged block (no end marker) is dropped to EOF, not left half-open.
        let damaged = format!("[a]\n\tb = 1\n{}\n[include]\n\tpath = /i\n", MARKER_BEGIN);
        assert_eq!(remove_marker_block(&damaged), "[a]\n\tb = 1\n");
        assert!(marker_block_include_path(foreign).is_none());
    }

    #[test]
    fn refusal_text_addresses_agents_and_never_contains_a_git_command() {
        let s = locked_refusal(Some("/Users/me/argos"), Some("origin"), Some("gitswitch-push-blocked://x"));
        assert!(s.starts_with("GitSwitch: push refused.\n"));
        assert!(s.contains("  repository: /Users/me/argos\n"));
        assert!(s.contains("  remote:     origin  (gitswitch-push-blocked://x)\n"));
        assert!(s.contains("Do not work around this"));
        assert!(s.contains("administrator password"));
        assert!(s.ends_with("gitswitch: lock=on policy=ask-owner\n"));
        assert!(!s.contains("git config"));
        let g = guardrail_refusal(Some("origin"));
        assert!(g.contains("guard-rail"));
        assert!(!g.contains("git config"));
        assert!(!locked_refusal(None, None, None).contains("remote:"));
        assert!(profile_refusal(Some("origin")).contains("Profiles"));
        let pol = render_polkit_policy("/usr/local/lib/gitswitch/lock-helper");
        assert!(pol.contains("<allow_active>auth_admin</allow_active>"));
        assert!(!pol.contains("auth_admin_keep"));
        assert!(pol.contains("exec.path\">/usr/local/lib/gitswitch/lock-helper<"));
    }

    #[test]
    fn registry_json_round_trips_and_tolerates_unknown_fields() {
        let l = layout();
        let mut reg = Registry::new(&l, "/etc/gitconfig", "T");
        apply_ops(&mut reg, &job(vec![lock_op("/r")]), "T").unwrap();
        let json = reg.to_json();
        let back: Registry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reg);
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["future_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Registry>(v).is_ok());
    }

    #[test]
    fn sha256_and_signature_plumbing() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // The embedded key decodes as a minisign public key file.
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD.decode(RELEASE_PUBKEY_B64).unwrap();
        let text = String::from_utf8(key).unwrap();
        assert!(text.starts_with("untrusted comment:"));
        assert!(minisign_verify::PublicKey::decode(&text).is_ok());
        // A garbage signature is refused, not panicked on.
        assert!(verify_minisign(RELEASE_PUBKEY_B64, b"data", "not a signature").is_err());
        assert!(verify_minisign("!!!", b"data", "x").is_err());
    }
}
