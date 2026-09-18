//! Blocking pushes for a single repository, in a way that also holds in the
//! user's terminal.
//!
//! Two mechanisms, because each has a blind spot the other covers:
//!
//! 1. A `pushInsteadOf` rewrite in the repo's own `.git/config`. Transport
//!    level, so `--no-verify` cannot get past it — but it misses a remote with
//!    an explicit `remote.<name>.pushurl`.
//! 2. A `pre-push` hook. Catches every remote and every URL form — but it is
//!    skipped by `--no-verify` and by `core.hooksPath` pointing elsewhere.
//!
//! Both read the same flag (`gitswitch.pushBlocked`), so toggling never rewrites
//! the hook, and an installed hook is inert while the flag is off. Whatever is
//! left uncovered is reported in `gaps` rather than papered over: this is a
//! guard-rail, not a lock, and the UI says so.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use crate::profiles;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The URL scheme pushes get rewritten to. git has no such remote helper, so
/// the push fails before anything leaves the machine.
pub const BLOCK_SCHEME: &str = "gitswitch-push-blocked://";
const FLAG: &str = "gitswitch.pushBlocked";
const HOOK_MARKER: &str = ">>> GitSwitch push guard >>>";

/// GitHub in every form git accepts, so a rewrite covers the common cases even
/// before per-remote prefixes are added. Mirrors `git_config::push_block_section`.
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

#[derive(Debug, Serialize, Clone)]
pub struct RemotePushState {
    pub name: String,
    pub fetch_url: String,
    /// The push URL *after* any rewrite — i.e. what git would really use.
    pub push_url: String,
    pub rewritten: bool,
    pub has_explicit_pushurl: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct PushState {
    pub profile_blocked: bool,
    pub profile_name: Option<String>,
    pub repo_blocked: bool,
    /// The effective answer. More restrictive wins: a repo cannot re-allow what
    /// a profile blocks, because URL rewrites accumulate across config files
    /// and nothing at repo level can remove one contributed by an include.
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

/// Reads the flag rather than hard-coding "blocked", so turning the block on and
/// off is one `git config` write and never a hook rewrite. Deliberately does not
/// read stdin: git feeds ref updates there and the chained hook (Git LFS, in
/// argos) needs them untouched.
const PUSH_HOOK_TEMPLATE: &str = r#"#!/bin/sh
# >>> GitSwitch push guard >>>
# Blocks pushes from this repository while GitSwitch has push access turned off.
# Managed by GitSwitch — turn it back on in the app (Changes -> Push access).
if [ "$(git config --bool --get gitswitch.pushBlocked)" = "true" ]; then
  echo "GitSwitch: push blocked for this repository." >&2
  echo "  remote: $1" >&2
  echo "  Turn push access back on in GitSwitch, or run:" >&2
  echo "    git config --local --unset gitswitch.pushBlocked" >&2
  exit 1
fi
# <<< GitSwitch push guard <<<
__CHAIN__
"#;

const CHAIN_SNIPPET: &str = "# run the hook that was here before GitSwitch\nif [ -x \"$(dirname \"$0\")/pre-push.gitswitch-saved\" ]; then\n  \"$(dirname \"$0\")/pre-push.gitswitch-saved\" \"$@\" || exit $?\nfi";

pub fn hook_body(chain: bool) -> String {
    PUSH_HOOK_TEMPLATE.replace("__CHAIN__", if chain { CHAIN_SNIPPET } else { "" })
}

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
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn read_remotes(repo: &Path) -> Vec<RemotePushState> {
    let mut out = Vec::new();
    for name in remote_names(repo).await {
        if crate::git_exec::validate_remote_name(&name).is_err() {
            continue;
        }
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
    gaps.push(
        "This is a guard-rail, not a lock: anyone with a terminal can turn it off with one git command. For enforcement nobody can bypass, use branch protection on GitHub."
            .to_string(),
    );
    gaps
}

/// One sentence naming who is blocking, so the UI never has to infer it.
pub fn block_reason(profile_blocked: bool, profile_name: Option<&str>, repo_blocked: bool) -> String {
    match (profile_blocked, repo_blocked) {
        (false, false) => "Pushing is allowed from this folder.".into(),
        (true, false) => format!(
            "Blocked by profile '{}'. Allow pushes for that profile, or move this folder.",
            profile_name.unwrap_or("?")
        ),
        (false, true) => "Blocked for this repository.".into(),
        (true, true) => format!(
            "Blocked for this repository, and by profile '{}' — unblocking the repo alone won't be enough.",
            profile_name.unwrap_or("?")
        ),
    }
}

/// Measure, never assume: reads the flag, the rewrite, the hook and every
/// remote's effective push URL.
pub async fn push_state(repo: &Path, hooks: &Path, profile_id: Option<&str>) -> PushState {
    let store = profiles::load_profiles().unwrap_or_else(|_| profiles::ProfileStore::new());
    let profile = profile_id.and_then(|id| store.profiles.iter().find(|p| p.id == id));
    let profile_blocked = profile.map(|p| !p.allow_push).unwrap_or(false);

    let repo_blocked = GitCmd::at(repo)
        .args(["config", "--local", "--bool", "--get", FLAG])
        .ok_text()
        .await
        .map(|v| v == "true")
        .unwrap_or(false);

    let config_block_present = GitCmd::at(repo)
        .args([
            "config",
            "--local",
            "--get-all",
            &format!("url.{}.pushInsteadOf", BLOCK_SCHEME),
        ])
        .ok_text()
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    let hooks_path_overridden = GitCmd::at(repo)
        .args(["config", "--get", "core.hooksPath"])
        .ok_text()
        .await
        .filter(|s| !s.is_empty());

    let hook = hook_kind(hooks);
    let remotes = read_remotes(repo).await;
    let blocked = profile_blocked || repo_blocked;

    // A remote added after the block went on, or a hook that was deleted.
    let hook_ok = hook == "gitswitch" || hook == "chained" || hooks_path_overridden.is_some();
    let needs_repair = repo_blocked
        && (!config_block_present
            || !hook_ok
            || remotes
                .iter()
                .any(|r| !r.rewritten && !r.has_explicit_pushurl));

    PushState {
        reason: block_reason(profile_blocked, profile.map(|p| p.name.as_str()), repo_blocked),
        gaps: gaps_for(blocked, &hook, hooks_path_overridden.as_deref(), &remotes),
        profile_blocked,
        profile_name: profile.map(|p| p.name.clone()),
        repo_blocked,
        blocked,
        hook,
        hooks_path_overridden,
        config_block_present,
        remotes,
        needs_repair,
    }
}

async fn write_config_block(repo: &Path) -> Result<(), AppError> {
    let urls: Vec<String> = {
        let mut v = Vec::new();
        for name in remote_names(repo).await {
            if let Some(u) = GitCmd::at(repo)
                .args(["remote", "get-url", &name])
                .ok_text()
                .await
            {
                v.push(u);
            }
        }
        v
    };
    let key = format!("url.{}.pushInsteadOf", BLOCK_SCHEME);
    // Start clean so repeated applies can't pile up duplicates.
    let _ = GitCmd::at(repo)
        .args(["config", "--local", "--unset-all", &key])
        .run()
        .await;
    for prefix in block_prefixes(&urls) {
        GitCmd::at(repo)
            .args(["config", "--local", "--add", &key, &prefix])
            .text()
            .await?;
    }
    Ok(())
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
    std::fs::write(&hook, hook_body(chained))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
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

/// Turn the repo-level block on or off, then re-measure and return the proof.
pub async fn set_repo_blocked(
    repo_path: &str,
    hooks: &Path,
    blocked: bool,
    profile_id: Option<&str>,
) -> Result<PushState, AppError> {
    let repo = PathBuf::from(repo_path);
    if blocked {
        GitCmd::at(&repo)
            .args(["config", "--local", "--bool", FLAG, "true"])
            .text()
            .await?;
        write_config_block(&repo).await?;
        // A hooksPath override means git runs hooks from somewhere else; writing
        // there could drop a GitSwitch file into a tracked folder (husky), so
        // the gap is reported instead.
        let overridden = GitCmd::at(&repo)
            .args(["config", "--get", "core.hooksPath"])
            .ok_text()
            .await
            .filter(|s| !s.is_empty());
        if overridden.is_none() {
            install_hook(hooks)?;
        }
    } else {
        let _ = GitCmd::at(&repo)
            .args(["config", "--local", "--unset", FLAG])
            .run()
            .await;
        remove_config_block(&repo).await?;
        remove_hook(hooks)?;
    }
    Ok(push_state(&repo, hooks, profile_id).await)
}

/// Re-apply whatever is missing — the fix for `needs_repair`.
pub async fn repair(
    repo_path: &str,
    hooks: &Path,
    profile_id: Option<&str>,
) -> Result<PushState, AppError> {
    let repo = PathBuf::from(repo_path);
    let state = push_state(&repo, hooks, profile_id).await;
    if !state.repo_blocked {
        return Ok(state);
    }
    set_repo_blocked(repo_path, hooks, true, profile_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_are_derived_from_every_url_shape() {
        assert_eq!(
            derive_push_prefix("git@github.com:Org/repo.git").as_deref(),
            Some("git@github.com:")
        );
        assert_eq!(
            derive_push_prefix("org-302118749@github.com:EtharaOrion/argos.git").as_deref(),
            Some("org-302118749@github.com:")
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
    }

    #[test]
    fn hook_reads_the_flag_and_never_consumes_stdin_before_chaining() {
        let body = hook_body(true);
        assert!(body.starts_with("#!/bin/sh"));
        assert!(body.contains(HOOK_MARKER));
        // Toggling must be a config write, not a hook rewrite.
        assert!(body.contains("git config --bool --get gitswitch.pushBlocked"));
        assert!(body.contains("pre-push.gitswitch-saved"));
        // git feeds ref updates on stdin and the chained hook (LFS) needs them.
        let before_chain = body.split("pre-push.gitswitch-saved").next().unwrap();
        assert!(!before_chain.contains("read "));
        assert!(!before_chain.contains("cat "));
        assert!(!before_chain.contains("xargs"));
    }

    #[test]
    fn an_unchained_hook_has_no_dangling_chain_call() {
        let body = hook_body(false);
        assert!(!body.contains("gitswitch-saved"));
        assert!(body.contains("exit 1"));
    }

    #[test]
    fn a_repo_cannot_re_allow_what_a_profile_blocks() {
        // URL rewrites accumulate across config files; nothing at repo level can
        // remove one contributed by a profile include.
        let reason = block_reason(true, Some("Work"), false);
        assert!(reason.contains("Work"));
        assert!(block_reason(false, None, true).contains("this repository"));
        assert!(block_reason(true, Some("Work"), true).contains("won't be enough"));
        assert!(block_reason(false, None, false).contains("allowed"));
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
        let gaps = gaps_for(true, "gitswitch", None, &[remote("origin", true, false)]);
        assert!(gaps.iter().any(|g| g.contains("--no-verify")));
        assert!(gaps.iter().any(|g| g.contains("guard-rail, not a lock")));
        assert!(!gaps.iter().any(|g| g.contains("isn't stopped by the URL rewrite")));

        // A pushurl remote is the case the rewrite provably misses.
        let gaps = gaps_for(true, "gitswitch", None, &[remote("origin", false, true)]);
        assert!(gaps
            .iter()
            .any(|g| g.contains("origin") && g.contains("its own pushurl")));

        // hooksPath redirected: say the hook isn't the one git runs.
        let gaps = gaps_for(true, "none", Some(".husky"), &[remote("origin", true, false)]);
        assert!(gaps.iter().any(|g| g.contains("core.hooksPath")));

        // Not blocked: nothing to disclose.
        assert!(gaps_for(false, "none", None, &[remote("origin", false, false)]).is_empty());
    }
}
