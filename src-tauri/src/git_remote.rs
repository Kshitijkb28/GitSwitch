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

/// Turn an https(://user:token@)github.com/owner/repo(.git) URL into git@github.com:owner/repo.git.
/// Returns None if it isn't a convertible GitHub HTTPS URL (e.g. already SSH).
fn https_to_ssh(url: &str) -> Option<String> {
    if url.starts_with("git@") {
        return None;
    }
    let after = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    // Drop any embedded credentials (user:token@)
    let after = match after.split_once('@') {
        Some((_creds, rest)) => rest,
        None => after,
    };
    let (host, path) = after.split_once('/')?;
    if !host.contains("github.com") {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = if path.ends_with(".git") {
        path.to_string()
    } else {
        format!("{}.git", path)
    };
    Some(format!("git@github.com:{}", path))
}

fn get_origin(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn convert_one(dir: &Path) -> Option<RemoteChange> {
    let old_url = get_origin(dir)?;
    let repo = dir.to_string_lossy().to_string();
    // Mask any token when echoing the old url back to the UI.
    let masked = mask_token(&old_url);
    match https_to_ssh(&old_url) {
        Some(ssh) => {
            let set = std::process::Command::new("git")
                .args(["-C", &repo, "remote", "set-url", "origin", &ssh])
                .output();
            match set {
                Ok(o) if o.status.success() => Some(RemoteChange {
                    repo,
                    old_url: masked,
                    new_url: ssh,
                    changed: true,
                    note: "Converted to SSH".into(),
                }),
                Ok(o) => Some(RemoteChange {
                    repo,
                    old_url: masked,
                    new_url: old_url,
                    changed: false,
                    note: format!("Failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                }),
                Err(e) => Some(RemoteChange {
                    repo,
                    old_url: masked,
                    new_url: old_url,
                    changed: false,
                    note: format!("git error: {}", e),
                }),
            }
        }
        None => Some(RemoteChange {
            repo,
            old_url: masked.clone(),
            new_url: masked,
            changed: false,
            note: "Already SSH / not a GitHub HTTPS remote".into(),
        }),
    }
}

fn mask_token(url: &str) -> String {
    // Mask ANY embedded credentials, both `user:token@` and the
    // token-as-username `token@` form, for https and http alike.
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            if let Some((_creds, host)) = rest.split_once('@') {
                return format!("{}***@{}", scheme, host);
            }
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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

/// Find git repos within `root` (up to a few levels deep) and convert their origin to SSH.
pub fn convert_repos_in_dir(root: &str) -> Result<Vec<RemoteChange>, AppError> {
    let root_path = PathBuf::from(root);
    if !root_path.exists() {
        return Err(AppError::Config(format!("Folder not found: {}", root)));
    }

    let mut changes = Vec::new();
    let mut stack = vec![(root_path, 0u32)];
    const MAX_DEPTH: u32 = 4;

    while let Some((dir, depth)) = stack.pop() {
        if dir.join(".git").exists() {
            if let Some(change) = convert_one(&dir) {
                changes.push(change);
            }
            // Don't descend into a repo's subfolders.
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
                    // Skip noise
                    if name == "node_modules" || name == ".git" || name.starts_with('.') {
                        continue;
                    }
                    stack.push((path, depth + 1));
                }
            }
        }
    }

    Ok(changes)
}
