//! Blocking pushes for a single repository, in a way that also holds in the
//! user's terminal.
//!
//! Three mechanisms, because each has a blind spot another covers:
//!
//! 1. A `pushInsteadOf` rewrite in the repo's own `.git/config`. Transport
//!    level, so `--no-verify` cannot get past it. It misses a remote with an
//!    explicit `remote.<name>.pushurl` (exempt from pushInsteadOf by git's
//!    design), a spelling of the URL no pinned prefix covers, and anything
//!    that skips or overrides config altogether: `GIT_CONFIG_*`, another git.
//! 2. A `pre-push` hook. Catches every remote and every URL form, but is
//!    skipped by `--no-verify` and by `core.hooksPath` pointing elsewhere.
//! 3. The push lock (`push_lock.rs`): the same rewrite, written by the
//!    privileged helper into admin-owned system-scope config, so removing it
//!    needs the administrator password. Under a lock, 1 and 2 are its
//!    user-level mirrors and are re-applied whenever they drift.
//!
//! 1 and 2 read the same flag (`gitswitch.pushBlocked`), so toggling never
//! rewrites the hook. Whatever is left uncovered is reported in `gaps` — and
//! for the lock in `lock.caveats` — rather than papered over. Without the lock
//! this is a guard-rail, not a lock, and the UI says so.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use crate::profiles;
use crate::push_lock::{self, LockState, MirrorFacts};
use gitswitch_lock_core as lc;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The URL scheme pushes get rewritten to. git has no such remote helper, so
/// the push fails before anything leaves the machine — unless GitSwitch's own
/// helper of that name is installed, whose only job is to explain why.
pub const BLOCK_SCHEME: &str = lc::BLOCK_SCHEME;
const FLAG: &str = "gitswitch.pushBlocked";
pub(crate) const HOOK_MARKER: &str = ">>> GitSwitch push guard >>>";

/// GitHub in every form git accepts, so a rewrite covers the common cases even
/// before per-remote prefixes are added. Mirrors `git_config::push_block_section`
/// except for its empty catch-all prefix, which only a profile's block uses:
/// these also go into lock jobs, whose validator refuses an empty prefix.
const BASE_PREFIXES: &[&str] = &[
    "git@github.com:",
    "git@github.com/",
    "ssh://git@github.com/",
    "ssh://git@github.com:22/",
    "ssh://git@ssh.github.com/",
    "ssh://git@ssh.github.com:443/",
    "https://github.com/",
    "http://github.com/",
    "github.com:",
];

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct RemotePushState {
    pub name: String,
    pub fetch_url: String,
    /// The push URL *after* any rewrite — i.e. what git would really use.
    pub push_url: String,
    pub rewritten: bool,
    pub has_explicit_pushurl: bool,
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct PushState {
    pub profile_blocked: bool,
    pub profile_name: Option<String>,
    pub repo_blocked: bool,
    /// The effective answer: profile, repo guard-rail or lock. More
    /// restrictive wins *in the UI*: the app never writes a longer rewrite to
    /// re-allow a push. git itself would honour one (longest match wins in
    /// any scope), which is one of the holes `gaps` and `lock.caveats` name.
    pub blocked: bool,
    pub reason: String,
    /// "gitswitch" | "chained" | "foreign" | "none"
    pub hook: String,
    pub hooks_path_overridden: Option<String>,
    pub config_block_present: bool,
    pub remotes: Vec<RemotePushState>,
    /// What this block does *not* cover, in plain words. Empty when nothing
    /// relevant is missing.
    pub gaps: Vec<String>,
    /// Enforcement has drifted from the flag — e.g. a remote was added after
    /// the block went on.
    pub needs_repair: bool,
    /// The tamper-resistant lock, measured.
    pub lock: LockState,
}

/// The longest prefix of a remote URL that identifies "this host, this access
/// method". `None` when no safe prefix can be derived (a local path remote),
/// because a wrong prefix would rewrite more than intended.
pub fn derive_push_prefix(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('/') || url.starts_with('.') || url.starts_with('~') {
        return None;
    }
    if let Some(idx) = url.find("://") {
        let after = &url[idx + 3..];
        let end = after.find('/')?;
        return Some(format!("{}{}/", &url[..idx + 3], &after[..end]));
    }
    // scp-like: host:path, or an ssh config alias: alias:path
    let colon = url.find(':')?;
    let head = &url[..colon];
    if head.is_empty()
        || head.contains('/')
        || head.contains('\\')
        // A Windows drive letter, not a host.
        || head.len() == 1
    {
        return None;
    }
    Some(format!("{}:", head))
}

/// Every prefix to rewrite for this repo: the GitHub forms, one derived from
/// each configured remote (which closes the ssh-alias hole the profile-level
/// block cannot see), and each remote's exact URL.
///
/// The exact URL matters: `pushInsteadOf` matches by prefix, and a whole URL is
/// simply the most specific prefix there is. Without it, a remote whose form we
/// decline to generalise — a filesystem path, an unfamiliar scheme — would be
/// left to the hook alone, and `--no-verify` would walk straight past it.
pub fn block_prefixes(remote_urls: &[String]) -> Vec<String> {
    let mut out: Vec<String> = BASE_PREFIXES.iter().map(|s| s.to_string()).collect();
    let mut add = |value: String| {
        if !value.is_empty() && !out.contains(&value) {
            out.push(value);
        }
    };
    for url in remote_urls {
        if let Some(prefix) = derive_push_prefix(url) {
            add(prefix);
        }
        add(url.trim().to_string());
    }
    out
}

/// The hook reads the flags rather than hard-coding "blocked", so turning the
/// block on and off is one `git config` write and never a hook rewrite.
/// `gitswitch.locked` comes from the admin-owned stanza and cannot be unset
/// locally. Deliberately does not read stdin: git feeds ref updates there and
/// the chained hook (Git LFS, in argos) needs them untouched. And it never
/// prints a command that would disable it — the earlier version did.
pub fn hook_body(chain: bool) -> String {
    let t = |s: &str| format!("  echo \"{}\" >&2\n", s.replace('"', "\\\""));
    let mut body = String::new();
    body.push_str("#!/bin/sh\n");
    body.push_str("# >>> GitSwitch push guard >>>\n");
    body.push_str("# Managed by GitSwitch. Blocks pushes while this repository is blocked or locked in GitSwitch.\n");
    body.push_str("locked=\"$(git config --bool --get gitswitch.locked 2>/dev/null)\"\n");
    body.push_str("blocked=\"$(git config --bool --get gitswitch.pushBlocked 2>/dev/null)\"\n");
    body.push_str("if [ \"$locked\" = \"true\" ]; then\n");
    body.push_str(&t(lc::text::LOCKED_HEAD));
    body.push_str(&t(lc::text::LOCKED_WHO));
    body.push_str("  echo \"  remote: $1\" >&2\n");
    body.push_str(&t(lc::text::LOCKED_ASK));
    body.push_str(&t(lc::text::LOCKED_UNLOCK));
    body.push_str(&t(lc::text::AGENT_TRAILER));
    body.push_str("  exit 1\nfi\n");
    body.push_str("if [ \"$blocked\" = \"true\" ]; then\n");
    body.push_str(&t(lc::text::GUARDRAIL_HEAD));
    body.push_str("  echo \"  remote: $1\" >&2\n");
    body.push_str(&t(lc::text::GUARDRAIL_WHERE));
    body.push_str("  exit 1\nfi\n");
    body.push_str("# <<< GitSwitch push guard <<<\n");
    if chain {
        body.push_str(CHAIN_SNIPPET);
    }
    body.push('\n');
    body
}

const CHAIN_SNIPPET: &str = "# run the hook that was here before GitSwitch\nif [ -x \"$(dirname \"$0\")/pre-push.gitswitch-saved\" ]; then\n  \"$(dirname \"$0\")/pre-push.gitswitch-saved\" \"$@\" || exit $?\nfi";

fn hook_kind(hooks: &Path) -> String {
    let hook = hooks.join("pre-push");
    match std::fs::read_to_string(&hook) {
        Ok(content) if content.contains(HOOK_MARKER) => {
            if hooks.join("pre-push.gitswitch-saved").exists() {
                "chained".into()
            } else {
                "gitswitch".into()
            }
        }
        Ok(_) => "foreign".into(),
        Err(_) => "none".into(),
    }
}

async fn remote_names(repo: &Path) -> Vec<String> {
    GitCmd::at(repo)
        .args(["remote"])
        .ok_text()
        .await
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && crate::git_exec::validate_remote_name(l).is_ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Validated remote names, for callers outside this module.
pub async fn remote_names_of(repo: &Path) -> Vec<String> {
    remote_names(repo).await
}

/// Every remote's fetch URL, unmasked — for building prefixes, never for display.
pub async fn remote_urls(repo: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for name in remote_names(repo).await {
        if let Some(u) = GitCmd::at(repo).args(["remote", "get-url", &name]).ok_text().await {
            if !u.is_empty() {
                v.push(u);
            }
        }
    }
    v
}

async fn read_remotes(repo: &Path) -> Vec<RemotePushState> {
    let mut out = Vec::new();
    for name in remote_names(repo).await {
        let fetch_url = GitCmd::at(repo)
            .args(["remote", "get-url", &name])
            .ok_text()
            .await
            .unwrap_or_default();
        // The authority on whether the block took effect for this remote: git
        // applies the rewrite here, so this is a measurement, not a guess.
        let push_url = GitCmd::at(repo)
            .args(["remote", "get-url", "--push", &name])
            .ok_text()
            .await
            .unwrap_or_default();
        let has_explicit_pushurl = GitCmd::at(repo)
            .args(["config", "--get", &format!("remote.{}.pushurl", name)])
            .ok_text()
            .await
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        out.push(RemotePushState {
            rewritten: push_url.starts_with(BLOCK_SCHEME),
            fetch_url: crate::repo_scan::mask_token(&fetch_url),
            push_url: crate::repo_scan::mask_token(&push_url),
            has_explicit_pushurl,
            name,
        });
    }
    out
}

/// Holes that remain for *this* repo, given what is actually installed.
pub fn gaps_for(
    blocked: bool,
    locked: bool,
    hook: &str,
    hooks_path_overridden: Option<&str>,
    remotes: &[RemotePushState],
) -> Vec<String> {
    let mut gaps = Vec::new();
    if !blocked {
        return gaps;
    }
    let hook_active = hook == "gitswitch" || hook == "chained";
    for r in remotes {
        if !r.rewritten {
            gaps.push(format!(
                "Pushing to '{}' isn't stopped by the URL rewrite{}. {}",
                r.name,
                if r.has_explicit_pushurl {
                    " because that remote sets its own pushurl"
                } else {
                    ""
                },
                if hook_active {
                    "Only the pre-push hook stops it, and `git push --no-verify` skips hooks."
                } else {
                    "Nothing local stops a push to it."
                }
            ));
        }
    }
    if let Some(path) = hooks_path_overridden {
        gaps.push(format!(
            "core.hooksPath points at {}, so GitSwitch's pre-push hook isn't the one git runs.",
            path
        ));
    } else if hook == "foreign" {
        gaps.push(
            "This repo already has its own pre-push hook, so GitSwitch didn't replace it."
                .to_string(),
        );
    } else if hook == "none" {
        gaps.push("No pre-push hook is installed, so only the URL rewrite applies.".to_string());
    } else {
        gaps.push("`git push --no-verify` skips the hook; the URL rewrite still applies.".to_string());
    }
    if !locked {
        gaps.push(
            "This is a guard-rail, not a lock: anyone with a terminal can turn it off with one git command, and a more specific `pushInsteadOf` of their own — or `GIT_CONFIG_NOSYSTEM=1` — gets past it. For enforcement that needs the administrator password to remove, use the lock."
                .to_string(),
        );
    }
    gaps
}

/// One sentence naming who is blocking, so the UI never has to infer it.
pub fn block_reason(
    profile_blocked: bool,
    profile_name: Option<&str>,
    repo_blocked: bool,
    locked: bool,
) -> String {
    if locked {
        let mut s = "Locked for this repository. Unlocking needs the administrator password.".to_string();
        if profile_blocked {
            s.push_str(&format!(" Profile '{}' blocks pushes here as well.", profile_name.unwrap_or("?")));
        }
        return s;
    }
    match (profile_blocked, repo_blocked) {
        (false, false) => "Pushing is allowed from this folder.".into(),
        (true, false) => format!(
            "Blocked by profile '{}'. Allow pushes for that profile, or move this folder.",
            profile_name.unwrap_or("?")
        ),
        (false, true) => "Blocked for this repository (guard-rail).".into(),
        (true, true) => format!(
            "Blocked for this repository, and by profile '{}' — unblocking the repo alone won't be enough.",
            profile_name.unwrap_or("?")
        ),
    }
}

pub(crate) async fn read_flag(repo: &Path) -> bool {
    GitCmd::at(repo)
        .args(["config", "--local", "--bool", "--get", FLAG])
        .ok_text()
        .await
        .map(|v| v == "true")
        .unwrap_or(false)
}

async fn read_rewrite_present(repo: &Path) -> bool {
    GitCmd::at(repo)
        .args([
            "config",
            "--local",
            "--get-all",
            &format!("url.{}.pushInsteadOf", BLOCK_SCHEME),
        ])
        .ok_text()
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

async fn read_hooks_path(repo: &Path) -> Option<String> {
    GitCmd::at(repo)
        .args(["config", "--get", "core.hooksPath"])
        .ok_text()
        .await
        .filter(|s| !s.is_empty())
}

/// One measurement or change of a repository's user-level pieces at a time.
/// A measurement taken while another task rewrites the same config (a lock
/// job's last step, the page's next reading healing it) saw half a block and
/// called a sound lock broken; two writers tripped over git's config lock.
async fn repo_turn(repo: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    static TURNS: OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let turn = {
        let mut all = TURNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        all.entry(push_lock::canonical(repo)).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
    };
    turn.lock_owned().await
}

/// Measure, never assume: reads the flag, the rewrite, the hook, every
/// remote's effective push URL, and the lock's admin-owned layers. Under a
/// lock, user-level mirrors that drifted are put back and the state says so.
pub async fn push_state(repo: &Path, hooks: &Path, profile_id: Option<&str>) -> PushState {
    let _turn = repo_turn(repo).await;
    let store = profiles::load_profiles().unwrap_or_else(|_| profiles::ProfileStore::new());
    let profile = profile_id.and_then(|id| store.profiles.iter().find(|p| p.id == id));
    let profile_blocked = profile.map(|p| !p.allow_push).unwrap_or(false);

    let mut repo_blocked = read_flag(repo).await;
    let mut config_block_present = read_rewrite_present(repo).await;
    let hooks_path_overridden = read_hooks_path(repo).await;
    let mut hook = hook_kind(hooks);
    let mut remotes = read_remotes(repo).await;
    let raw_urls = remote_urls(repo).await;

    let facts = |flag: bool, rewrite: bool, hook: &str, remotes: &[RemotePushState]| MirrorFacts {
        flag,
        rewrite,
        hook_ok: hook == "gitswitch" || hook == "chained",
        hooks_redirected: hooks_path_overridden.is_some(),
        explicit_pushurl: remotes.iter().filter(|r| r.has_explicit_pushurl).map(|r| r.name.clone()).collect(),
        uncovered_remotes: remotes.iter().filter(|r| !r.rewritten && !r.has_explicit_pushurl).map(|r| r.name.clone()).collect(),
        remote_urls: raw_urls.clone(),
    };
    // Boxed, here and below: this measurement sits deep inside an Update's
    // Continue (status reads, submodules downloaded under a block), where a
    // debug build's nested poll frames outgrew a 2 MiB thread stack.
    let mut lock = Box::pin(push_lock::measure(repo, &facts(repo_blocked, config_block_present, &hook, &remotes))).await;

    if lock.locked && lock.mirrors == "drifted" && push_lock::heal_allowed(repo) {
        let before = lock.drift.iter().filter(|d| d.starts_with("mirror-")).cloned().collect::<Vec<_>>().join(", ");
        let healed = Box::pin(apply_mirrors_held(repo, hooks)).await.is_ok();
        repo_blocked = read_flag(repo).await;
        config_block_present = read_rewrite_present(repo).await;
        hook = hook_kind(hooks);
        remotes = read_remotes(repo).await;
        let again = Box::pin(push_lock::measure(repo, &facts(repo_blocked, config_block_present, &hook, &remotes))).await;
        let ok = healed && again.mirrors != "drifted";
        lock = again;
        if ok {
            lock.mirrors = "healed".into();
        }
        let repo_c = push_lock::canonical(repo);
        push_lock::log_event(
            &repo_c,
            "mirrors-drifted",
            &format!("{} — {}", before, if ok { "re-applied" } else { "could not be re-applied" }),
            ok,
        );
        lock.recent_events = push_lock::recent_events(&repo_c);
    }

    let blocked = profile_blocked || repo_blocked || lock.locked;
    // A remote added after the block went on, or a hook that was deleted. A
    // redirected hooks dir is reported as a gap, not as something to repair:
    // writing into a shared or tracked hooks folder is not the app's to do.
    let hook_ok = hook == "gitswitch" || hook == "chained" || hooks_path_overridden.is_some();
    let needs_repair = repo_blocked
        && !lock.locked
        && (!config_block_present
            || !hook_ok
            || remotes.iter().any(|r| !r.rewritten && !r.has_explicit_pushurl));

    PushState {
        reason: block_reason(profile_blocked, profile.map(|p| p.name.as_str()), repo_blocked, lock.locked),
        gaps: gaps_for(blocked, lock.locked, &hook, hooks_path_overridden.as_deref(), &remotes),
        profile_blocked,
        profile_name: profile.map(|p| p.name.clone()),
        repo_blocked,
        blocked,
        hook,
        hooks_path_overridden,
        config_block_present,
        remotes,
        needs_repair,
        lock,
    }
}

async fn write_config_block(repo: &Path) -> Result<(), AppError> {
    let urls = remote_urls(repo).await;
    let key = format!("url.{}.pushInsteadOf", BLOCK_SCHEME);
    let want = block_prefixes(&urls);
    let have: Vec<String> = GitCmd::at(repo)
        .args(["config", "--local", "--get-all", &key])
        .ok_text()
        .await
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default();
    // What is missing goes in first, and nothing that stays is ever taken
    // out: the block is never half gone — for a push from a terminal, or for
    // this app's next measurement — and applying it again writes nothing.
    for prefix in want.iter().filter(|p| !have.contains(p)) {
        GitCmd::at(repo)
            .args(["config", "--local", "--add", &key, prefix])
            .text()
            .await?;
    }
    // Then what no remote needs any more, and copies an earlier apply left.
    // Both only ever block more, so a failure here is not the block's.
    let mut seen = std::collections::HashSet::new();
    for h in &have {
        if !want.contains(h) {
            let _ = GitCmd::at(repo).args(["config", "--local", "--unset-all", &key, &value_pattern(h)]).run().await;
        } else if !seen.insert(h.as_str()) {
            // One write that leaves exactly one copy.
            let _ = GitCmd::at(repo).args(["config", "--local", "--replace-all", &key, h, &value_pattern(h)]).run().await;
        }
    }
    Ok(())
}

/// A git config value pattern matching `value` exactly: git reads it as an
/// extended regular expression, and these prefixes are full of dots.
fn value_pattern(value: &str) -> String {
    let mut s = String::from("^");
    for c in value.chars() {
        if "\\.[]()*+?{}|^$".contains(c) {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('$');
    s
}

async fn remove_config_block(repo: &Path) -> Result<(), AppError> {
    let key = format!("url.{}.pushInsteadOf", BLOCK_SCHEME);
    let _ = GitCmd::at(repo)
        .args(["config", "--local", "--unset-all", &key])
        .run()
        .await;
    // Leaving an empty [url "…"] section behind would be untidy but harmless;
    // remove it when git will let us.
    let _ = GitCmd::at(repo)
        .args([
            "config",
            "--local",
            "--remove-section",
            &format!("url.{}", BLOCK_SCHEME),
        ])
        .run()
        .await;
    Ok(())
}

fn install_hook(hooks: &Path) -> Result<bool, AppError> {
    std::fs::create_dir_all(hooks)?;
    let hook = hooks.join("pre-push");
    let mut chained = false;
    if hook.exists() {
        let existing = std::fs::read_to_string(&hook).unwrap_or_default();
        if !existing.contains(HOOK_MARKER) {
            // argos's pre-push is Git LFS's. Replacing it outright would break
            // LFS uploads, so it is preserved and called.
            std::fs::rename(&hook, hooks.join("pre-push.gitswitch-saved"))?;
            chained = true;
        } else {
            chained = hooks.join("pre-push.gitswitch-saved").exists();
        }
    }
    // Written only when it differs, and whole: a push that runs the hook while
    // it is being written must not find it empty.
    let body = hook_body(chained);
    if std::fs::read_to_string(&hook).ok().as_deref() != Some(body.as_str()) {
        let tmp = hooks.join("pre-push.gitswitch-new");
        std::fs::write(&tmp, &body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &hook)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&hook).map(|m| m.permissions().mode() & 0o111 == 0).unwrap_or(false) {
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(chained)
}

fn remove_hook(hooks: &Path) -> Result<(), AppError> {
    let hook = hooks.join("pre-push");
    if !hook.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&hook).unwrap_or_default();
    if !content.contains(HOOK_MARKER) {
        // Someone else's hook: leave it exactly as it is.
        return Ok(());
    }
    std::fs::remove_file(&hook)?;
    let saved = hooks.join("pre-push.gitswitch-saved");
    if saved.exists() {
        std::fs::rename(&saved, &hook)?;
    }
    Ok(())
}

/// The user-level pieces: flag, local rewrite, and the hook unless
/// `core.hooksPath` sends hooks elsewhere (writing there could drop a
/// GitSwitch file into a tracked folder — husky — or a shared hooks dir).
pub async fn apply_mirrors(repo: &Path, hooks: &Path) -> Result<(), AppError> {
    let _turn = repo_turn(repo).await;
    Box::pin(apply_mirrors_held(repo, hooks)).await
}

/// `apply_mirrors` for a caller that already has the repository's turn.
async fn apply_mirrors_held(repo: &Path, hooks: &Path) -> Result<(), AppError> {
    // Each piece is written only when it is missing: re-applying a complete
    // block leaves the config file alone.
    if !read_flag(repo).await {
        GitCmd::at(repo)
            .args(["config", "--local", "--bool", FLAG, "true"])
            .text()
            .await?;
    }
    Box::pin(write_config_block(repo)).await?;
    if read_hooks_path(repo).await.is_none() {
        install_hook(hooks)?;
    }
    Ok(())
}

/// Undo `apply_mirrors`, and put back any pushurl the lock stored.
pub async fn remove_mirrors(repo: &Path, hooks: &Path) -> Result<(), AppError> {
    let _turn = repo_turn(repo).await;
    let _ = GitCmd::at(repo)
        .args(["config", "--local", "--unset", FLAG])
        .run()
        .await;
    remove_config_block(repo).await?;
    remove_hook(hooks)?;
    push_lock::restore_pushurls(repo).await;
    Ok(())
}

/// Turn the repo-level guard-rail on or off, then re-measure and return the
/// proof. Refuses to turn it off while the repository is locked: those files
/// belong to the lock until an administrator removes it.
pub async fn set_repo_blocked(
    repo_path: &str,
    hooks: &Path,
    blocked: bool,
    profile_id: Option<&str>,
) -> Result<PushState, AppError> {
    let repo = PathBuf::from(repo_path);
    if blocked {
        apply_mirrors(&repo, hooks).await?;
        // The submodules too: the code lives there, and a push can start there.
        cover_submodules(repo_path, None).await;
    } else {
        let current = push_state(&repo, hooks, profile_id).await;
        if current.lock.locked {
            return Err(AppError::Config(
                "This repository is locked. Unlock it first — that needs the administrator password.".into(),
            ));
        }
        remove_mirrors(&repo, hooks).await?;
        uncover_submodules(repo_path).await;
    }
    Ok(push_state(&repo, hooks, profile_id).await)
}

/// Re-apply whatever is missing — the fix for `needs_repair`, and for a
/// locked repository the mirrors — in the repository and its submodules.
pub async fn repair(
    repo_path: &str,
    hooks: &Path,
    profile_id: Option<&str>,
) -> Result<PushState, AppError> {
    Ok(repair_noting(repo_path, hooks, profile_id).await?.0)
}

/// `repair`, and what could not be written in the submodules, one line each.
pub async fn repair_noting(
    repo_path: &str,
    hooks: &Path,
    profile_id: Option<&str>,
) -> Result<(PushState, Vec<String>), AppError> {
    let repo = PathBuf::from(repo_path);
    let state = Box::pin(push_state(&repo, hooks, profile_id)).await;
    if !state.repo_blocked && !state.lock.locked {
        return Ok((state, Vec::new()));
    }
    Box::pin(apply_mirrors(&repo, hooks)).await?;
    let notes = Box::pin(cover_submodules(repo_path, None)).await;
    Ok((Box::pin(push_state(&repo, hooks, profile_id)).await, notes))
}

/// Written into a submodule the repository's block or lock was extended to,
/// so that only the repository's "allowed" takes it away again — a block the
/// person put on the submodule itself stays. The value names no path: a
/// submodule has exactly one superproject, and that may be moved.
const BY: &str = "gitswitch.pushBlockedBy";
const BY_VALUE: &str = "superproject";

pub(crate) async fn marked(dir: &Path) -> bool {
    GitCmd::at(dir).args(["config", "--local", "--get", BY]).ok_text().await.map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Every populated submodule of the repository, nested ones included, as
/// paths relative to it ("a/b" for one inside another).
pub async fn populated_submodules(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue: Vec<(PathBuf, String)> = vec![(repo.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = queue.pop() {
        let Ok(list) = crate::submodules::list_submodules(&dir.to_string_lossy()).await else { continue };
        for s in list.into_iter().filter(|s| s.initialised && s.gitdir_valid) {
            let rel = if prefix.is_empty() { s.path.clone() } else { format!("{}/{}", prefix, s.path) };
            queue.push((dir.join(&s.path), rel.clone()));
            out.push(rel);
        }
    }
    out.sort();
    out
}

/// One submodule's share of the repository's push block or lock.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubCoverage {
    pub path: String,
    pub blocked: bool,
    pub locked: bool,
    pub needs_repair: bool,
    /// It has what the repository has: a lock under a locked repository, at
    /// least the guard-rail under a blocked one, with nothing drifted.
    pub covered: bool,
    /// When not covered: what it has instead, in the person's words.
    pub why: String,
    /// Its block is the repository's doing (marked), not the person's own:
    /// the repository's "Allowed" takes it away — with the administrator
    /// prompt when it is a lock.
    #[serde(default)]
    pub by_superproject: bool,
    /// When its lock is not whole: what was measured, in the lock's own
    /// terms (`stanza-stale`, `mirror-rewrite`…), for whoever has to look.
    #[serde(default)]
    pub drift: Vec<String>,
}

/// How far the repository's push block or lock reaches into its submodules —
/// the repositories the code actually lives in.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PushCoverage {
    /// allowed | guardrail | locked — the repository's own mode.
    pub mode: String,
    pub submodules: Vec<SubCoverage>,
    /// Submodules with less than the repository has, or drifted: a push from
    /// inside them is not stopped.
    pub uncovered: Vec<String>,
    /// Repair can do something about what is uncovered.
    pub needs_repair: bool,
    /// ...and part of that is the administrator's: Repair will show the prompt.
    pub needs_elevation: bool,
    /// One sentence for the card; empty when there is nothing to say.
    pub summary: String,
}

/// What Repair can do for one uncovered submodule.
#[derive(PartialEq)]
enum Fix {
    /// Nothing: a remote with its own push address, hooks sent elsewhere, a
    /// path the lock cannot carry — said, not offered.
    Nothing,
    /// Its own settings, no password.
    Plain,
    /// The lock's administrator-owned part: the prompt.
    Admin,
}

pub fn mode_of(state: &PushState) -> &'static str {
    if state.lock.locked {
        "locked"
    } else if state.repo_blocked {
        "guardrail"
    } else {
        "allowed"
    }
}

fn some_submodules(n: usize) -> String {
    format!("{} submodule{}", n, if n == 1 { "" } else { "s" })
}

/// Measure every populated submodule against the repository's own mode —
/// four at a time, since each is several git calls.
pub async fn coverage(repo_path: &str, state: &PushState) -> PushCoverage {
    let repo = PathBuf::from(repo_path);
    let mode = mode_of(state);
    let rels = populated_submodules(&repo).await;
    let mut measured: Vec<(String, PushState, bool)> = Vec::new();
    for chunk in rels.chunks(4) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|rel| {
                let dir = repo.join(rel);
                let rel = rel.clone();
                tokio::spawn(async move {
                    let paths = crate::git_status::git_paths(&dir).await.ok()?;
                    let profile = crate::git_status::owning_profile_id(&dir.to_string_lossy());
                    let by = marked(&dir).await;
                    Some((rel, push_state(&dir, &paths.hooks, profile.as_deref()).await, by))
                })
            })
            .collect();
        for h in handles {
            if let Ok(Some(m)) = h.await {
                measured.push(m);
            }
        }
    }
    // A submodule measured while something else was still writing its pieces
    // could read as broken for that moment: one that measures as needing
    // repair is measured once more, on its own, before the card says so.
    let shaky = |s: &PushState| s.lock.locked && (s.lock.needs_elevation || s.lock.mirrors == "drifted");
    for (rel, s, _) in measured.iter_mut() {
        if !shaky(s) {
            continue;
        }
        let dir = repo.join(&*rel);
        if let Ok(paths) = crate::git_status::git_paths(&dir).await {
            let profile = crate::git_status::owning_profile_id(&dir.to_string_lossy());
            *s = Box::pin(push_state(&dir, &paths.hooks, profile.as_deref())).await;
        }
    }
    let mut subs = Vec::new();
    let mut uncovered: Vec<(String, String)> = Vec::new();
    let mut fixes: Vec<Fix> = Vec::new();
    for (rel, s, by_superproject) in measured {
        // What the person wants to hear: whether a push from inside is really
        // stopped, and by what — and whether Repair needs the password for it.
        let nothing_stops = s.gaps.iter().any(|g| g.contains("Nothing local stops a push"));
        let (why, fix) = match mode {
            "locked" => {
                if s.lock.locked && s.lock.needs_elevation {
                    ("its lock needs repair".to_string(), Fix::Admin)
                } else if s.lock.locked && s.lock.mirrors == "drifted" {
                    ("it is locked, but its own settings lost part of the block; Repair puts it back without a password".to_string(), Fix::Plain)
                } else if s.lock.locked {
                    (String::new(), Fix::Nothing)
                } else if lc::validate_path(&push_lock::canonical(&repo.join(&rel))).is_err() {
                    // Said by what was measured: the guard-rail only when it is there.
                    if s.repo_blocked && !nothing_stops {
                        ("its path holds a character the lock cannot carry; the guard-rail stops a push there, anyone with a terminal can turn that off".to_string(), Fix::Nothing)
                    } else {
                        ("its path holds a character the lock cannot carry, and nothing stops a push from inside it".to_string(), Fix::Plain)
                    }
                } else if s.repo_blocked && !nothing_stops {
                    ("has the guard-rail, not the lock — anyone with a terminal can turn it off; Repair adds the lock".to_string(), Fix::Admin)
                } else {
                    ("nothing stops a push from inside it".to_string(), Fix::Admin)
                }
            }
            "guardrail" => {
                if (s.repo_blocked || s.lock.locked) && !s.needs_repair && !nothing_stops {
                    (String::new(), Fix::Nothing)
                } else if s.repo_blocked && nothing_stops {
                    ("its guard-rail is turned aside there (a remote with its own push address, hooks sent elsewhere): nothing stops a push from inside it".to_string(), Fix::Nothing)
                } else if s.repo_blocked {
                    ("its guard-rail has drifted; Repair re-applies it".to_string(), Fix::Plain)
                } else if s.config_block_present && !s.remotes.iter().any(|r| !r.rewritten) {
                    ("only its address rewrite still stops a push; Repair re-applies the rest".to_string(), Fix::Plain)
                } else {
                    ("nothing stops a push from inside it".to_string(), Fix::Plain)
                }
            }
            _ => (String::new(), Fix::Nothing),
        };
        let covered = why.is_empty();
        let drift = if covered { Vec::new() } else { s.lock.drift.clone() };
        if !covered {
            uncovered.push((rel.clone(), why.clone()));
            fixes.push(fix);
        }
        subs.push(SubCoverage { path: rel, blocked: s.blocked, locked: s.lock.locked, needs_repair: s.needs_repair, covered, why, by_superproject, drift });
    }
    let summary = if subs.is_empty() {
        String::new()
    } else if mode == "allowed" {
        let own: Vec<&str> = subs.iter().filter(|s| s.blocked).map(|s| s.path.as_str()).collect();
        if own.is_empty() { String::new() } else { format!("Blocked on their own: {}.", own.join(", ")) }
    } else if uncovered.is_empty() {
        format!("Covers this repository and {}.", some_submodules(subs.len()))
    } else {
        format!(
            "Not covering {} of {}: {}.",
            uncovered.len(),
            some_submodules(subs.len()),
            uncovered.iter().map(|(p, w)| format!("{} ({})", p, w)).collect::<Vec<_>>().join("; ")
        )
    };
    PushCoverage {
        mode: mode.into(),
        needs_repair: fixes.iter().any(|f| *f != Fix::Nothing),
        needs_elevation: fixes.contains(&Fix::Admin),
        submodules: subs,
        uncovered: uncovered.into_iter().map(|(p, _)| p).collect(),
        summary,
    }
}

/// Give the populated submodules (or the named ones) the user-level pieces the
/// repository has — the guard-rail everywhere a push could start. The lock's
/// administrator-owned layer for a submodule is a job of its own (`push_lock`);
/// until it runs, the guard-rail holds in the terminal too. Failures are
/// returned, one line each; `coverage` reports what is still missing.
pub async fn cover_submodules(repo_path: &str, only: Option<&[String]>) -> Vec<String> {
    let mut notes = Vec::new();
    let repo = PathBuf::from(repo_path);
    for rel in populated_submodules(&repo).await {
        // The named ones and the submodules inside them (a download is recursive).
        if let Some(o) = only {
            if !o.iter().any(|p| p == &rel || rel.starts_with(&format!("{}/", p))) {
                continue;
            }
        }
        let dir = repo.join(&rel);
        let paths = match crate::git_status::git_paths(&dir).await {
            Ok(p) => p,
            Err(e) => {
                notes.push(format!("{}: {}", rel, e));
                continue;
            }
        };
        // Blocked on its own already, by the person: the block stays theirs,
        // so "allowed" on the repository will not take it away.
        let own = read_flag(&dir).await && !marked(&dir).await;
        if let Err(e) = Box::pin(apply_mirrors(&dir, &paths.hooks)).await {
            notes.push(format!("{}: {}", rel, e));
            continue;
        }
        // Marked once: writing the same mark again would rewrite its config
        // for nothing, under whatever else is reading it.
        if !own && !marked(&dir).await {
            let _ = GitCmd::at(&dir).args(["config", "--local", BY, BY_VALUE]).run().await;
        }
    }
    notes
}

/// Take the user-level pieces away from every populated submodule the
/// repository's block was extended to. One blocked on its own, or locked, keeps them.
pub async fn uncover_submodules(repo_path: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let repo = PathBuf::from(repo_path);
    for rel in populated_submodules(&repo).await {
        let dir = repo.join(&rel);
        let Ok(paths) = crate::git_status::git_paths(&dir).await else { continue };
        if !marked(&dir).await {
            continue;
        }
        let profile = crate::git_status::owning_profile_id(&dir.to_string_lossy());
        if Box::pin(push_state(&dir, &paths.hooks, profile.as_deref())).await.lock.locked {
            notes.push(format!("{} stays locked on its own", rel));
            continue;
        }
        if let Err(e) = Box::pin(remove_mirrors(&dir, &paths.hooks)).await {
            notes.push(format!("{}: {}", rel, e));
            continue;
        }
        let _ = GitCmd::at(&dir).args(["config", "--local", "--unset", BY]).run().await;
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_pattern_matches_exactly_that_prefix() {
        assert_eq!(value_pattern("org-1@github.com:Org/x.git"), "^org-1@github\\.com:Org/x\\.git$");
        assert_eq!(value_pattern("a+b(c)[d]{e}|f^$\\"), "^a\\+b\\(c\\)\\[d\\]\\{e\\}\\|f\\^\\$\\\\$");
        assert_eq!(value_pattern("ssh://git@github.com:22/"), "^ssh://git@github\\.com:22/$");
    }

    #[test]
    fn prefixes_are_derived_from_every_url_shape() {
        assert_eq!(
            derive_push_prefix("git@github.com:Org/repo.git").as_deref(),
            Some("git@github.com:")
        );
        assert_eq!(
            derive_push_prefix("org-1234567@github.com:AcmeOrg/widgets.git").as_deref(),
            Some("org-1234567@github.com:")
        );
        assert_eq!(
            derive_push_prefix("ssh://git@github.com/o/r.git").as_deref(),
            Some("ssh://git@github.com/")
        );
        assert_eq!(
            derive_push_prefix("https://gitlab.example.com/o/r.git").as_deref(),
            Some("https://gitlab.example.com/")
        );
        // The hole the profile-level block cannot see.
        assert_eq!(derive_push_prefix("gh-work:o/r.git").as_deref(), Some("gh-work:"));
    }

    #[test]
    fn local_paths_never_produce_a_prefix() {
        for url in ["/srv/git/repo.git", "./repo", "~/repo", "C:/repos/x", ""] {
            assert!(
                derive_push_prefix(url).is_none(),
                "{} should not yield a prefix",
                url
            );
        }
    }

    #[test]
    fn block_prefixes_cover_github_and_dedupe_the_repo_remote() {
        let urls = vec![
            "git@github.com:Org/repo.git".to_string(),
            "gh-work:Org/other.git".to_string(),
        ];
        let prefixes = block_prefixes(&urls);
        assert!(prefixes.iter().any(|p| p == "git@github.com:"));
        assert!(prefixes.iter().any(|p| p == "https://github.com/"));
        assert!(prefixes.iter().any(|p| p == "gh-work:"));
        // git@github.com: is in the base list already and must appear once.
        assert_eq!(prefixes.iter().filter(|p| *p == "git@github.com:").count(), 1);
        // Each remote's exact URL is pinned too, so a form we don't generalise
        // is still covered by the rewrite rather than by the hook alone.
        assert!(prefixes.iter().any(|p| p == "git@github.com:Org/repo.git"));
        let local = block_prefixes(&["/srv/git/repo.git".to_string()]);
        assert!(local.iter().any(|p| p == "/srv/git/repo.git"));
        // Nothing here may ever be a plain insteadOf — that would break fetch.
        assert!(prefixes.iter().all(|p| !p.contains('\\')));
        // Every prefix passes the helper's validation, so a lock job built
        // from them is never refused for its own remotes.
        for p in &prefixes {
            assert!(lc::validate_prefix(p).is_ok(), "{}", p);
        }
    }

    #[test]
    fn hook_reads_both_flags_and_never_consumes_stdin_before_chaining() {
        let body = hook_body(true);
        assert!(body.starts_with("#!/bin/sh"));
        assert!(body.contains(HOOK_MARKER));
        // Toggling must be a config write, not a hook rewrite.
        assert!(body.contains("git config --bool --get gitswitch.pushBlocked"));
        assert!(body.contains("git config --bool --get gitswitch.locked"));
        assert!(body.contains("pre-push.gitswitch-saved"));
        // git feeds ref updates on stdin and the chained hook (LFS) needs them.
        let before_chain = body.split("pre-push.gitswitch-saved").next().unwrap();
        assert!(!before_chain.contains("read "));
        assert!(!before_chain.contains("cat "));
        assert!(!before_chain.contains("xargs"));
    }

    #[test]
    fn hook_never_prints_the_command_that_disables_it() {
        let body = hook_body(false);
        // Only the two reads may mention git config; nothing printed may.
        for line in body.lines().filter(|l| l.trim_start().starts_with("echo")) {
            assert!(!line.contains("git config"), "{}", line);
            assert!(!line.contains("--unset"), "{}", line);
        }
        assert!(body.contains("Do not work around this"));
        assert!(body.contains("gitswitch: lock=on policy=ask-owner"));
        assert!(body.contains("guard-rail"));
    }

    #[test]
    fn an_unchained_hook_has_no_dangling_chain_call() {
        let body = hook_body(false);
        assert!(!body.contains("gitswitch-saved"));
        assert!(body.contains("exit 1"));
    }

    #[test]
    fn ui_treats_profile_block_as_binding() {
        // The app never writes a longer rewrite to re-allow a push. git itself
        // would honour one — longest match wins in any scope — which is why
        // this is a UI rule, not a property of git.
        let reason = block_reason(true, Some("Work"), false, false);
        assert!(reason.contains("Work"));
        assert!(block_reason(false, None, true, false).contains("this repository"));
        assert!(block_reason(true, Some("Work"), true, false).contains("won't be enough"));
        assert!(block_reason(false, None, false, false).contains("allowed"));
        let locked = block_reason(true, Some("Work"), false, true);
        assert!(locked.starts_with("Locked for this repository."));
        assert!(locked.contains("administrator password"));
        assert!(locked.contains("Work"));
    }

    fn remote(name: &str, rewritten: bool, explicit: bool) -> RemotePushState {
        RemotePushState {
            name: name.into(),
            fetch_url: "git@github.com:o/r.git".into(),
            push_url: if rewritten {
                format!("{}git@github.com:o/r.git", BLOCK_SCHEME)
            } else {
                "git@github.com:o/r.git".into()
            },
            rewritten,
            has_explicit_pushurl: explicit,
        }
    }

    #[test]
    fn gaps_name_what_is_actually_uncovered() {
        // Fully covered: still discloses --no-verify and the guard-rail caveat.
        let gaps = gaps_for(true, false, "gitswitch", None, &[remote("origin", true, false)]);
        assert!(gaps.iter().any(|g| g.contains("--no-verify")));
        assert!(gaps.iter().any(|g| g.contains("guard-rail, not a lock")));
        assert!(gaps.iter().any(|g| g.contains("GIT_CONFIG_NOSYSTEM")));
        assert!(!gaps.iter().any(|g| g.contains("isn't stopped by the URL rewrite")));

        // A pushurl remote is the case the rewrite provably misses.
        let gaps = gaps_for(true, false, "gitswitch", None, &[remote("origin", false, true)]);
        assert!(gaps
            .iter()
            .any(|g| g.contains("origin") && g.contains("its own pushurl")));

        // hooksPath redirected: say the hook isn't the one git runs.
        let gaps = gaps_for(true, false, "none", Some(".husky"), &[remote("origin", true, false)]);
        assert!(gaps.iter().any(|g| g.contains("core.hooksPath")));

        // Under the lock the guard-rail sentence gives way to the lock's caveats.
        let gaps = gaps_for(true, true, "gitswitch", None, &[remote("origin", true, false)]);
        assert!(!gaps.iter().any(|g| g.contains("guard-rail, not a lock")));

        // Not blocked: nothing to disclose.
        assert!(gaps_for(false, false, "none", None, &[remote("origin", false, false)]).is_empty());
    }
}
