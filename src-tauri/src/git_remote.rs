use crate::error::AppError;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Clone)]
pub struct RemoteChange {
    pub repo: String,
    pub old_url: String,
    pub new_url: String,
    pub changed: bool,
    pub note: String,
}

/// An HTTPS GitHub address as its SSH form: `https://[user[:token]@]github.com[:port]/owner/repo[.git][/][?…][#…]`
/// → `git@github.com:owner/repo.git`, and a gist's `https://gist.github.com/<id>[.git]`
/// → `git@gist.github.com:<id>.git`. The host must be GitHub's exactly — a host
/// that only contains "github.com" is someone else's — and credentials are
/// dropped. None for anything else (already SSH, another host, a path git
/// would not reach the same repository by).
pub(crate) fn https_to_ssh(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, p),
        None => (rest, ""),
    };
    // userinfo ends at the LAST '@' of the authority; a port after the host.
    let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = host_port.split(':').next().unwrap_or("").to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match host.as_str() {
        "github.com" | "www.github.com" if segments.len() == 2 => Some(format!("git@github.com:{}/{}.git", segments[0], segments[1])),
        "gist.github.com" if segments.len() == 1 => Some(format!("git@gist.github.com:{}.git", segments[0])),
        _ => None,
    }
}

fn git_out(dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    // The one runner, on the local clock (this writes config). A command
    // that could not run or was stopped is `None`, as one that could not
    // start always was; the exit code comes back as the status it was.
    let out = crate::git_exec::GitCmd::at(dir)
        .args(args.iter().copied())
        .timeout(crate::git_exec::LOCAL_TIMEOUT)
        .run_blocking()
        .ok()?;
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw((out.code & 0xff) << 8)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(out.code as u32)
    };
    Some(std::process::Output { status, stdout: out.stdout, stderr: out.stderr.into_bytes() })
}

/// Every value of one local config key, converted where it can be: Ok(Some)
/// with the values written back in order when something changed, Ok(None)
/// when nothing needed to.
fn rewrite_key(dir: &Path, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(out) = git_out(dir, &["config", "--local", "--get-all", key]) else { return Err("git could not be run".into()) };
    if !out.status.success() {
        return Ok(None); // the key is not set
    }
    let old: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let new: Vec<String> = old.iter().map(|v| https_to_ssh(v).unwrap_or_else(|| v.clone())).collect();
    if new == old {
        return Ok(None);
    }
    let unset = git_out(dir, &["config", "--local", "--unset-all", key]).ok_or("git could not be run")?;
    if !unset.status.success() {
        return Err(String::from_utf8_lossy(&unset.stderr).trim().to_string());
    }
    for v in &new {
        let add = git_out(dir, &["config", "--local", "--add", key, v]).ok_or("git could not be run")?;
        if !add.status.success() {
            return Err(String::from_utf8_lossy(&add.stderr).trim().to_string());
        }
    }
    Ok(Some(new))
}

/// origin's fetch URL, its push URL, and the push URL a push lock keeps aside
/// to put back on unlock: each converted, so a "converted" repository really
/// fetches and pushes over SSH, with no token left in its config.
fn convert_one(dir: &Path) -> Option<RemoteChange> {
    let repo = dir.to_string_lossy().to_string();
    let old_url = git_out(dir, &["remote", "get-url", "origin"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    let masked = mask_token(&old_url);
    let mut moved: Vec<&str> = Vec::new();
    for (key, what) in [("remote.origin.url", "fetch URL"), ("remote.origin.pushurl", "push URL"), ("gitswitch.savedpushurl.origin", "saved push URL")] {
        match rewrite_key(dir, key) {
            Ok(Some(_)) => moved.push(what),
            Ok(None) => {}
            Err(e) => {
                return Some(RemoteChange { repo, old_url: masked, new_url: mask_token(&old_url), changed: false, note: format!("Failed: {}", e) });
            }
        }
    }
    let new_url = git_out(dir, &["remote", "get-url", "origin"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| old_url.clone());
    if moved.is_empty() {
        return Some(RemoteChange {
            repo,
            old_url: masked.clone(),
            new_url: masked,
            changed: false,
            note: "Already SSH / not a GitHub HTTPS remote".into(),
        });
    }
    let token = old_url != masked;
    Some(RemoteChange {
        repo,
        old_url: masked,
        new_url: mask_token(&new_url),
        changed: true,
        note: format!("Converted to SSH: {}{}", moved.join(", "), if token { "; token removed" } else { "" }),
    })
}

fn mask_token(url: &str) -> String {
    crate::cmdlog::redact(url)
}

/// The populated submodules of a repository, by their path in it — only a
/// plain path inside it: never `.`, an empty one, `..` or an absolute path
/// (a hand-written .gitmodules can say anything), so nothing outside the
/// folder is touched and nothing is visited twice.
fn populated_submodules(dir: &Path) -> Vec<PathBuf> {
    let Some(out) = git_out(dir, &["config", "-f", ".gitmodules", "--get-regexp", r"^submodule\..*\.path$"]) else { return Vec::new() };
    let root = std::fs::canonicalize(dir).ok();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, p)| p.trim().to_string()))
        .filter(|p| !p.is_empty() && Path::new(p).components().all(|c| matches!(c, std::path::Component::Normal(_))))
        .map(|p| dir.join(p))
        .filter(|p| p.join(".git").exists())
        .filter(|p| match (&root, std::fs::canonicalize(p)) {
            (Some(r), Ok(c)) => c.starts_with(r) && &c != r,
            _ => false,
        })
        .collect()
}

/// Find git repos within `root` — the depth and noise folders of the
/// repository scan — and convert their origins to SSH, their populated
/// submodules too (never the committed .gitmodules: only each clone's own
/// config, and the superproject's local submodule.<name>.url).
pub fn convert_repos_in_dir(root: &str) -> Result<Vec<RemoteChange>, AppError> {
    let root_path = PathBuf::from(root);
    if !root_path.exists() {
        return Err(AppError::Config(format!("Folder not found: {}", root)));
    }

    let mut changes = Vec::new();
    let mut stack = vec![(root_path, 0u32)];
    const MAX_DEPTH: u32 = 5;

    while let Some((dir, depth)) = stack.pop() {
        if dir.join(".git").exists() {
            let mut repos = vec![dir.clone()];
            let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
            let mut i = 0;
            while i < repos.len() {
                let r = repos[i].clone();
                i += 1;
                if !seen.insert(std::fs::canonicalize(&r).unwrap_or_else(|_| r.clone())) {
                    continue;
                }
                if let Some(change) = convert_one(&r) {
                    changes.push(change);
                }
                // The superproject's own record of where each submodule lives.
                if let Some(out) = git_out(&r, &["config", "--local", "--name-only", "--get-regexp", r"^submodule\..*\.url$"]) {
                    for key in String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect::<Vec<_>>() {
                        let _ = rewrite_key(&r, &key);
                    }
                }
                repos.extend(populated_submodules(&r));
            }
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let noise = name.starts_with('.') || matches!(name, "node_modules" | "target" | "Library" | "dist" | "build");
                    if noise && !path.join(".git").exists() {
                        continue;
                    }
                    stack.push((path, depth + 1));
                }
            }
        }
    }

    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conversion's runner is the one runner, and an exit code comes
    /// back as the status git ended with.
    #[test]
    fn the_remote_runner_goes_through_gitcmd() {
        let dir = std::env::temp_dir();
        let out = git_out(&dir, &crate::git_exec::test_support::SHOW_ENV).expect("git runs");
        assert!(out.status.success());
        crate::git_exec::test_support::assert_hardened(&String::from_utf8_lossy(&out.stdout));
        let five = git_out(&dir, &["-c", "alias.gitswitch-five=!exit 5", "gitswitch-five"]).expect("git runs");
        assert!(!five.status.success());
        assert_eq!(five.status.code(), Some(5));
    }

    #[test]
    fn converts_plain_https() {
        assert_eq!(
            https_to_ssh("https://github.com/owner/repo.git").as_deref(),
            Some("git@github.com:owner/repo.git")
        );
        // .git suffix is added when missing
        assert_eq!(
            https_to_ssh("https://github.com/owner/repo").as_deref(),
            Some("git@github.com:owner/repo.git")
        );
    }

    #[test]
    fn strips_embedded_credentials() {
        assert_eq!(
            https_to_ssh("https://user:ghp_secret123@github.com/owner/repo.git").as_deref(),
            Some("git@github.com:owner/repo.git")
        );
    }

    #[test]
    fn leaves_ssh_and_non_github_alone() {
        assert_eq!(https_to_ssh("git@github.com:owner/repo.git"), None);
        assert_eq!(https_to_ssh("https://gitlab.com/owner/repo.git"), None);
    }

    #[test]
    fn only_githubs_own_hosts_are_converted_and_each_to_its_own_ssh_form() {
        assert_eq!(https_to_ssh("https://gist.github.com/abc123.git").as_deref(), Some("git@gist.github.com:abc123.git"));
        assert_eq!(https_to_ssh("https://notgithub.com/o/r.git"), None);
        assert_eq!(https_to_ssh("https://github.com.mirror.example/o/r.git"), None);
        assert_eq!(https_to_ssh("https://evil.example/x@github.com/o/r"), None);
        assert_eq!(https_to_ssh("https://GitHub.com/o/r").as_deref(), Some("git@github.com:o/r.git"));
        assert_eq!(https_to_ssh("https://u:t@github.com:443/o/r/").as_deref(), Some("git@github.com:o/r.git"));
        assert_eq!(https_to_ssh("https://github.com/o/r/tree/main"), None);
        assert_eq!(https_to_ssh("https://github.com/o/r.git?x=1#y").as_deref(), Some("git@github.com:o/r.git"));
    }

    #[test]
    fn mask_token_hides_secret_in_both_credential_forms() {
        // user:token@ form
        let m1 = mask_token("https://user:ghp_secret123@github.com/o/r.git");
        assert!(!m1.contains("ghp_secret123"));
        assert!(m1.contains("***@github.com"));
        // token-as-username form (GitHub's documented PAT clone syntax)
        let m2 = mask_token("https://ghp_secret456@github.com/o/r.git");
        assert!(!m2.contains("ghp_secret456"));
        assert!(m2.contains("***@github.com"));
        // http too
        let m3 = mask_token("http://tok@github.com/o/r.git");
        assert!(!m3.contains("tok@"));
    }
}
