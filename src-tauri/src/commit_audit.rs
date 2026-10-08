use crate::error::AppError;
use crate::profiles;
use crate::repo_scan;
use serde::Serialize;
use std::path::{Path, PathBuf};

const MARKER: &str = "GitSwitch identity guard";
const MAX_COMMITS_PER_REPO: usize = 60;

#[derive(Debug, Serialize, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub subject: String,
    /// Already on the remote — rewriting it would need a force-push.
    pub pushed: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoAudit {
    pub path: String,
    pub repo_name: String,
    pub profile_name: String,
    pub expected_name: String,
    pub expected_email: String,
    /// Commits that are ours but carry the wrong identity.
    pub mismatched: Vec<CommitInfo>,
    pub unpushed_count: usize,
    pub pushed_count: usize,
    pub has_upstream: bool,
    pub dirty: bool,
    /// "none" | "gitswitch" | "foreign"
    pub guard: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let started = std::time::Instant::now();
    // Signature checks must never land in output that is parsed.
    let safe: Vec<&str> = match args.first().and_then(|sub| crate::git_exec::parse_safe_cfg(sub)) {
        Some(kv) => vec!["-c", kv],
        None => Vec::new(),
    };
    let out = crate::spawn::command("git")
        .arg("-C")
        .arg(repo)
        .args(&safe)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;
    crate::cmdlog::record(
        repo,
        ["git", "-C", &repo.to_string_lossy()]
            .iter()
            .map(|a| a.to_string())
            .chain(safe.iter().map(|a| a.to_string()))
            .chain(args.iter().map(|a| a.to_string()))
            .collect(),
        out.status.code().unwrap_or(-1),
        started.elapsed(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    );
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The profile that owns a repo path — longest matching directory wins, which
/// mirrors how the generated gitconfig resolves overlapping folders.
pub(crate) fn owning_profile<'a>(
    repo: &str,
    profs: &'a [profiles::Profile],
) -> Option<&'a profiles::Profile> {
    let mut best: Option<&profiles::Profile> = None;
    let mut best_len = 0usize;
    for p in profs {
        for d in &p.directories {
            let dir = crate::paths::norm(d);
            if crate::paths::is_within(repo, &dir) && dir.len() >= best_len {
                best_len = dir.len();
                best = Some(p);
            }
        }
    }
    best
}

pub fn guard_state(repo: &Path) -> String {
    match hooks_dir(repo) {
        Ok(h) => guard_state_in(&h.dir),
        Err(_) => guard_state_in(&repo.join(".git").join("hooks")),
    }
}

/// Where this repository's hooks live, from git itself. `.git/hooks` is only
/// right for a plain checkout: in a worktree or a submodule `.git` is a file
/// pointing elsewhere, and `core.hooksPath` moves the directory anywhere —
/// a guard written to `.git/hooks` there is never run.
pub(crate) struct HooksDir {
    pub dir: PathBuf,
    /// `core.hooksPath` is set: the directory is shared or managed by another
    /// tool (husky, lefthook), and not GitSwitch's to write into.
    pub redirected: Option<String>,
}

pub(crate) fn hooks_dir(repo: &Path) -> Result<HooksDir, AppError> {
    let said = git(repo, &["rev-parse", "--git-path", "hooks"]).map_err(AppError::Command)?;
    let raw = PathBuf::from(said.trim());
    let dir = if raw.is_absolute() { raw } else { repo.join(raw) };
    let redirected = git(repo, &["config", "--get", "core.hooksPath"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(HooksDir { dir, redirected })
}

/// Same answer, but for a hooks directory already resolved with
/// `git rev-parse --git-path hooks` — which is the only correct way to find it
/// in a submodule or worktree (`.git` is a file there) or when the repo sets
/// `core.hooksPath`.
pub(crate) fn guard_state_in(hooks: &Path) -> String {
    match std::fs::read_to_string(hooks.join("pre-commit")) {
        Ok(content) if content.contains(MARKER) => "gitswitch".into(),
        Ok(_) => "foreign".into(),
        Err(_) => "none".into(),
    }
}

fn audit_repo(
    repo_path: &str,
    profs: &[profiles::Profile],
    own_emails: &[String],
) -> Option<RepoAudit> {
    let repo = PathBuf::from(repo_path);
    let profile = owning_profile(repo_path, profs)?;
    let expected_email = profile.git_email.trim().to_string();

    // Which commits haven't been pushed yet? Those are always safe to rewrite.
    let has_upstream = git(&repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok();
    let unpushed: Vec<String> = if has_upstream {
        git(&repo, &["log", "@{upstream}..HEAD", "--format=%H"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    } else {
        // No upstream: nothing has been published from this branch.
        git(&repo, &["log", "--format=%H", "-n", "500"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    };

    let log = git(
        &repo,
        &[
            "log",
            &format!("-n{}", MAX_COMMITS_PER_REPO),
            "--format=%H\x1f%h\x1f%an\x1f%ae\x1f%cI\x1f%s",
        ],
    )
    .ok()?;

    let mut mismatched = Vec::new();
    for line in log.lines() {
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 6 {
            continue;
        }
        let (hash, short, an, ae, date, subject) = (f[0], f[1], f[2], f[3], f[4], f[5]);
        if ae.eq_ignore_ascii_case(&expected_email) {
            continue;
        }
        let is_unpushed = unpushed.iter().any(|h| h == hash);
        // Only flag commits that are plausibly OURS: either not yet published,
        // or authored with an email belonging to one of the user's profiles.
        // Otherwise every teammate's commit would look like a mismatch.
        let ours = is_unpushed || own_emails.iter().any(|e| e.eq_ignore_ascii_case(ae));
        if !ours {
            continue;
        }
        mismatched.push(CommitInfo {
            hash: hash.to_string(),
            short: short.to_string(),
            author_name: an.to_string(),
            author_email: ae.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
            pushed: !is_unpushed,
        });
    }

    if mismatched.is_empty() {
        return None;
    }

    let dirty = !git(&repo, &["status", "--porcelain"])
        .unwrap_or_default()
        .is_empty();

    Some(RepoAudit {
        repo_name: crate::paths::base_name(repo_path),
        path: repo_path.to_string(),
        profile_name: profile.name.clone(),
        expected_name: profile.git_name.clone(),
        expected_email,
        unpushed_count: mismatched.iter().filter(|c| !c.pushed).count(),
        pushed_count: mismatched.iter().filter(|c| c.pushed).count(),
        mismatched,
        has_upstream,
        dirty,
        guard: guard_state(&repo),
    })
}

/// Find commits across all profile folders that carry the wrong identity.
pub async fn audit_all() -> Result<Vec<RepoAudit>, AppError> {
    let store = profiles::load_profiles()?;
    let profs = store.profiles.clone();
    let own_emails: Vec<String> = profs.iter().map(|p| p.git_email.clone()).collect();

    let mut repo_paths: Vec<String> = Vec::new();
    for p in &profs {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            for r in repo_scan::scan_repos(dir.clone()).await.unwrap_or_default() {
                if !repo_paths.contains(&r.path) {
                    repo_paths.push(r.path);
                }
            }
        }
    }

    tokio::task::spawn_blocking(move || {
        let mut out: Vec<RepoAudit> = repo_paths
            .iter()
            .filter_map(|p| audit_repo(p, &profs, &own_emails))
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.unpushed_count));
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Rewrite the author/committer of UNPUSHED commits to the folder's identity.
/// Pushed commits are never touched — that would require a force-push.
pub fn fix_unpushed(repo_path: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);

    if !git(&repo, &["status", "--porcelain"])
        .map_err(AppError::Command)?
        .is_empty()
    {
        return Err(AppError::Config(
            "This repo has uncommitted changes. Commit or stash them first — rewriting history needs a clean working tree.".into(),
        ));
    }

    // Determine the range of local-only commits.
    let base = match git(&repo, &["rev-parse", "@{upstream}"]) {
        Ok(up) => up,
        Err(_) => {
            return Err(AppError::Config(
                "This branch has no upstream, so GitSwitch can't tell which commits are unpublished. Push the branch first, or amend manually.".into(),
            ))
        }
    };

    let count = git(&repo, &["rev-list", "--count", &format!("{}..HEAD", base)])
        .map_err(AppError::Command)?
        .parse::<usize>()
        .unwrap_or(0);
    if count == 0 {
        return Err(AppError::Config("No unpushed commits to fix.".into()));
    }

    // Safety net: a ref pointing at the pre-rewrite history.
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let backup = format!("gitswitch-backup-{}", stamp);
    git(&repo, &["branch", &backup]).map_err(AppError::Command)?;

    // --reset-author rewrites author AND committer to the identity git resolves
    // for this directory, which is exactly what the profile sets.
    let res = git(
        &repo,
        &[
            "-c",
            "core.editor=true",
            // With rebase.updateRefs=true git would move the backup branch
            // along with the rewrite — the one thing it exists not to do.
            "-c",
            "rebase.updateRefs=false",
            "rebase",
            "--empty=keep",
            &base,
            "--exec",
            "git commit --amend --reset-author --no-edit --allow-empty",
        ],
    );

    match res {
        Ok(_) => Ok(format!(
            "Rewrote {} unpushed commit{}. Previous history kept on branch {}.",
            count,
            if count == 1 { "" } else { "s" },
            backup
        )),
        Err(e) => {
            let _ = git(&repo, &["rebase", "--abort"]);
            Err(AppError::Command(format!(
                "Rewrite failed and was rolled back ({}). Your history is unchanged; branch {} still points at it.",
                e, backup
            )))
        }
    }
}

const HOOK_TEMPLATE: &str = r#"#!/bin/sh
# >>> GitSwitch identity guard >>>
# Blocks commits whose email doesn't match the profile that owns this folder.
# Managed by GitSwitch — switch it off in GitSwitch: Repository > Changes, the guard beside "Committing as".
# gitswitch-expected-email: __EXPECTED_EMAIL_NOTE__
expected=__EXPECTED_EMAIL_SH__
actual="$(git config user.email)"
if [ "$actual" != "$expected" ]; then
  echo "GitSwitch: commit blocked."
  echo "  this folder should commit as: $expected"
  echo "  git is currently configured as: ${actual:-<unset>}"
  echo "  Check the profile for this folder in GitSwitch, or override with:"
  echo "    git -c user.email=\"$actual\" commit --no-verify ..."
  exit 1
fi
# <<< GitSwitch identity guard <<<
__CHAIN__
"#;

/// The guard's text for this email, with the call to the hook that was there
/// before (or none). A profile's email may hold `"`, `$` or a backtick:
/// single-quoted for the shell, where nothing but `'` itself is special.
fn hook_body(expected_email: &str, chain: &str) -> String {
    HOOK_TEMPLATE
        .replace("__EXPECTED_EMAIL_NOTE__", expected_email)
        .replace("__EXPECTED_EMAIL_SH__", &format!("'{}'", expected_email.replace('\'', "'\\''")))
        .replace("__CHAIN__", chain)
}

/// Install a pre-commit hook that refuses commits with the wrong identity.
/// An existing foreign hook is preserved and chained, never discarded.
pub fn install_guard(repo_path: &str, expected_email: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let resolved = hooks_dir(&repo)?;
    if let Some(p) = resolved.redirected {
        return Err(AppError::Config(format!(
            "This repository routes its hooks through core.hooksPath = {} — a directory another tool manages, which GitSwitch won't write into. Add the identity check to that tool's pre-commit hook, or unset core.hooksPath here.",
            p
        )));
    }
    let hooks = resolved.dir;
    std::fs::create_dir_all(&hooks)?;
    let hook = hooks.join("pre-commit");

    // The hook that was here before GitSwitch is kept and run after the check:
    // moved aside on the first install, and still chained when the guard is
    // written again (a new email, say).
    let saved = hooks.join("pre-commit.gitswitch-saved");
    if hook.exists() {
        let existing = std::fs::read_to_string(&hook)?;
        if !existing.contains(MARKER) {
            std::fs::rename(&hook, &saved)?;
        }
    }
    let chain = if saved.exists() {
        "# run the hook that was here before GitSwitch\nif [ -x \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" ]; then\n  \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" \"$@\" || exit $?\nfi".to_string()
    } else {
        String::new()
    };

    let body = hook_body(expected_email, &chain);
    std::fs::write(&hook, body)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(format!(
        "Guard installed{}",
        if chain.is_empty() {
            String::new()
        } else {
            " (existing hook preserved and chained)".to_string()
        }
    ))
}

/// The email a GitSwitch guard in this hooks directory expects; None when the
/// pre-commit hook there is not GitSwitch's. Hooks written before the email
/// had its own comment line carry it as `expected="…"`.
pub(crate) fn guard_email_in(hooks: &Path) -> Option<String> {
    let text = std::fs::read_to_string(hooks.join("pre-commit")).ok()?;
    if !text.contains(MARKER) {
        return None;
    }
    let lines = || text.lines().map(str::trim);
    lines()
        .find_map(|l| l.strip_prefix("# gitswitch-expected-email:").map(|e| e.trim().to_string()))
        .or_else(|| lines().find_map(|l| l.strip_prefix("expected=\"").and_then(|e| e.strip_suffix('"')).map(str::to_string)))
}

/// The profile a repository commits as: by folder, else by the include file
/// git reads its email from (a remote rule's clone outside every folder).
pub(crate) fn profile_for_repo<'a>(repo: &Path, profs: &'a [profiles::Profile]) -> Option<&'a profiles::Profile> {
    let path = crate::paths::norm(&repo.to_string_lossy());
    owning_profile(&path, profs).or_else(|| {
        let origin = git(repo, &["config", "--show-origin", "--get", "user.email"]).ok()?;
        let file = origin.split('\t').next()?.trim().trim_start_matches("file:").to_string();
        profiles::profile_for_include_origin(&file, profs)
    })
}

/// Bring every GitSwitch guard under these folders in line with the profile
/// that now owns its repository: after a profile's email changed, or a folder
/// moved to another profile, a guard expecting the old email would refuse
/// every commit. A repository no profile owns any more keeps its guard — the
/// commit box offers to switch it off; protection is never dropped silently.
/// One line per guard rewritten.
pub async fn refresh_guards(dirs: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for dir in dirs {
        if let Ok(repos) = repo_scan::scan_repos(dir.clone()).await {
            for r in repos {
                if !paths.contains(&r.path) {
                    paths.push(r.path);
                }
            }
        }
    }
    tokio::task::spawn_blocking(move || {
        let Ok(store) = profiles::load_profiles() else { return Vec::new() };
        let mut out = Vec::new();
        for path in paths {
            let repo = PathBuf::from(&path);
            let Ok(hooks) = hooks_dir(&repo) else { continue };
            let Some(expects) = guard_email_in(&hooks.dir) else { continue };
            let Some(owner) = profile_for_repo(&repo, &store.profiles) else { continue };
            let email = owner.git_email.trim();
            if expects.trim().eq_ignore_ascii_case(email) {
                continue;
            }
            if install_guard(&path, email).is_ok() {
                out.push(format!("{}: the identity guard now expects {} (was {})", path, email, expects));
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

pub fn uninstall_guard(repo_path: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let hooks = hooks_dir(&repo).map(|h| h.dir).unwrap_or_else(|_| repo.join(".git").join("hooks"));
    let hook = hooks.join("pre-commit");
    if !hook.exists() {
        return Ok("No guard installed.".into());
    }
    let content = std::fs::read_to_string(&hook)?;
    if !content.contains(MARKER) {
        return Err(AppError::Config(
            "The pre-commit hook here isn't GitSwitch's — leaving it alone.".into(),
        ));
    }
    std::fs::remove_file(&hook)?;

    // Put back whatever we displaced when installing.
    let saved = hooks.join("pre-commit.gitswitch-saved");
    if saved.exists() {
        std::fs::rename(&saved, &hook)?;
        return Ok("Guard removed; your original pre-commit hook is restored.".into());
    }
    Ok("Guard removed.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn prof(name: &str, email: &str, dirs: &[&str]) -> profiles::Profile {
        profiles::Profile {
            id: name.into(),
            name: name.into(),
            git_name: name.into(),
            git_email: email.into(),
            ssh_key_path: None,
            is_default: false,
            allow_push: true,
            signing_enabled: false,
            remote_patterns: Vec::new(),
            github_login: None,
            directories: dirs.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn longest_matching_folder_owns_the_repo() {
        let profs = vec![
            prof("Personal", "me@home", &["/dev"]),
            prof("Work", "me@work", &["/dev/acme"]),
        ];
        assert_eq!(owning_profile("/dev/acme/api", &profs).unwrap().name, "Work");
        assert_eq!(owning_profile("/dev/side", &profs).unwrap().name, "Personal");
        assert!(owning_profile("/elsewhere/x", &profs).is_none());
    }

    #[test]
    fn trailing_slashes_do_not_break_ownership() {
        let profs = vec![prof("P", "e", &["/dev/work/"])];
        assert!(owning_profile("/dev/work/repo", &profs).is_some());
        // A sibling that merely shares a prefix must NOT match.
        assert!(owning_profile("/dev/workshop/repo", &profs).is_none());
    }

    #[test]
    fn hook_template_embeds_email_and_marker() {
        let body = hook_body("a@b.com", "");
        assert!(body.contains("expected='a@b.com'"));
        assert!(body.contains("# gitswitch-expected-email: a@b.com"));
        assert!(body.contains(MARKER));
        assert!(body.starts_with("#!/bin/sh"));
        // A quote in the email cannot end the shell's string.
        assert!(hook_body("o'neil@b.com", "").contains(r"expected='o'\''neil@b.com'"));
    }

    #[test]
    fn the_guard_email_is_read_back_from_new_and_old_hooks() {
        let dir = std::env::temp_dir().join(format!("gitswitch-guard-email-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hook = dir.join("pre-commit");
        std::fs::write(&hook, hook_body("new@x.test", "")).unwrap();
        assert_eq!(guard_email_in(&dir).as_deref(), Some("new@x.test"));
        // Written by a build before the email had a line of its own.
        std::fs::write(&hook, format!("#!/bin/sh\n# >>> {} >>>\nexpected=\"old@x.test\"\n", MARKER)).unwrap();
        assert_eq!(guard_email_in(&dir).as_deref(), Some("old@x.test"));
        // Somebody else's hook: no guard of ours to read.
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(guard_email_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
