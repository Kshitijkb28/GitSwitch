use crate::error::AppError;
use crate::profiles;
use crate::repo_scan;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The profile that owns a repo path — longest matching directory wins, which
/// mirrors how the generated gitconfig resolves overlapping folders.
fn owning_profile<'a>(
    repo: &str,
    profs: &'a [profiles::Profile],
) -> Option<&'a profiles::Profile> {
    let mut best: Option<&profiles::Profile> = None;
    let mut best_len = 0usize;
    for p in profs {
        for d in &p.directories {
            let dir = d.trim_end_matches('/');
            if (repo == dir || repo.starts_with(&format!("{}/", dir))) && dir.len() >= best_len {
                best_len = dir.len();
                best = Some(p);
            }
        }
    }
    best
}

pub fn guard_state(repo: &Path) -> String {
    let hook = repo.join(".git").join("hooks").join("pre-commit");
    match std::fs::read_to_string(&hook) {
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
        repo_name: repo_path.rsplit('/').next().unwrap_or(repo_path).to_string(),
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
        out.sort_by(|a, b| b.unpushed_count.cmp(&a.unpushed_count));
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
        .map_err(|e| AppError::Command(e))?
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
# Managed by GitSwitch — remove via the app (Commit Audit -> guard off).
expected="__EXPECTED_EMAIL__"
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

/// Install a pre-commit hook that refuses commits with the wrong identity.
/// An existing foreign hook is preserved and chained, never discarded.
pub fn install_guard(repo_path: &str, expected_email: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let hooks = repo.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks)?;
    let hook = hooks.join("pre-commit");

    let mut chain = String::new();
    if hook.exists() {
        let existing = std::fs::read_to_string(&hook)?;
        if !existing.contains(MARKER) {
            let saved = hooks.join("pre-commit.gitswitch-saved");
            std::fs::rename(&hook, &saved)?;
            chain = format!(
                "# run the hook that was here before GitSwitch\nif [ -x \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" ]; then\n  \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" \"$@\" || exit $?\nfi"
            );
        }
    }

    let body = HOOK_TEMPLATE
        .replace("__EXPECTED_EMAIL__", expected_email)
        .replace("__CHAIN__", &chain);
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

pub fn uninstall_guard(repo_path: &str) -> Result<String, AppError> {
    let hooks = PathBuf::from(repo_path).join(".git").join("hooks");
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
        let body = HOOK_TEMPLATE
            .replace("__EXPECTED_EMAIL__", "a@b.com")
            .replace("__CHAIN__", "");
        assert!(body.contains("expected=\"a@b.com\""));
        assert!(body.contains(MARKER));
        assert!(body.starts_with("#!/bin/sh"));
    }
}
