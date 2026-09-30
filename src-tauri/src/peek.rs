//! "Check GitHub": what is new on the remote branch, WITHOUT downloading
//! anything. `git ls-remote` only reads the remote's branch tips — it writes
//! no objects, no remote-tracking refs, not even FETCH_HEAD — so the folder is
//! byte-for-byte what it was. A count of the new commits comes from the
//! objects if they happen to be here already, else from GitHub's compare API
//! with a signed-in account; otherwise only "moved / not moved" is known.

use crate::error::AppError;
use crate::git_exec::{GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize, Clone)]
pub struct RemotePeek {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub remote: Option<String>,
    /// The branch's tip on the remote right now.
    pub remote_tip: Option<String>,
    /// The tip as of the last fetch (`refs/remotes/<upstream>`).
    pub known_tip: Option<String>,
    /// The remote branch is not where the last fetch left it.
    pub changed: bool,
    /// The remote no longer has this branch at all.
    pub branch_gone: bool,
    /// New commits on the remote since the last fetch, when countable.
    pub new_commits: Option<usize>,
    /// "local" (the objects were already here) | "github" (compare API) | null
    pub counted_by: Option<String>,
    pub message: String,
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

/// `owner/repo` from a GitHub remote URL (ssh or https), else None.
pub fn github_slug(url: &str) -> Option<(String, String)> {
    let u = url.trim().trim_end_matches('/');
    let rest = u
        .strip_prefix("git@github.com:")
        .or_else(|| u.strip_prefix("ssh://git@github.com/"))
        .or_else(|| u.strip_prefix("https://github.com/"))
        .or_else(|| u.strip_prefix("http://github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut it = rest.splitn(2, '/');
    let owner = it.next()?.to_string();
    let repo = it.next()?.to_string();
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner, repo))
}

async fn github_compare(owner: &str, repo: &str, base: &str, head: &str) -> Option<usize> {
    let client = crate::github::http_client().ok()?;
    let api = std::env::var("GITSWITCH_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into());
    for account in crate::remote_repos::accounts().await {
        let Ok(token) = crate::remote_repos::token_for(&account.key).await else { continue };
        let resp = client
            .get(format!("{}/repos/{}/{}/compare/{}...{}", api, owner, repo, base, head))
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "GitSwitch/0.1.0")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await;
        let Ok(resp) = resp else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(v) = resp.json::<serde_json::Value>().await else { continue };
        if let Some(n) = v.get("ahead_by").and_then(|n| n.as_u64()) {
            return Some(n as usize);
        }
    }
    None
}

pub async fn peek_remote(repo_path: &str) -> Result<RemotePeek, AppError> {
    let repo = Path::new(repo_path);
    let g = |args: &[&str]| GitCmd::at(repo).args(args.iter().copied()).timeout(LOCAL_TIMEOUT);
    let branch = g(&["symbolic-ref", "--short", "-q", "HEAD"]).ok_text().await.filter(|s| !s.is_empty());
    let upstream = g(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).ok_text().await.filter(|s| !s.is_empty());
    let mut out = RemotePeek {
        branch: branch.clone(),
        upstream: upstream.clone(),
        remote: None,
        remote_tip: None,
        known_tip: None,
        changed: false,
        branch_gone: false,
        new_commits: None,
        counted_by: None,
        message: String::new(),
    };
    let Some(upstream) = upstream else {
        out.message = match branch {
            Some(_) => "This branch has no upstream branch to check against.".into(),
            None => "Check out a branch first — a detached HEAD has nothing to compare with.".into(),
        };
        return Ok(out);
    };
    let (remote, rbranch) = match upstream.split_once('/') {
        Some((r, b)) => (r.to_string(), b.to_string()),
        None => (upstream.clone(), String::new()),
    };
    crate::git_exec::validate_remote_name(&remote)?;
    out.remote = Some(remote.clone());
    out.known_tip = g(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{}", upstream)]).ok_text().await.filter(|s| !s.is_empty());

    // Read-only by construction: ls-remote never writes into .git.
    let refspec = format!("refs/heads/{}", rbranch);
    let ls = GitCmd::at(repo)
        .args(["ls-remote", "--heads", "--exit-code", "--", &remote, &refspec])
        .timeout(NET_TIMEOUT)
        .run()
        .await?;
    if ls.code == 2 {
        out.branch_gone = true;
        out.changed = true;
        out.message = format!("{} no longer has a branch named {}.", remote, rbranch);
        return Ok(out);
    }
    if !ls.ok() {
        return Err(AppError::Command(format!("Couldn't reach {}: {}", remote, ls.stderr.trim())));
    }
    let remote_tip = ls
        .text()
        .lines()
        .find_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .ok_or_else(|| AppError::Command(format!("{} answered without a tip for {}", remote, rbranch)))?;
    out.remote_tip = Some(remote_tip.clone());
    out.changed = out.known_tip.as_deref() != Some(remote_tip.as_str());
    if !out.changed {
        out.message = format!("Nothing new on {} since your last fetch.", upstream);
        return Ok(out);
    }
    // Count without downloading: the objects may already be here (someone
    // else fetched, or the tip is one of ours), else ask GitHub's compare API.
    if let Some(known) = &out.known_tip {
        let have = g(&["cat-file", "-e", &format!("{}^{{commit}}", remote_tip)]).run().await.map(|o| o.ok()).unwrap_or(false);
        if have {
            out.new_commits = g(&["rev-list", "--count", &format!("{}..{}", known, remote_tip)]).ok_text().await.and_then(|s| s.trim().parse().ok());
            if out.new_commits.is_some() {
                out.counted_by = Some("local".into());
            }
        }
        if out.new_commits.is_none() {
            let url = g(&["remote", "get-url", &remote]).ok_text().await.unwrap_or_default();
            if let Some((owner, name)) = github_slug(&url) {
                if let Some(n) = github_compare(&owner, &name, known, &remote_tip).await {
                    out.new_commits = Some(n);
                    out.counted_by = Some("github".into());
                }
            }
        }
    }
    out.message = match out.new_commits {
        Some(0) => format!("{} moved to {} — no new commits, the branch was rewritten or reset there. Nothing was downloaded.", upstream, short(&remote_tip)),
        Some(1) => format!("1 new commit on {} since your last fetch ({}). Nothing was downloaded — Fetch to see it.", upstream, short(&remote_tip)),
        Some(n) => format!("{} new commits on {} since your last fetch ({}). Nothing was downloaded — Fetch to see them.", n, upstream, short(&remote_tip)),
        None => format!("{} has moved to {} since your last fetch (new commits you don't have). Nothing was downloaded — Fetch to see them.", upstream, short(&remote_tip)),
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_slugs_come_from_ssh_and_https_urls_only() {
        assert_eq!(github_slug("git@github.com:Kshitijkb28/GitSwitch.git"), Some(("Kshitijkb28".into(), "GitSwitch".into())));
        assert_eq!(github_slug("https://github.com/o/r"), Some(("o".into(), "r".into())));
        assert_eq!(github_slug("ssh://git@github.com/o/r.git"), Some(("o".into(), "r".into())));
        assert_eq!(github_slug("/tmp/remote.git"), None);
        assert_eq!(github_slug("git@gitlab.com:o/r.git"), None);
    }
}
