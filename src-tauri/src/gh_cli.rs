use crate::error::AppError;
use serde::Serialize;

/// `gh` could not be started. "No such file" means it is not installed, which
/// deserves the answer in words — the raw OS error reads like a bug.
fn gh_error(e: std::io::Error) -> AppError {
    if e.kind() == std::io::ErrorKind::NotFound {
        AppError::Command(
            "The GitHub CLI (gh) isn't installed on this computer. Install it from https://cli.github.com, sign in with `gh auth login`, then try again.".into(),
        )
    } else {
        AppError::Command(format!("Failed to run gh: {}", e))
    }
}

/// One account `gh auth status` lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GhAccount {
    pub host: String,
    pub login: String,
    /// "- Active account: true|false"; None from a gh that does not say.
    pub active: Option<bool>,
    /// "- Token scopes: …": Some(empty) for "none", None when gh prints no
    /// scopes line (a fine-grained or app token: scopes do not apply).
    pub scopes: Option<Vec<String>>,
}

/// gh colours its output on a terminal; through a pipe it should not, but a
/// stray escape must not become part of a login.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every account in `gh auth status`, in its order: "✓ Logged in to <host>
/// account <login> (keyring)" and "X Failed to log in to <host> account
/// <login>" each start one; the lines under it say whether it is active and
/// which scopes its token has. The token line itself is never read.
pub(crate) fn parse_auth_status(text: &str) -> Vec<GhAccount> {
    let mut out: Vec<GhAccount> = Vec::new();
    for raw in strip_ansi(text).lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("- Active account:") {
            if let Some(a) = out.last_mut() {
                a.active = Some(rest.trim() == "true");
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("- Token scopes:") {
            if let Some(a) = out.last_mut() {
                let list: Vec<String> = rest
                    .split(',')
                    .map(|s| s.trim().trim_matches('\'').trim_matches('"').trim().to_string())
                    .filter(|s| !s.is_empty() && s != "none")
                    .collect();
                a.scopes = Some(list);
            }
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = words.iter().position(|w| *w == "account") else { continue };
        if i < 2 || i + 1 >= words.len() || words[i - 2] != "to" {
            continue;
        }
        let (host, login) = (words[i - 1].to_string(), words[i + 1].trim_end_matches(':').to_string());
        if login.is_empty() || out.iter().any(|a| a.host == host && a.login == login) {
            continue;
        }
        out.push(GhAccount { host, login, active: None, scopes: None });
    }
    out
}

/// `gh auth status`, both streams: gh writes it to stderr in some versions.
pub async fn gh_auth_status() -> Result<Vec<GhAccount>, AppError> {
    let output = crate::spawn::async_command("gh")
        .args(["auth", "status"])
        .output()
        .await
        .map_err(gh_error)?;
    Ok(parse_auth_status(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

/// The logins of a status listing, once each in its order.
fn logins(accounts: &[GhAccount]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in accounts {
        if !out.contains(&a.login) {
            out.push(a.login.clone());
        }
    }
    out
}

/// List the GitHub accounts the local `gh` CLI is authenticated with.
pub async fn gh_list_accounts() -> Result<Vec<String>, AppError> {
    let accounts = logins(&gh_auth_status().await?);
    if accounts.is_empty() {
        return Err(AppError::Command(
            "No gh accounts found. Run `gh auth login` first.".into(),
        ));
    }
    Ok(accounts)
}

/// The scopes a gh token lacks for what GitSwitch asks of it, each with what
/// needs it: registering SSH keys, private repositories, and — for an account
/// a signing profile uses — registering signing keys. (user:email is the
/// profile form's own concern: it says so where it matters.)
pub(crate) fn missing_scopes(scopes: &[String], needs_signing: bool) -> Vec<(&'static str, &'static str)> {
    let has = |s: &str| scopes.iter().any(|x| x == s);
    let mut out = Vec::new();
    if !has("admin:public_key") && !has("write:public_key") {
        out.push(("admin:public_key", "registering SSH keys on the account"));
    }
    if !has("repo") {
        out.push(("repo", "listing and reaching private repositories"));
    }
    if needs_signing && !has("write:ssh_signing_key") && !has("admin:ssh_signing_key") {
        out.push(("write:ssh_signing_key", "registering a signing key for verified commits"));
    }
    out
}

/// Log a specific account out of the local `gh` CLI (removes its stored
/// token) — from every host gh has it signed in to: the list shows one row
/// per login, and gh refuses `--user` alone when the login is on two hosts.
pub async fn gh_logout(account: &str) -> Result<String, AppError> {
    let hosts: Vec<String> = gh_auth_status()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.login.eq_ignore_ascii_case(account))
        .map(|a| a.host)
        .collect();
    let runs: Vec<Vec<String>> = if hosts.is_empty() {
        vec![vec!["auth".into(), "logout".into(), "--user".into(), account.into()]]
    } else {
        hosts.iter().map(|h| vec!["auth".into(), "logout".into(), "--hostname".into(), h.clone(), "--user".into(), account.into()]).collect()
    };
    let mut removed = false;
    for args in runs {
        let output = crate::spawn::async_command("gh").args(&args).output().await.map_err(gh_error)?;
        if output.status.success() {
            removed = true;
            continue;
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.contains("no accounts matched") {
            // Signed out elsewhere already (a terminal, another app): the row was stale.
            continue;
        }
        let host = args.iter().position(|a| a == "--hostname").and_then(|i| args.get(i + 1)).cloned();
        return Err(AppError::Command(match host {
            Some(h) => format!("gh could not log {} out of {}: {}", account, h, stderr),
            None => format!("gh logout failed for {}: {}", account, stderr),
        }));
    }
    Ok(if removed {
        format!("Removed {} from the GitHub CLI", account)
    } else {
        format!("{} was already signed out of the GitHub CLI", account)
    })
}

/// Get an OAuth token for a specific gh account without switching the active one.
pub async fn gh_get_token(account: &str) -> Result<String, AppError> {
    let output = crate::spawn::async_command("gh")
        .args(["auth", "token", "--user", account])
        .output()
        .await
        .map_err(gh_error)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Command(format!(
            "gh auth token failed for {}: {}",
            account, stderr
        )));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AppError::Command("gh returned an empty token".into()));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "github.com\n  \u{1b}[32m✓\u{1b}[0m Logged in to github.com account alice-dev (keyring)\n  - Active account: true\n  - Git operations protocol: ssh\n  - Token: gho_************************************\n  - Token scopes: 'gist', 'read:org', 'repo'\n\n  ✓ Logged in to github.com account bob-work (keyring)\n  - Active account: false\n  - Git operations protocol: https\n  - Token: gho_************************************\n  - Token scopes: none\n\n  ✓ Logged in to github.com account carol-fg (GH_TOKEN)\n  - Active account: false\n  - Token: github_pat_************************************\n\nghe.example.com\n  ✓ Logged in to ghe.example.com account alice-dev (keyring)\n  X Failed to log in to github.com account dave (keyring)\n";

    #[test]
    fn gh_status_names_each_account_with_its_host_state_and_scopes() {
        let a = parse_auth_status(STATUS);
        let brief: Vec<(&str, &str, Option<bool>)> = a.iter().map(|x| (x.host.as_str(), x.login.as_str(), x.active)).collect();
        assert_eq!(
            brief,
            vec![
                ("github.com", "alice-dev", Some(true)),
                ("github.com", "bob-work", Some(false)),
                ("github.com", "carol-fg", Some(false)),
                ("ghe.example.com", "alice-dev", None),
                ("github.com", "dave", None),
            ]
        );
        assert_eq!(a[0].scopes, Some(vec!["gist".into(), "read:org".into(), "repo".into()]));
        assert_eq!(a[1].scopes, Some(vec![]));
        assert_eq!(a[2].scopes, None);
        assert_eq!(logins(&a), vec!["alice-dev", "bob-work", "carol-fg", "dave"]);
        assert!(parse_auth_status("  - Active account: true\n").is_empty());
    }

    #[test]
    fn the_scopes_a_token_lacks_are_named_with_what_needs_them() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let names = |m: Vec<(&str, &str)>| m.into_iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>();
        assert_eq!(names(missing_scopes(&s(&["write:public_key", "repo"]), false)), Vec::<String>::new());
        assert_eq!(names(missing_scopes(&s(&["read:public_key", "repo"]), false)), vec!["admin:public_key"]);
        assert_eq!(names(missing_scopes(&s(&["admin:public_key", "repo", "admin:ssh_signing_key"]), true)), Vec::<String>::new());
        assert_eq!(names(missing_scopes(&s(&["admin:public_key", "repo"]), true)), vec!["write:ssh_signing_key"]);
        assert_eq!(names(missing_scopes(&s(&[]), false)), vec!["admin:public_key", "repo"]);
    }
}
