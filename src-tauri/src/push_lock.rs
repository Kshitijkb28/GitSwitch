//! The push lock as the app sees it. Nothing here runs elevated: this module
//! measures the admin-owned layers the helper wrote, keeps the user-level
//! mirrors in step, and writes the jobs the privileged helper
//! (`lock-helper/`) applies after the operating system's administrator prompt.
//!
//! Measured, never assumed: every "ok" below comes from reading the file git
//! reads or from asking git what it sees at system scope.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use crate::push_guard;
use gitswitch_lock_core as lc;
use lc::{Layout, Op, Platform, Registry};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Self-heal of the mirrors is rate-limited so a fight with something that
/// keeps removing them cannot become a busy loop.
const HEAL_INTERVAL: Duration = Duration::from_secs(10);
const RECENT_EVENTS: usize = 10;
const EVENT_LOG_KEEP: usize = 2000;

// ---------------------------------------------------------------------------
// State types (mirrored in src/lib/api.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct LockEvent {
    pub ts: String,
    pub repo: String,
    pub code: String,
    pub detail: String,
    pub healed: bool,
}

/// Nested inside `PushState`, because the Changes page replaces `status.push`
/// wholesale with what the backend returns.
#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct LockState {
    /// The platform is handled and the system gitconfig is known.
    pub supported: bool,
    pub platform: Option<String>,
    /// The registry has this repository.
    pub locked: bool,
    /// ok | missing | untrusted | unreadable
    pub registry: String,
    /// ok | missing | wrong-target  (only meaningful when locked)
    pub system_include: String,
    /// ok | missing | stale
    pub stanza: String,
    /// git itself reports the rewrite at system scope, from the stanza file.
    pub system_rewrite_measured: bool,
    /// ok | missing | outdated | untrusted
    pub helper: String,
    pub helper_installed: bool,
    /// ok | missing | foreign | unprotected-dir
    pub remote_helper: String,
    /// ok | healed | drifted | unfixable
    pub mirrors: String,
    /// Stable codes, for Doctor and the tests.
    pub drift: Vec<String>,
    pub needs_elevation: bool,
    /// What the lock does not stop, for this platform, in plain words.
    pub caveats: Vec<String>,
    pub locked_at: Option<String>,
    pub recent_events: Vec<LockEvent>,
    pub system_gitconfig: Option<String>,
    pub audit_log: Option<String>,
    pub bundled_helper_sha256: Option<String>,
}

/// What `push_guard` measured about the user-level mirrors.
#[derive(Debug, Clone, Default)]
pub struct MirrorFacts {
    pub flag: bool,
    pub rewrite: bool,
    pub hook_ok: bool,
    /// `core.hooksPath` points elsewhere, so the app will not write a hook.
    pub hooks_redirected: bool,
    pub explicit_pushurl: Vec<String>,
    /// Remotes neither rewritten nor carrying an explicit pushurl.
    pub uncovered_remotes: Vec<String>,
    /// Raw remote URLs, for the expected prefix list.
    pub remote_urls: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct JobOutcome {
    /// applied | cancelled | denied | no-agent | manual-required | busy | failed | refused | unsupported
    pub outcome: String,
    pub message: String,
    /// For `manual-required`: the exact command to run as an administrator.
    pub command: Option<String>,
    /// For `manual-required`: pass back to `finish_manual`.
    pub job_nonce: Option<String>,
    pub changed: Vec<lc::Change>,
    pub errors: Vec<String>,
    pub bootstrapped: bool,
}

impl JobOutcome {
    fn simple(outcome: &str, message: impl Into<String>) -> JobOutcome {
        JobOutcome { outcome: outcome.into(), message: message.into(), ..Default::default() }
    }
    pub fn applied(&self) -> bool {
        self.outcome == "applied"
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct PushModeResult {
    pub outcome: String,
    pub message: String,
    pub command: Option<String>,
    pub job_nonce: Option<String>,
    pub state: Option<push_guard::PushState>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct LockSummary {
    pub repo: String,
    pub label: String,
    pub locked_at: String,
    pub exists: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct HelperStatus {
    pub supported: bool,
    pub platform: Option<String>,
    pub helper_path: Option<String>,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub installed_sha256: Option<String>,
    /// ok | missing | outdated | untrusted
    pub helper: String,
    pub bundled_path: Option<String>,
    pub bundled_version: Option<String>,
    pub bundled_sha256: Option<String>,
    pub bundled_signed: bool,
    pub registry: String,
    pub locks: Vec<LockSummary>,
    pub system_gitconfig: Option<String>,
    pub remote_helper: String,
    pub remote_helper_path: Option<String>,
    pub registry_dir: Option<String>,
    pub audit_log: Option<String>,
    pub events_log: Option<String>,
    pub uac: Option<UacState>,
}

// ---------------------------------------------------------------------------
// Layout, platform, files
// ---------------------------------------------------------------------------

/// Debug builds only: the verify suites re-root every admin-owned path.
fn test_root() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("GITSWITCH_LOCK_ROOT").map(PathBuf::from)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Git for Windows' InstallPath from the registry (64-bit view first); the
/// helper reads the same key itself and cross-checks the job against it.
#[cfg(windows)]
fn windows_install_path() -> Option<String> {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY};
    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };
    let key = wide("SOFTWARE\\GitForWindows");
    let val = wide("InstallPath");
    for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
        let mut size = 0u32;
        // SAFETY: documented two-call pattern; buffer sized from the first call.
        unsafe {
            if RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ | view, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) != ERROR_SUCCESS || size == 0 {
                continue;
            }
            let mut buf = vec![0u16; size as usize / 2 + 1];
            if RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ | view, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) != ERROR_SUCCESS {
                continue;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..len]);
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn windows_install_path() -> Option<String> {
    None
}

pub fn layout() -> Option<Layout> {
    let p = Platform::current()?;
    Some(Layout::new(p, test_root().as_deref(), windows_install_path().as_deref()))
}

/// The git whose system scope the lock lives in — Apple's on macOS, whatever
/// Homebrew put first on PATH is deliberately not it.
pub fn system_git() -> PathBuf {
    if cfg!(target_os = "macos") {
        let p = PathBuf::from("/usr/bin/git");
        if p.exists() {
            return p;
        }
    }
    if cfg!(windows) {
        if let Some(install) = windows_install_path() {
            let p = PathBuf::from(install).join("cmd").join("git.exe");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("git")
}

fn sys_git(repo: &Path) -> GitCmd {
    GitCmd::at(repo).with_program(system_git())
}

/// Forward-slash form of a real path: git matches `gitdir:` patterns against
/// the realpath of the .git directory, so the lock must use it too.
pub fn canonical(path: &Path) -> String {
    let c = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim(&lc::norm(&c.to_string_lossy()))
}

/// Windows' canonicalize returns the verbatim form (`\\?\C:\…`, UNC as
/// `\\?\UNC\server\share`); git, the registry and the helper's validator
/// all want the plain form.
pub fn strip_verbatim(n: &str) -> String {
    if let Some(rest) = n.strip_prefix("//?/UNC/") {
        return format!("//{}", rest);
    }
    if let Some(rest) = n.strip_prefix("//?/") {
        return rest.to_string();
    }
    n.to_string()
}

/// Windows User Account Control as the registry reports it; `None` elsewhere
/// or when unreadable.
#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct UacState {
    /// `EnableLUA`: 0 means no prompt at all.
    pub enabled: bool,
    /// `ConsentPromptBehaviorAdmin`: 0 = no prompt, 1/3 = password, 2/5 = consent click.
    pub admin_behavior: u32,
}

#[cfg(windows)]
fn windows_uac_state() -> Option<UacState> {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };
    let key = wide("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System");
    let read = |name: &str| -> Option<u32> {
        let val = wide(name);
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: documented DWORD read into a correctly sized buffer.
        let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), val.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut c_void, &mut size) };
        if rc == ERROR_SUCCESS { Some(data) } else { None }
    };
    Some(UacState { enabled: read("EnableLUA").unwrap_or(1) != 0, admin_behavior: read("ConsentPromptBehaviorAdmin").unwrap_or(5) })
}

#[cfg(not(windows))]
fn windows_uac_state() -> Option<UacState> {
    None
}

fn same_path(a: &Path, b: &Path) -> bool {
    if lc::path_str(a).eq_ignore_ascii_case(&lc::path_str(b)) {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Owned by the administrator and not group- or world-writable — or, under a
/// test root, owned by the user running the suite.
fn trusted_owner(m: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        let owner_ok = match test_root() {
            Some(_) => m.uid() == me,
            None => m.uid() == 0,
        };
        owner_ok && m.mode() & 0o022 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        true
    }
}

fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        Some(unsafe { libc::geteuid() })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// path -> (mtime, len, sha256): hashing a 3 MB helper on every status poll
/// would be wasteful, and mtime+len is enough to know when to redo it.
type HashCache = HashMap<PathBuf, (SystemTime, u64, String)>;

fn sha256_of_file(p: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashCache>> = OnceLock::new();
    let meta = fs::metadata(p).ok()?;
    let key = (meta.modified().ok()?, meta.len());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(c) = cache.lock() {
        if let Some((t, l, s)) = c.get(p) {
            if (*t, *l) == key {
                return Some(s.clone());
            }
        }
    }
    let bytes = fs::read(p).ok()?;
    let s = lc::sha256_hex(&bytes);
    if let Ok(mut c) = cache.lock() {
        c.insert(p.to_path_buf(), (key.0, key.1, s.clone()));
    }
    Some(s)
}

/// The sidecar shipped next to the app executable (one level up for the test
/// binaries, which live in `target/debug/deps`).
pub fn bundled_helper_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { format!("{}.exe", lc::HELPER_BIN_NAME) } else { lc::HELPER_BIN_NAME.to_string() };
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let direct = dir.join(&name);
    if direct.exists() {
        return Some(direct);
    }
    let up = dir.parent()?.join(&name);
    if up.exists() {
        return Some(up);
    }
    None
}

fn app_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.gitswitch.app"))
}

fn jobs_dir() -> Option<PathBuf> {
    Some(app_dir()?.join("lock-jobs"))
}

fn events_path() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("GITSWITCH_EVENTS_LOG") {
        return Some(PathBuf::from(p));
    }
    Some(app_dir()?.join("lock-events.jsonl"))
}

pub enum RegistryRead {
    Missing,
    Untrusted(String),
    Unreadable(String),
    Ok(Box<Registry>),
}

impl RegistryRead {
    fn label(&self) -> &'static str {
        match self {
            RegistryRead::Missing => "missing",
            RegistryRead::Untrusted(_) => "untrusted",
            RegistryRead::Unreadable(_) => "unreadable",
            RegistryRead::Ok(_) => "ok",
        }
    }
    fn ok(&self) -> Option<&Registry> {
        match self {
            RegistryRead::Ok(r) => Some(r),
            _ => None,
        }
    }
}

pub fn read_registry(layout: &Layout) -> RegistryRead {
    let path = &layout.locks_json;
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RegistryRead::Missing,
        Err(e) => return RegistryRead::Unreadable(e.to_string()),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return RegistryRead::Untrusted(format!("{} is not a regular file", path.display()));
    }
    if !trusted_owner(&meta) {
        return RegistryRead::Untrusted(format!("{} is not owned by the administrator", path.display()));
    }
    if let Ok(dm) = fs::symlink_metadata(&layout.registry_dir) {
        if dm.file_type().is_symlink() || !trusted_owner(&dm) {
            return RegistryRead::Untrusted(format!("{} is not an administrator-only directory", layout.registry_dir.display()));
        }
    }
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => return RegistryRead::Unreadable(e.to_string()),
    };
    match serde_json::from_slice::<Registry>(&bytes) {
        Ok(r) if r.schema > lc::SCHEMA => RegistryRead::Unreadable(format!("written by a newer helper (schema {})", r.schema)),
        Ok(r) => RegistryRead::Ok(Box::new(r)),
        Err(e) => RegistryRead::Unreadable(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

struct HelperMeasure {
    state: String,
    installed_sha: Option<String>,
    bundled_sha: Option<String>,
}

fn measure_helper(layout: &Layout, reg: Option<&Registry>) -> HelperMeasure {
    let bundled_sha = bundled_helper_path().and_then(|p| sha256_of_file(&p));
    let m = match fs::symlink_metadata(&layout.helper_path) {
        Ok(m) => m,
        Err(_) => return HelperMeasure { state: "missing".into(), installed_sha: None, bundled_sha },
    };
    if m.file_type().is_symlink() || !m.is_file() || !trusted_owner(&m) {
        return HelperMeasure { state: "untrusted".into(), installed_sha: None, bundled_sha };
    }
    let installed_sha = sha256_of_file(&layout.helper_path);
    let vouched = reg.and_then(|r| r.helper.as_ref()).map(|h| h.sha256.clone());
    let state = match (&installed_sha, &vouched) {
        (Some(s), Some(v)) if s == v => {
            if bundled_sha.as_ref().is_some_and(|b| b != s) {
                "outdated"
            } else {
                "ok"
            }
        }
        _ => "untrusted",
    };
    HelperMeasure { state: state.into(), installed_sha, bundled_sha }
}

fn measure_remote_helper(layout: &Layout, installed_sha: Option<&str>, vouched_sha: Option<&str>) -> String {
    // On Windows the remote helper is a full copy, so it keeps working even
    // when the installed helper itself is gone; judge it by the registry's
    // checksum then.
    let installed_sha = installed_sha.or(vouched_sha);
    let link = &layout.remote_helper_path;
    let dir_ok = link
        .parent()
        .and_then(|d| fs::symlink_metadata(d).ok())
        .map(|m| trusted_owner(&m))
        .unwrap_or(false);
    match fs::symlink_metadata(link) {
        Err(_) => "missing".into(),
        Ok(m) if m.file_type().is_symlink() => match fs::read_link(link) {
            // A link to our helper whose target is gone is as good as absent:
            // git then prints its generic "not a git command" instead of ours.
            Ok(t) if same_path(&t, &layout.helper_path) && !layout.helper_path.exists() => "missing".into(),
            Ok(t) if same_path(&t, &layout.helper_path) => {
                if dir_ok {
                    "ok".into()
                } else {
                    "unprotected-dir".into()
                }
            }
            _ => "foreign".into(),
        },
        Ok(_) => match (sha256_of_file(link), installed_sha) {
            (Some(a), Some(b)) if a == b => {
                if dir_ok {
                    "ok".into()
                } else {
                    "unprotected-dir".into()
                }
            }
            _ => "foreign".into(),
        },
    }
}

/// The marker block's include path in the system gitconfig, and whether the
/// stanza git will read matches what the registry says it should be.
fn measure_system(layout: &Layout, entry: &lc::LockEntry, expected_prefixes: &[String]) -> (String, String, Option<PathBuf>) {
    let include = match fs::read_to_string(&layout.system_gitconfig) {
        Ok(text) => match lc::marker_block_include_path(&text) {
            Some(p) if same_path(Path::new(&p), &layout.include_file) => "ok",
            Some(_) => "wrong-target",
            None => "missing",
        },
        Err(_) => "missing",
    };
    let (stanza, stanza_path) = match fs::read_to_string(&layout.include_file) {
        Ok(text) => {
            let pairs = lc::parse_include_file(&text);
            match pairs.iter().find(|(pat, _)| entry.patterns.iter().any(|p| p.eq_ignore_ascii_case(pat))) {
                Some((_, path)) => {
                    let path = PathBuf::from(path);
                    let mut expected = entry.clone();
                    expected.block_prefixes = expected_prefixes.to_vec();
                    match fs::read_to_string(&path) {
                        Ok(actual) if actual == lc::render_stanza(entry) => {
                            if entry.block_prefixes == expected_prefixes {
                                ("ok", Some(path))
                            } else {
                                // The stanza matches the registry, but the
                                // remotes changed since: the prefix list is old.
                                ("stale", Some(path))
                            }
                        }
                        Ok(_) => ("stale", Some(path)),
                        Err(_) => ("missing", Some(path)),
                    }
                }
                None => ("missing", None),
            }
        }
        Err(_) => ("missing", None),
    };
    (include.into(), stanza.into(), stanza_path)
}

async fn rewrite_measured(repo: &Path, stanza_path: Option<&Path>) -> bool {
    let Some(stanza) = stanza_path else { return false };
    let name = stanza.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let out = sys_git(repo)
        .args([
            "config",
            "--show-scope",
            "--show-origin",
            "--get-all",
            &format!("url.{}.pushInsteadOf", lc::BLOCK_SCHEME),
        ])
        .ok_text()
        .await
        .unwrap_or_default();
    out.lines().any(|l| {
        let mut it = l.split('\t');
        it.next() == Some("system") && it.next().is_some_and(|origin| origin.ends_with(&name))
    })
}

/// What the lock does not stop, in this platform's terms. Shown under the
/// lock, verbatim; the verify suite proves the first three are true.
pub fn caveats(platform: Platform, system_gitconfig: &str, git: &str) -> Vec<String> {
    let mut v = vec![
        "Turning this lock off needs an administrator: its rules live in files only an administrator can change, and GitSwitch will not remove them without the administrator prompt. Removing them by hand needs the same prompt.".to_string(),
        "It stops `git push` — with or without `--no-verify` — from anything that uses this machine's system git in a normal environment: your terminal, editors, scripts and AI agents.".to_string(),
        "It does not stop someone who deliberately works around it. Without any password, a user of this account can: set `GIT_CONFIG_NOSYSTEM=1` (or `GIT_CONFIG_SYSTEM=/dev/null`) so git ignores the system-wide rules; give the remote an explicit push URL (`remote.<name>.pushurl` is exempt from pushInsteadOf, and only the pre-push hook catches it — which `--no-verify` skips); push to a spelling of the URL that no pinned prefix covers; supply their own `git-remote-gitswitch-push-blocked` program; use a different git (Homebrew git, a git library inside an editor, `gh`); or move or copy the repository to another folder.".to_string(),
        "git's own manual says the system scope is not a barrier against the logged-in user: these scopes are assumed to be protected by the user's environment, not against the user (git-config(1), \"Protected configuration\").".to_string(),
        "What it gives you: nobody pushes by accident, a refused push says exactly why and tells an AI agent not to work around it, and turning the lock off leaves an administrator-only record. For a limit nobody on this machine can get around, the remote itself has to enforce it.".to_string(),
    ];
    match platform {
        Platform::Macos => {
            v.push("Anyone who used `sudo` in the same terminal within the last five minutes can run the unlock helper without a new password (sudo's timestamp). macOS also remembers an administrator approval for five minutes for an identical command; GitSwitch binds each command to a one-time job file and its checksum, so a remembered approval can only repeat that exact job.".into());
            v.push(format!("This covers Apple's git ({}) through {}, which survives Command Line Tools updates. A Homebrew git reads a different system file and is not covered.", git, system_gitconfig));
        }
        Platform::Linux => {
            v.push("A live `sudo` timestamp or a NOPASSWD rule lets a terminal run the unlock helper without a new password. Over SSH or on a text console there is no polkit agent, so GitSwitch falls back to showing you the exact command to run.".into());
            v.push(format!("This covers the git at {} through {}. A Nix or Linuxbrew git reads a different file and is not covered.", git, system_gitconfig));
        }
        Platform::Windows => {
            v.push("On an administrator account Windows asks for a click (\"Yes\"), not a password — anyone at your keyboard, or software that can click for you, can approve it. For a real password prompt, work from a standard account and keep a separate administrator account.".into());
            v.push(format!("This covers Git for Windows through {}. WSL git, other git installations and git libraries inside editors are not covered.", system_gitconfig));
        }
    }
    v
}

/// The lock's whole state for one repository. Read-only; the mirror facts
/// come from `push_guard`, which also heals them.
pub async fn measure(repo: &Path, facts: &MirrorFacts) -> LockState {
    let Some(layout) = layout() else {
        return LockState { supported: false, registry: "missing".into(), helper: "missing".into(), remote_helper: "missing".into(), mirrors: "ok".into(), ..Default::default() };
    };
    let platform = layout.platform;
    let reg = read_registry(&layout);
    let helper = measure_helper(&layout, reg.ok());
    let vouched = reg.ok().and_then(|r| r.helper.as_ref()).map(|h| h.sha256.clone());
    let remote_helper = measure_remote_helper(&layout, helper.installed_sha.as_deref(), vouched.as_deref());
    let mut state = LockState {
        supported: true,
        platform: Some(platform.as_str().into()),
        registry: reg.label().into(),
        helper: helper.state.clone(),
        helper_installed: helper.state != "missing",
        remote_helper: remote_helper.clone(),
        mirrors: "ok".into(),
        system_gitconfig: Some(lc::path_str(&layout.system_gitconfig)),
        audit_log: Some(lc::path_str(&layout.audit_log)),
        bundled_helper_sha256: helper.bundled_sha.clone(),
        ..Default::default()
    };
    let repo_c = canonical(repo);
    let entry = reg.ok().and_then(|r| r.find(&repo_c).map(|i| r.locks[i].clone()));
    let Some(entry) = entry else {
        match &reg {
            RegistryRead::Untrusted(why) => state.drift.push(format!("registry-untrusted:{}", why)),
            RegistryRead::Unreadable(why) => state.drift.push(format!("registry-unreadable:{}", why)),
            _ => {}
        }
        return state;
    };

    state.locked = true;
    state.locked_at = Some(entry.locked_at.clone());
    let expected_prefixes = push_guard::block_prefixes(&facts.remote_urls);
    let (include, stanza, stanza_path) = measure_system(&layout, &entry, &expected_prefixes);
    state.system_include = include;
    state.stanza = stanza;
    state.system_rewrite_measured = rewrite_measured(repo, stanza_path.as_deref()).await;

    let mut drift = Vec::new();
    match state.registry.as_str() {
        "ok" => {}
        other => drift.push(format!("registry-{}", other)),
    }
    if state.system_include != "ok" {
        drift.push(format!("system-include-{}", state.system_include));
    }
    if state.stanza != "ok" {
        drift.push(format!("stanza-{}", state.stanza));
    }
    if !state.system_rewrite_measured {
        drift.push("system-rewrite-not-measured".into());
    }
    if state.helper != "ok" {
        drift.push(format!("helper-{}", state.helper));
    }
    if state.remote_helper != "ok" {
        drift.push(format!("remote-helper-{}", state.remote_helper));
    }
    state.needs_elevation = drift.iter().any(|d| {
        d.starts_with("registry-")
            || d.starts_with("system-")
            || d.starts_with("stanza-")
            || (d.starts_with("helper-") && d != "helper-outdated")
            || d == "remote-helper-missing"
            || d == "remote-helper-foreign"
    });

    let mut mirror_drift = Vec::new();
    if !facts.flag {
        mirror_drift.push("mirror-flag".to_string());
    }
    if !facts.rewrite || !facts.uncovered_remotes.is_empty() {
        mirror_drift.push("mirror-rewrite".to_string());
    }
    if !facts.hook_ok {
        mirror_drift.push(if facts.hooks_redirected { "hook-redirected".to_string() } else { "mirror-hook".to_string() });
    }
    for r in &facts.explicit_pushurl {
        drift.push(format!("explicit-pushurl:{}", r));
    }
    state.mirrors = if mirror_drift.is_empty() {
        "ok".into()
    } else if mirror_drift.iter().all(|d| d == "hook-redirected") {
        "unfixable".into()
    } else {
        "drifted".into()
    };
    drift.extend(mirror_drift);
    state.drift = drift;
    state.caveats = caveats(platform, &lc::path_str(&layout.system_gitconfig), &system_git().to_string_lossy());
    state.recent_events = recent_events(&repo_c);
    state
}

// ---------------------------------------------------------------------------
// Self-heal bookkeeping and the user-side event log
// ---------------------------------------------------------------------------

/// May the mirrors be re-applied for this repo right now? Never while a prompt
/// is open, and not more than once per `HEAL_INTERVAL`.
pub fn heal_allowed(repo: &Path) -> bool {
    if crate::elevate::in_flight() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = canonical(repo);
    let map = LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut m) = map.lock() else { return false };
    let now = Instant::now();
    if m.get(&key).is_some_and(|t| now.duration_since(*t) < HEAL_INTERVAL) {
        return false;
    }
    m.insert(key, now);
    true
}

fn now_rfc3339() -> String {
    lc::rfc3339_utc(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

/// Append one line to the user-side log. This log lives in the user's own
/// files and can be edited without a password — the UI says so; the
/// administrator-only record is the helper's audit log.
pub fn log_event(repo: &str, code: &str, detail: &str, healed: bool) {
    let Some(path) = events_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let ev = LockEvent { ts: now_rfc3339(), repo: lc::norm(repo), code: code.into(), detail: detail.into(), healed };
    let Ok(line) = serde_json::to_string(&ev) else { return };
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = existing.lines().collect();
    if lines.len() >= EVENT_LOG_KEEP * 2 {
        lines = lines[lines.len() - EVENT_LOG_KEEP..].to_vec();
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&line);
    out.push('\n');
    let _ = fs::write(&path, out);
}

pub fn recent_events(repo: &str) -> Vec<LockEvent> {
    let Some(path) = events_path() else { return Vec::new() };
    let Ok(text) = fs::read_to_string(&path) else { return Vec::new() };
    let want = lc::norm(repo);
    let mut out: Vec<LockEvent> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("repo").and_then(|r| r.as_str()).is_some_and(|r| lc::same_repo(r, &want)))
        .filter_map(|v| {
            Some(LockEvent {
                ts: v.get("ts")?.as_str()?.to_string(),
                repo: v.get("repo")?.as_str()?.to_string(),
                code: v.get("code")?.as_str()?.to_string(),
                detail: v.get("detail")?.as_str()?.to_string(),
                healed: v.get("healed").and_then(|h| h.as_bool()).unwrap_or(false),
            })
        })
        .collect();
    if out.len() > RECENT_EVENTS {
        out = out[out.len() - RECENT_EVENTS..].to_vec();
    }
    out.reverse();
    out
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// The un-rooted system gitconfig path a job names; the helper compares it
/// with its own constant (macOS, Windows) or allowlist (Linux).
async fn job_system_gitconfig(platform: Platform) -> String {
    let default = lc::path_str(&Layout::new(platform, None, windows_install_path().as_deref()).system_gitconfig);
    if platform != Platform::Linux {
        return default;
    }
    // Ask the system git where its system scope lives; the answer may be a
    // distribution-specific path.
    let out = tokio::process::Command::new(system_git())
        .args(["config", "--system", "--show-origin", "--list"])
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .output()
        .await;
    let Ok(o) = out else { return default };
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    let found = stdout
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("file:"))
        .and_then(|l| l.split('\t').next())
        .map(|s| s.to_string())
        .or_else(|| {
            let start = stderr.find("unable to read config file '")? + "unable to read config file '".len();
            let rest = &stderr[start..];
            Some(rest[..rest.find('\'')?].to_string())
        });
    match found {
        Some(p) => {
            let p = lc::norm(&p);
            match test_root() {
                Some(root) => {
                    let r = lc::path_str(&root);
                    p.strip_prefix(&r).map(|s| s.to_string()).unwrap_or(default)
                }
                None => {
                    if lc::system_gitconfig_allowed(platform, &p) {
                        p
                    } else {
                        default
                    }
                }
            }
        }
        None => default,
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

fn result_path_for(job: &Path) -> PathBuf {
    let mut name = job.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".result.json");
    job.with_file_name(name)
}

fn read_result_file(job: &Path, nonce: &str) -> Option<lc::JobResult> {
    let text = fs::read_to_string(result_path_for(job)).ok()?;
    let r: lc::JobResult = serde_json::from_str(&text).ok()?;
    // A refusal written before the helper knew the nonce carries none; it is
    // accepted only as a failure, never as success.
    if r.nonce == nonce || (r.nonce.is_empty() && !r.ok) {
        Some(r)
    } else {
        None
    }
}

fn tidy(stderr: &str) -> String {
    let t = stderr.trim();
    let t = t.strip_prefix(&format!("{}: ", lc::HELPER_BIN_NAME)).unwrap_or(t);
    t.to_string()
}

fn outcome_from_run(code: i32, stdout: &str, stderr: &str, job: &Path, nonce: &str, bootstrap: bool) -> JobOutcome {
    let from_stdout = serde_json::from_str::<lc::JobResult>(stdout.trim()).ok().filter(|r| r.nonce == nonce);
    let result = from_stdout.or_else(|| read_result_file(job, nonce));
    // UAC gives no stderr back; the helper then leaves its reason in the result file.
    let reason = || -> String {
        let t = tidy(stderr);
        if !t.is_empty() {
            return t;
        }
        result.as_ref().map(|r| r.errors.join("; ")).filter(|s| !s.is_empty()).unwrap_or_else(|| "no details were returned".into())
    };
    match code {
        0 => match result {
            Some(r) => JobOutcome {
                outcome: "applied".into(),
                message: r.message,
                changed: r.changed,
                errors: r.errors,
                bootstrapped: bootstrap,
                ..Default::default()
            },
            None => JobOutcome::simple("failed", "The lock helper reported success, but its result could not be read."),
        },
        2 => JobOutcome::simple("failed", format!("The lock helper rejected the request: {}", reason())),
        3 => JobOutcome::simple("refused", format!("The lock helper refused: {}", reason())),
        4 => {
            let r = result.unwrap_or_else(|| lc::JobResult {
                schema: lc::SCHEMA,
                nonce: nonce.into(),
                ok: false,
                helper_version: String::new(),
                changed: Vec::new(),
                errors: vec![tidy(stderr)],
                message: String::new(),
                registry_sha256: None,
            });
            JobOutcome {
                outcome: "failed".into(),
                message: format!("Some changes could not be applied. {}", r.message),
                changed: r.changed,
                errors: r.errors,
                bootstrapped: bootstrap,
                ..Default::default()
            }
        }
        6 => JobOutcome::simple("denied", "The lock helper did not receive administrator rights."),
        7 => JobOutcome::simple("failed", "The installed lock helper is older than its own registry. Upgrade it from Doctor."),
        other => JobOutcome::simple("failed", format!("The lock helper failed (exit {}): {}", other, reason())),
    }
}

/// Hand `ops` to the helper behind the administrator prompt. Picks the
/// installed helper when the registry vouches for it, otherwise the bundled
/// sidecar in `--bootstrap` mode. Never runs anything itself as root.
pub async fn run_job(mut ops: Vec<Op>) -> JobOutcome {
    let Some(layout) = layout() else {
        return JobOutcome::simple("unsupported", "Push locks are not supported on this operating system.");
    };
    let platform = layout.platform;
    let Some(_guard) = crate::elevate::begin() else {
        return JobOutcome::simple("busy", "Another administrator request is still open. Finish or cancel it first.");
    };

    let reg = read_registry(&layout);
    let helper = measure_helper(&layout, reg.ok());
    // The installed copy runs jobs only while it is the one this GitSwitch
    // ships (or a signed upgrade is available). An outdated or untrusted copy
    // is replaced first by re-bootstrapping the bundled helper — the same
    // trust step as the first install, and the UI says so before the prompt.
    let signed_upgrade = helper.state == "outdated" && matches!(upgrade_op(), Some(Op::UpgradeHelper { sig: Some(_), .. }));
    let installed_usable = matches!(reg, RegistryRead::Ok(_)) && (helper.state == "ok" || signed_upgrade);
    let (program, bootstrap) = if installed_usable {
        (layout.helper_path.clone(), false)
    } else {
        match bundled_helper_path() {
            Some(p) => (p, true),
            None => return JobOutcome::simple("failed", "The lock helper is missing from this GitSwitch installation. Reinstall GitSwitch."),
        }
    };
    if bootstrap && !ops.iter().any(|o| matches!(o, Op::Bootstrap)) {
        ops.insert(0, Op::Bootstrap);
    }
    if (bootstrap && helper.state != "missing" || signed_upgrade) && !ops.iter().any(|o| matches!(o, Op::UpgradeHelper { .. })) {
        if let Some(op) = upgrade_op() {
            ops.push(op);
        }
    }

    let job = lc::Job {
        schema: lc::SCHEMA,
        nonce: uuid::Uuid::new_v4().to_string(),
        app_version: APP_VERSION.into(),
        platform,
        system_gitconfig: job_system_gitconfig(platform).await,
        requested_by_uid: current_uid(),
        requested_by_name: std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok(),
        ops,
    };
    if let Err(e) = lc::validate_job(&job, platform) {
        return JobOutcome::simple("refused", format!("This request cannot be made safely: {}", e));
    }
    let Some(dir) = jobs_dir() else {
        return JobOutcome::simple("failed", "No application data directory is available.");
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        return JobOutcome::simple("failed", format!("Cannot create {}: {}", dir.display(), e));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let job_path = dir.join(format!("{}.json", job.nonce));
    let bytes = match serde_json::to_vec(&job) {
        Ok(b) => b,
        Err(e) => return JobOutcome::simple("failed", format!("Cannot encode the job: {}", e)),
    };
    if let Err(e) = write_private(&job_path, &bytes) {
        return JobOutcome::simple("failed", format!("Cannot write the job file: {}", e));
    }
    let sha = lc::sha256_hex(&bytes);
    let _ = fs::remove_file(result_path_for(&job_path));

    let elev = crate::elevate::run(&program, &job_path, &sha, bootstrap).await;
    let mut out = match elev {
        crate::elevate::Elevated::Ran { code, stdout, stderr } => outcome_from_run(code, &stdout, &stderr, &job_path, &job.nonce, bootstrap),
        crate::elevate::Elevated::Cancelled => JobOutcome::simple("cancelled", "You cancelled the administrator prompt. Nothing changed."),
        crate::elevate::Elevated::Denied => JobOutcome::simple("denied", "The administrator password was not accepted. Nothing changed."),
        crate::elevate::Elevated::NoAgent => JobOutcome::simple("no-agent", "There is no way to show an administrator prompt in this session. Nothing changed."),
        crate::elevate::Elevated::Manual { command } => JobOutcome {
            outcome: "manual-required".into(),
            message: "No administrator prompt is available here. Run this command in a terminal as an administrator, then choose \"I ran it\".".into(),
            command: Some(command),
            job_nonce: Some(job.nonce.clone()),
            ..Default::default()
        },
        crate::elevate::Elevated::Unsupported(m) => JobOutcome::simple("unsupported", m),
        crate::elevate::Elevated::Failed(m) => JobOutcome::simple("failed", m),
    };
    if out.job_nonce.is_none() {
        out.job_nonce = Some(job.nonce.clone());
    }
    if out.outcome != "manual-required" {
        let _ = fs::remove_file(&job_path);
        let _ = fs::remove_file(result_path_for(&job_path));
    }
    out
}

/// After a `manual-required` outcome: read the result the helper wrote for
/// that job, if the person ran the command.
pub async fn finish_manual(nonce: &str) -> JobOutcome {
    if lc::validate_path(&format!("/{}", nonce)).is_err() || nonce.contains('/') {
        return JobOutcome::simple("failed", "That is not a job identifier.");
    }
    let Some(dir) = jobs_dir() else {
        return JobOutcome::simple("failed", "No application data directory is available.");
    };
    let job_path = dir.join(format!("{}.json", nonce));
    match read_result_file(&job_path, nonce) {
        Some(r) => {
            let _ = fs::remove_file(&job_path);
            let _ = fs::remove_file(result_path_for(&job_path));
            JobOutcome {
                outcome: if r.ok { "applied".into() } else { "failed".into() },
                message: r.message,
                changed: r.changed,
                errors: r.errors,
                ..Default::default()
            }
        }
        None => JobOutcome::simple("failed", "No result from the helper yet. Run the command first, then try again."),
    }
}

// ---------------------------------------------------------------------------
// Mode switching and repairs
// ---------------------------------------------------------------------------

async fn lock_op_for(repo_path: &str) -> Result<Op, AppError> {
    let repo = Path::new(repo_path);
    let repo_c = canonical(repo);
    let gitdir = sys_git(repo)
        .args(["rev-parse", "--absolute-git-dir"])
        .ok_text()
        .await
        .ok_or_else(|| AppError::Command("Could not resolve this repository's git directory".into()))?;
    let gitdir_c = canonical(Path::new(&gitdir));
    let urls = push_guard::remote_urls(repo).await;
    Ok(Op::Lock {
        repo: repo_c.clone(),
        gitdir: gitdir_c,
        block_prefixes: push_guard::block_prefixes(&urls),
        label: crate::paths::base_name(&repo_c),
    })
}

fn result_with_state(out: JobOutcome, state: Option<push_guard::PushState>) -> PushModeResult {
    PushModeResult { outcome: out.outcome, message: out.message, command: out.command, job_nonce: out.job_nonce, state }
}

/// "allowed" | "guardrail" | "locked". Locking and unlocking go through the
/// helper and the prompt; the guard-rail is the user's own setting.
pub async fn set_push_mode(repo_path: &str, mode: &str) -> Result<PushModeResult, AppError> {
    let before = crate::git_status::push_state_for(repo_path).await?;
    let hooks = crate::git_status::git_paths(Path::new(repo_path)).await?.hooks;
    let profile = crate::git_status::owning_profile_id(repo_path);
    match mode {
        "locked" => {
            if before.lock.locked && !before.lock.needs_elevation {
                let state = push_guard::repair(repo_path, &hooks, profile.as_deref()).await?;
                return Ok(result_with_state(JobOutcome::simple("applied", "Already locked."), Some(state)));
            }
            let op = lock_op_for(repo_path).await?;
            let mut ops = vec![op];
            if before.lock.locked {
                ops.push(Op::RepairSystem);
                ops.push(Op::InstallRemoteHelper);
            }
            let out = run_job(ops).await;
            if out.applied() {
                push_guard::apply_mirrors(Path::new(repo_path), &hooks).await?;
                log_event(&canonical(Path::new(repo_path)), "locked", &out.message, false);
            }
            let state = if out.outcome == "manual-required" { None } else { Some(crate::git_status::push_state_for(repo_path).await?) };
            Ok(result_with_state(out, state))
        }
        "allowed" => {
            if before.lock.locked {
                let out = run_job(vec![Op::Unlock { repo: canonical(Path::new(repo_path)) }]).await;
                if !out.applied() {
                    let state = if out.outcome == "manual-required" { None } else { Some(before) };
                    return Ok(result_with_state(out, state));
                }
                log_event(&canonical(Path::new(repo_path)), "unlocked", &out.message, false);
                push_guard::remove_mirrors(Path::new(repo_path), &hooks).await?;
                let state = crate::git_status::push_state_for(repo_path).await?;
                return Ok(result_with_state(out, Some(state)));
            }
            let state = push_guard::set_repo_blocked(repo_path, &hooks, false, profile.as_deref()).await?;
            Ok(result_with_state(JobOutcome::simple("applied", "Pushes are allowed again."), Some(state)))
        }
        "guardrail" => {
            if before.lock.locked {
                return Err(AppError::Config(
                    "This repository is locked. Unlock it first — that needs the administrator password.".into(),
                ));
            }
            let state = push_guard::set_repo_blocked(repo_path, &hooks, true, profile.as_deref()).await?;
            Ok(result_with_state(JobOutcome::simple("applied", "Pushes are blocked (guard-rail)."), Some(state)))
        }
        other => Err(AppError::Config(format!("Unknown push mode '{}'", other))),
    }
}

/// Repair everything the lock needs for one repository: the mirrors (no
/// prompt) and, when the admin-owned layers drifted, one elevated job.
pub async fn repair_lock(repo_path: &str) -> Result<PushModeResult, AppError> {
    let hooks = crate::git_status::git_paths(Path::new(repo_path)).await?.hooks;
    let profile = crate::git_status::owning_profile_id(repo_path);
    let state = push_guard::repair(repo_path, &hooks, profile.as_deref()).await?;
    if !state.lock.locked {
        return Ok(result_with_state(JobOutcome::simple("applied", "This repository is not locked; the guard-rail was re-applied."), Some(state)));
    }
    if !state.lock.needs_elevation && state.lock.helper != "outdated" {
        return Ok(result_with_state(JobOutcome::simple("applied", "Everything is in place."), Some(state)));
    }
    let mut ops = vec![lock_op_for(repo_path).await?, Op::RepairSystem, Op::InstallRemoteHelper];
    if state.lock.helper == "outdated" {
        if let Some(op) = upgrade_op() {
            ops.push(op);
        }
    }
    let out = run_job(ops).await;
    if out.applied() {
        log_event(&canonical(Path::new(repo_path)), "repaired", &out.message, true);
    }
    let state = if out.outcome == "manual-required" { None } else { Some(crate::git_status::push_state_for(repo_path).await?) };
    Ok(result_with_state(out, state))
}

/// An upgrade from the bundled sidecar: signed when a `.sig` ships next to it,
/// otherwise an explicit re-bootstrap (the same trust step as the first install).
fn upgrade_op() -> Option<Op> {
    let bundled = bundled_helper_path()?;
    let mut sig = bundled.clone().into_os_string();
    sig.push(".sig");
    let sig = PathBuf::from(sig);
    Some(Op::UpgradeHelper {
        source: canonical(&bundled),
        sig: if sig.exists() { Some(canonical(&sig)) } else { None },
        version: bundled_version().unwrap_or_else(|| APP_VERSION.into()),
    })
}

fn bundled_version() -> Option<String> {
    let out = std::process::Command::new(bundled_helper_path()?).arg("--version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace().nth(1).map(|v| v.to_string())
}

pub async fn helper_status() -> HelperStatus {
    let Some(layout) = layout() else {
        return HelperStatus { supported: false, helper: "missing".into(), registry: "missing".into(), remote_helper: "missing".into(), ..Default::default() };
    };
    let reg = read_registry(&layout);
    let helper = measure_helper(&layout, reg.ok());
    let bundled = bundled_helper_path();
    let bundled_signed = bundled.as_ref().is_some_and(|b| {
        let mut s = b.clone().into_os_string();
        s.push(".sig");
        PathBuf::from(s).exists()
    });
    HelperStatus {
        supported: true,
        platform: Some(layout.platform.as_str().into()),
        helper_path: Some(lc::path_str(&layout.helper_path)),
        installed: helper.state != "missing",
        installed_version: reg.ok().and_then(|r| r.helper.as_ref()).map(|h| h.version.clone()),
        installed_sha256: helper.installed_sha.clone(),
        helper: helper.state.clone(),
        bundled_path: bundled.as_ref().map(|p| lc::path_str(p)),
        bundled_version: bundled_version(),
        bundled_sha256: helper.bundled_sha.clone(),
        bundled_signed,
        registry: reg.label().into(),
        locks: reg
            .ok()
            .map(|r| {
                r.locks
                    .iter()
                    .map(|l| LockSummary { repo: l.repo.clone(), label: l.label.clone(), locked_at: l.locked_at.clone(), exists: Path::new(&l.repo).is_dir() })
                    .collect()
            })
            .unwrap_or_default(),
        system_gitconfig: Some(lc::path_str(&layout.system_gitconfig)),
        remote_helper: measure_remote_helper(&layout, helper.installed_sha.as_deref(), reg.ok().and_then(|r| r.helper.as_ref()).map(|h| h.sha256.as_str())),
        remote_helper_path: Some(lc::path_str(&layout.remote_helper_path)),
        registry_dir: Some(lc::path_str(&layout.registry_dir)),
        audit_log: Some(lc::path_str(&layout.audit_log)),
        events_log: events_path().map(|p| lc::path_str(&p)),
        uac: windows_uac_state(),
    }
}

/// Remove every lock, the registry, the remote helper and the helper (one
/// prompt), then the per-repo mirrors, which are the user's own files.
pub async fn uninstall_all() -> JobOutcome {
    let repos: Vec<String> = layout()
        .map(|l| read_registry(&l))
        .and_then(|r| r.ok().map(|r| r.locks.iter().map(|l| l.repo.clone()).collect()))
        .unwrap_or_default();
    let out = run_job(vec![Op::UninstallAll]).await;
    if out.applied() {
        for repo in repos {
            if let Ok(paths) = crate::git_status::git_paths(Path::new(&repo)).await {
                let _ = push_guard::remove_mirrors(Path::new(&repo), &paths.hooks).await;
            }
            log_event(&repo, "uninstalled", "push locks and helper removed", false);
        }
    }
    out
}

/// Doctor's fix ids, all prefixed `lock-`.
pub async fn fix(fix_id: &str) -> JobOutcome {
    match fix_id {
        "lock-bootstrap" | "lock-upgrade-helper" => {
            let mut ops = vec![Op::RepairSystem, Op::InstallRemoteHelper];
            if let Some(op) = upgrade_op() {
                ops.push(op);
            }
            run_job(ops).await
        }
        "lock-repair-system" => run_job(vec![Op::RepairSystem, Op::InstallRemoteHelper]).await,
        "lock-install-remote-helper" => run_job(vec![Op::InstallRemoteHelper]).await,
        other => {
            if let Some(repo) = other.strip_prefix("lock-forget:") {
                let out = run_job(vec![Op::Unlock { repo: lc::norm(repo) }]).await;
                if out.applied() {
                    if let Ok(paths) = crate::git_status::git_paths(Path::new(repo)).await {
                        let _ = push_guard::remove_mirrors(Path::new(repo), &paths.hooks).await;
                    }
                    log_event(repo, "forgotten", "lock removed for a repository that no longer exists", false);
                }
                return out;
            }
            if let Some(repo) = other.strip_prefix("lock-repair:") {
                return match repair_lock(repo).await {
                    Ok(r) => JobOutcome { outcome: r.outcome, message: r.message, command: r.command, job_nonce: r.job_nonce, ..Default::default() },
                    Err(e) => JobOutcome::simple("failed", e.to_string()),
                };
            }
            if let Some(repo) = other.strip_prefix("lock-repair-mirrors:") {
                return match crate::git_status::repair_push_block_for(repo).await {
                    Ok(state) => JobOutcome::simple("applied", if state.lock.mirrors == "ok" || state.lock.mirrors == "healed" { "The user-level mirrors are back in place." } else { "The mirrors could not all be re-applied; see the card." }),
                    Err(e) => JobOutcome::simple("failed", e.to_string()),
                };
            }
            if let Some(repo) = other.strip_prefix("lock-neutralise-pushurl:") {
                return neutralise_pushurl(repo).await;
            }
            JobOutcome::simple("failed", format!("Unknown fix {}", other))
        }
    }
}

/// Store each explicit pushurl under `gitswitch.savedpushurl.<remote>` (all
/// lowercase: git config subsection names are case-sensitive) and
/// unset it, so the rewrite applies. Unlocking restores them.
async fn neutralise_pushurl(repo_path: &str) -> JobOutcome {
    let repo = Path::new(repo_path);
    let mut changed = Vec::new();
    for name in push_guard::remote_names_of(repo).await {
        let key = format!("remote.{}.pushurl", name);
        let Some(url) = GitCmd::at(repo).args(["config", "--get", &key]).ok_text().await.filter(|s| !s.is_empty()) else { continue };
        if GitCmd::at(repo).args(["config", "--local", &format!("gitswitch.savedpushurl.{}", name), &url]).text().await.is_err() {
            continue;
        }
        let _ = GitCmd::at(repo).args(["config", "--local", "--unset", &key]).run().await;
        changed.push(lc::Change { op: "neutralise-pushurl".into(), repo: Some(lc::norm(repo_path)), detail: format!("remote {}: pushurl stored and removed", name) });
    }
    let n = changed.len();
    JobOutcome {
        outcome: "applied".into(),
        message: if n == 0 { "No remote here has an explicit push URL.".into() } else { format!("{} explicit push URL(s) stored and removed; the rewrite now applies to them.", n) },
        changed,
        ..Default::default()
    }
}

/// Put back any pushurl `neutralise_pushurl` stored.
pub async fn restore_pushurls(repo: &Path) {
    let Some(out) = GitCmd::at(repo).args(["config", "--local", "--get-regexp", r"^gitswitch\.savedpushurl\."]).ok_text().await else { return };
    for line in out.lines() {
        let Some((key, url)) = line.split_once(' ') else { continue };
        let Some(name) = key.strip_prefix("gitswitch.savedpushurl.") else { continue };
        let _ = GitCmd::at(repo).args(["config", "--local", &format!("remote.{}.pushurl", name), url]).run().await;
        let _ = GitCmd::at(repo).args(["config", "--local", "--unset", key]).run().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caveats_tell_the_truth_and_name_the_platform_file() {
        let v = caveats(Platform::Macos, "/etc/gitconfig", "/usr/bin/git");
        assert!(v.iter().any(|c| c.contains("GIT_CONFIG_NOSYSTEM=1")));
        assert!(v.iter().any(|c| c.contains("administrator")));
        assert!(v.iter().any(|c| c.contains("/etc/gitconfig") && c.contains("/usr/bin/git")));
        assert!(v.iter().any(|c| c.contains("sudo")));
        assert!(!v.iter().any(|c| c.contains("GitHub")), "no remote-side advice: the user chose a local lock");
        let w = caveats(Platform::Windows, "C:/Program Files/Git/etc/gitconfig", "git");
        assert!(w.iter().any(|c| c.contains("click")));
    }

    #[test]
    fn windows_verbatim_prefixes_are_stripped() {
        assert_eq!(strip_verbatim("//?/C:/Users/me/repo"), "C:/Users/me/repo");
        assert_eq!(strip_verbatim("//?/UNC/server/share/repo"), "//server/share/repo");
        assert_eq!(strip_verbatim("/Users/me/repo"), "/Users/me/repo");
        assert!(lc::validate_path(&strip_verbatim("//?/C:/Users/me/repo")).is_ok());
    }

    #[test]
    fn the_release_key_in_lock_core_matches_tauri_conf() {
        let conf = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert_eq!(v["plugins"]["updater"]["pubkey"].as_str().unwrap(), lc::RELEASE_PUBKEY_B64);
        assert_eq!(v["bundle"]["externalBin"][0].as_str().unwrap(), "binaries/gitswitch-lock-helper");
    }

    #[test]
    fn run_outcomes_map_exit_codes_to_sentences() {
        let tmp = std::env::temp_dir().join(format!("gitswitch-lock-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let job = tmp.join("j.json");
        let ok_json = r#"{"schema":1,"nonce":"n","ok":true,"helper_version":"0.1.0","changed":[{"op":"lock","repo":"/r","detail":"locked"}],"errors":[],"message":"Locked 1 repository."}"#;
        let o = outcome_from_run(0, ok_json, "", &job, "n", true);
        assert_eq!(o.outcome, "applied");
        assert_eq!(o.message, "Locked 1 repository.");
        assert!(o.bootstrapped);
        // A result for another nonce is not accepted.
        let o = outcome_from_run(0, ok_json, "", &job, "other", false);
        assert_eq!(o.outcome, "failed");
        // With stdout swallowed (AppleScript raised), the result file is read.
        fs::write(result_path_for(&job), ok_json).unwrap();
        let o = outcome_from_run(0, "", "", &job, "n", false);
        assert_eq!(o.outcome, "applied");
        let o = outcome_from_run(3, "", "gitswitch-lock-helper: the job file does not match the checksum that was approved", &job, "n", false);
        assert_eq!(o.outcome, "refused");
        assert!(o.message.contains("does not match the checksum"));
        assert!(!o.message.contains("gitswitch-lock-helper:"));
        assert_eq!(outcome_from_run(6, "", "", &job, "n", false).outcome, "denied");
        assert_eq!(outcome_from_run(2, "", "usage", &job, "n", false).outcome, "failed");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn events_round_trip_newest_first_and_stay_per_repo() {
        // Never the real profile's log: unit tests write to a temp file.
        let log = std::env::temp_dir().join(format!("gitswitch-events-test-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&log);
        std::env::set_var("GITSWITCH_EVENTS_LOG", &log);
        let repo = format!("/tmp/gitswitch-events-test-{}", std::process::id());
        log_event(&repo, "a", "first", false);
        log_event(&repo, "b", "second", true);
        log_event("/tmp/somewhere-else", "c", "other repo", false);
        let ev = recent_events(&repo);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].code, "b");
        assert!(ev[0].healed);
        assert_eq!(ev[1].code, "a");
        let _ = fs::remove_file(&log);
    }
}
