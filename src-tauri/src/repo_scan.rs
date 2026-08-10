use crate::error::AppError;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct ScannedRepo {
    pub path: String,
    /// origin URL with any embedded token masked.
    pub remote_url: String,
    /// GitHub owner/name when the remote is a GitHub URL; empty otherwise.
    pub owner: String,
    pub name: String,
}

fn owner_name_from_path(path: &str) -> Option<(String, String)> {
    let p = path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut it = p.splitn(2, '/');
    let owner = it.next()?.trim();
    let name = it.next()?.trim();
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

/// Extract (owner, name) from any common GitHub remote URL form, including
/// ssh-config host aliases (git@github.com-work:o/r — the classic manual
/// multi-account setup) and port-qualified ssh (ssh://git@github.com:22/o/r,
/// ssh://git@ssh.github.com:443/o/r).
pub fn parse_github_owner_name(url: &str) -> Option<(String, String)> {
    let u = url.trim();

    // ssh://[user@]host[:port]/owner/repo
    if let Some(rest) = u.strip_prefix("ssh://") {
        let rest = match rest.split_once('@') {
            Some((_user, r)) => r,
            None => rest,
        };
        let (host_port, path) = rest.split_once('/')?;
        let host = host_port.split(':').next().unwrap_or(host_port);
        if host == "github.com" || host == "ssh.github.com" || host.starts_with("github.com-") {
            return owner_name_from_path(path);
        }
        return None;
    }

    // https/http://[creds@]github.com/owner/repo
    if let Some(after_scheme) = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
    {
        let host_rest = match after_scheme.split_once('@') {
            Some((_creds, rest)) => rest,
            None => after_scheme,
        };
        let (host, path) = host_rest.split_once('/')?;
        if host == "github.com" || host == "www.github.com" {
            return owner_name_from_path(path);
        }
        return None;
    }

    // scp-like: [user@]host:owner/repo — host may be an ssh-config alias
    // (github.com-work) that ultimately points at github.com.
    if let Some((host_part, path)) = u.split_once(':') {
        let host = host_part.rsplit('@').next().unwrap_or(host_part);
        if host == "github.com" || host.starts_with("github.com-") {
            return owner_name_from_path(path);
        }
    }

    None
}

/// Hide any credentials embedded in a remote URL — covers BOTH forms GitHub
/// documents: `https://user:token@…` and the token-as-username `https://token@…`.
fn mask_token(url: &str) -> String {
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            if let Some((_creds, host)) = rest.split_once('@') {
                return format!("{}***@{}", scheme, host);
            }
        }
    }
    url.to_string()
}

fn get_origin(dir: &PathBuf) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

fn scan_sync(root: &str) -> Result<Vec<ScannedRepo>, AppError> {
    let root_path = PathBuf::from(root);
    if !root_path.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", root)));
    }

    const MAX_DEPTH: u32 = 5;
    let mut repos = Vec::new();
    let mut stack = vec![(root_path, 0u32)];

    while let Some((dir, depth)) = stack.pop() {
        if dir.join(".git").exists() {
            let (remote_url, owner, name) = match get_origin(&dir) {
                Some(url) => {
                    let (o, n) = parse_github_owner_name(&url).unwrap_or_default();
                    (mask_token(&url), o, n)
                }
                None => (String::new(), String::new(), String::new()),
            };
            repos.push(ScannedRepo {
                path: dir.to_string_lossy().to_string(),
                remote_url,
                owner,
                name,
            });
            // Don't descend into a repo's own subfolders.
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let dirname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if dirname.starts_with('.')
                    || matches!(dirname, "node_modules" | "target" | "Library" | "dist" | "build")
                {
                    continue;
                }
                stack.push((path, depth + 1));
            }
        }
    }

    repos.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(repos)
}

/// Walk `root` (skipping noise dirs) and report every git repo with its origin.
/// Runs on a blocking thread — filesystem walks can be large.
pub async fn scan_repos(root: String) -> Result<Vec<ScannedRepo>, AppError> {
    tokio::task::spawn_blocking(move || scan_sync(&root))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_github_url_forms() {
        let cases = [
            "git@github.com:Ethara-Ai/unified-personas.git",
            "https://github.com/Ethara-Ai/unified-personas.git",
            "https://github.com/Ethara-Ai/unified-personas",
            "https://user:ghp_tok@github.com/Ethara-Ai/unified-personas.git",
            "ssh://git@github.com/Ethara-Ai/unified-personas.git",
        ];
        for c in cases {
            assert_eq!(
                parse_github_owner_name(c),
                Some(("Ethara-Ai".to_string(), "unified-personas".to_string())),
                "failed for {c}"
            );
        }
    }

    #[test]
    fn rejects_non_github_and_malformed() {
        assert_eq!(parse_github_owner_name("https://gitlab.com/o/r.git"), None);
        assert_eq!(parse_github_owner_name("git@github.com:only-owner"), None);
        assert_eq!(parse_github_owner_name("git@gitlab.com:o/r.git"), None);
        assert_eq!(parse_github_owner_name(""), None);
    }

    #[test]
    fn parses_alias_and_port_qualified_forms() {
        // ssh-config host alias — the classic manual multi-account setup
        assert_eq!(
            parse_github_owner_name("git@github.com-work:acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
        // port-qualified ssh
        assert_eq!(
            parse_github_owner_name("ssh://git@github.com:22/acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
        // ssh-over-443 endpoint
        assert_eq!(
            parse_github_owner_name("ssh://git@ssh.github.com:443/acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
    }

    #[test]
    fn mask_token_hides_all_credential_forms() {
        let m1 = mask_token("https://user:ghp_aaa@github.com/o/r.git");
        assert!(!m1.contains("ghp_aaa") && m1.contains("***@"));
        // token-as-username (no colon) — previously leaked unmasked
        let m2 = mask_token("https://ghp_bbb@github.com/o/r.git");
        assert!(!m2.contains("ghp_bbb") && m2.contains("***@"));
        let m3 = mask_token("http://ghp_ccc@github.com/o/r.git");
        assert!(!m3.contains("ghp_ccc"));
    }

    #[test]
    fn scan_finds_repos_and_skips_noise_dirs() {
        let base = std::env::temp_dir().join(format!("gitswitch_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // repo at top level, repo nested two deep, decoy inside node_modules,
        // and a repo-inside-a-repo that must NOT be descended into.
        std::fs::create_dir_all(base.join("repo_a/.git")).unwrap();
        std::fs::create_dir_all(base.join("repo_a/vendor/inner/.git")).unwrap();
        std::fs::create_dir_all(base.join("group/sub/repo_b/.git")).unwrap();
        std::fs::create_dir_all(base.join("node_modules/dep/.git")).unwrap();

        let repos = scan_sync(&base.to_string_lossy()).unwrap();
        let names: Vec<&str> = repos
            .iter()
            .map(|r| r.path.rsplit('/').next().unwrap())
            .collect();

        assert!(names.contains(&"repo_a"));
        assert!(names.contains(&"repo_b"));
        assert!(!names.contains(&"inner"), "must not descend into a repo");
        assert!(!names.contains(&"dep"), "must skip node_modules");
        let _ = std::fs::remove_dir_all(&base);
    }
}
