//! Who a push would authenticate as.
//!
//! The mistake this app exists to prevent is pushing with the work key to a
//! personal repository, or the reverse — and nothing in git says which account
//! a push will use until GitHub has accepted it. For an SSH remote the answer is
//! the key ssh would offer (git's `core.sshCommand`, else `~/.ssh/config`
//! through `ssh -G`, else the agent) and what github.com says to it
//! (`Hi <login>!`). For an HTTPS remote it is whatever the credential helper
//! hands out (`git credential fill`), resolved to a login through the API when
//! the helper stores a placeholder user name. Answers are cached for ten
//! minutes; nothing here is ever written anywhere, and no secret leaves this
//! module — the command log redacts `password=` lines before storing them.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Default)]
pub struct PushIdentity {
    pub remote: Option<String>,
    /// The push URL, credentials masked.
    pub url: Option<String>,
    /// "ssh-key" | "ssh-agent" | "https-credential" | "blocked" | "local" | "none" | "unknown"
    pub via: String,
    pub login: Option<String>,
    pub key_path: Option<String>,
    pub host: Option<String>,
    /// The credential helper's user name, for HTTPS.
    pub account: Option<String>,
    /// The GitHub account the owning profile says it is, when it says.
    pub expected_login: Option<String>,
    /// One sentence a person can act on when `login` is None.
    pub detail: String,
}

const TTL: Duration = Duration::from_secs(10 * 60);

/// login (or none) by cache key, with when it was learned.
type Answers = HashMap<String, (Instant, Option<String>)>;

fn cache() -> &'static Mutex<Answers> {
    static C: OnceLock<Mutex<Answers>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(key: &str) -> Option<Option<String>> {
    cache().lock().ok().and_then(|c| c.get(key).filter(|(at, _)| at.elapsed() < TTL).map(|(_, v)| v.clone()))
}

fn remember(key: String, v: Option<String>) {
    if let Ok(mut c) = cache().lock() {
        c.insert(key, (Instant::now(), v));
    }
}

/// Forget every cached answer — the "Re-check" button.
pub fn forget() {
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
}

/// The identity file in a `core.sshCommand` such as
/// `ssh -i '/Users/me/My Keys/id' -o IdentitiesOnly=yes`.
pub fn key_in_ssh_command(cmd: &str) -> Option<String> {
    let words = shell_words(cmd);
    let mut it = words.iter();
    while let Some(w) = it.next() {
        if w == "-i" {
            return it.next().cloned();
        }
        if let Some(rest) = w.strip_prefix("-i") {
            if !rest.is_empty() && !rest.starts_with('-') {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Enough of sh's word splitting for a command GitSwitch or a person wrote:
/// single and double quotes, backslash escapes outside single quotes.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => {
                            if let Some(n) = chars.next() {
                                cur.push(n);
                            }
                        }
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// A user name a credential helper stores when the real identity is the token.
pub fn is_placeholder_user(u: &str) -> bool {
    matches!(
        u.to_ascii_lowercase().as_str(),
        "" | "x-access-token" | "git" | "token" | "oauth2" | "x-oauth-basic" | "personalaccesstoken" | "pat" | "apikey" | "api-key"
    )
}

/// `host` and `owner/repo.git` from an HTTPS remote URL (credentials dropped).
pub fn https_host_and_path(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit('@').next().unwrap_or(authority).split(':').next().unwrap_or("");
    (!host.is_empty()).then(|| (host.to_string(), path.to_string()))
}

fn base(p: &str) -> String {
    crate::paths::base_name(p)
}

pub async fn push_identity(repo: &str, refresh: bool) -> Result<PushIdentity, AppError> {
    if refresh {
        forget();
    }
    let g = |args: Vec<&str>| GitCmd::at(repo).args(args).ok_text();
    let branch = g(vec!["symbolic-ref", "-q", "--short", "HEAD"]).await;
    let remotes: Vec<String> = g(vec!["remote"])
        .await
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let cfg = |key: String| GitCmd::at(repo).args(["config", "--get", key.as_str()]).ok_text();
    let (branch_remote, merge_ref, push_remote) = match &branch {
        Some(b) => (cfg(format!("branch.{}.remote", b)).await, cfg(format!("branch.{}.merge", b)).await, cfg(format!("branch.{}.pushRemote", b)).await),
        None => (None, None, None),
    };
    let upstream = branch_remote
        .as_ref()
        .filter(|r| r.as_str() != ".")
        .and_then(|r| merge_ref.as_ref().map(|m| format!("{}/{}", r, m.trim_start_matches("refs/heads/"))));
    let inputs = crate::git_ops::PushInputs {
        branch: branch.clone().unwrap_or_default(),
        upstream,
        branch_remote,
        merge_ref,
        push_remote,
        push_default: cfg("remote.pushDefault".to_string()).await,
        remotes,
        // The remote matters here, not whether the branch name agrees.
        set_upstream: true,
    };
    let remote = match crate::git_ops::resolve_push_target(&inputs) {
        Ok(t) => t.remote,
        Err(r) => {
            return Ok(PushIdentity { via: "none".into(), detail: r.message, ..Default::default() });
        }
    };
    let Some(url) = GitCmd::at(repo).args(["remote", "get-url", "--push", &remote]).ok_text().await else {
        return Ok(PushIdentity {
            remote: Some(remote.clone()),
            via: "none".into(),
            detail: format!("The remote {} has no URL.", remote),
            ..Default::default()
        });
    };
    identity_for_url(repo, Some(remote), &url).await
}

/// Who a push to `remote` authenticates as (its push address), the way
/// `push_identity` finds out; None when the remote has no address.
pub async fn identity_of_remote(repo: &str, remote: &str) -> Option<PushIdentity> {
    crate::git_exec::validate_remote_name(remote).ok()?;
    let url = GitCmd::at(repo).args(["remote", "get-url", "--push", remote]).ok_text().await?;
    identity_for_url(repo, Some(remote.to_string()), &url).await.ok()
}

/// Who `url` authenticates as from `repo`: for SSH, the key git names
/// (core.sshCommand) or ssh's own choice, and what github.com says to it;
/// for HTTPS, what the credential helpers hand out.
pub async fn identity_for_url(repo: &str, remote: Option<String>, url: &str) -> Result<PushIdentity, AppError> {
    let masked = crate::cmdlog::redact(url);
    // The profile that applies here — by folder, or by the remote rule that
    // git actually applied (seen in where user.email comes from).
    let expected_login = crate::git_status::profile_for_repo(repo).await.and_then(|p| p.github_login);
    let mut id = PushIdentity {
        remote,
        url: Some(masked),
        expected_login,
        ..Default::default()
    };

    if url.starts_with("gitswitch-push-blocked://") {
        id.via = "blocked".into();
        id.detail = "Pushes are blocked here, so no account would be used.".into();
        return Ok(id);
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        via_https(repo, url, id).await
    } else if let Some(remote) = crate::ssh_config::ssh_host_of_url(url) {
        via_ssh(repo, &remote, id).await
    } else {
        id.via = "local".into();
        id.detail = "The remote is a local path; no account is involved.".into();
        Ok(id)
    }
}

async fn via_ssh(repo: &str, remote: &crate::ssh_config::SshRemote, mut id: PushIdentity) -> Result<PushIdentity, AppError> {
    let alias = remote.host.as_str();
    // 1. The key git names — the profile's, through core.sshCommand's `-i …
    //    IdentitiesOnly=yes`. That is the one case where the key is known.
    let ssh_cmd = GitCmd::at(repo).args(["config", "--get", "core.sshCommand"]).ok_text().await;
    let key_from_git = ssh_cmd.as_deref().and_then(key_in_ssh_command);
    // 2. Otherwise ssh decides, and `ssh -G` says how: the agent's keys first
    //    (so no single file can be named), unless the Host block says
    //    IdentitiesOnly, in which case its IdentityFile list is all ssh tries.
    let resolved = crate::ssh_config::resolve(alias).await;
    let hostname = resolved.as_ref().map(|r| r.hostname.clone()).unwrap_or_else(|| alias.to_string());
    let config_key = resolved
        .as_ref()
        .filter(|r| r.identities_only)
        .and_then(|r| r.identity_files.iter().find(|f| Path::new(f).is_file()).cloned());
    let key = key_from_git.clone().or(config_key);
    id.host = Some(hostname.clone());
    id.key_path = key.clone();
    id.via = if key.is_some() { "ssh-key" } else { "ssh-agent" }.into();

    let is_github = hostname.eq_ignore_ascii_case("github.com") || hostname.eq_ignore_ascii_case("ssh.github.com");
    if !is_github {
        id.detail = format!(
            "{} isn't GitHub, so the account can't be looked up; ssh would offer {}.",
            hostname,
            key.as_deref().map(base).unwrap_or_else(|| "the agent's keys".into())
        );
        return Ok(id);
    }

    // The probe runs ssh exactly as git would: against the alias (so ssh
    // applies the same Host block — IdentityFile, IdentitiesOnly, ProxyJump,
    // agent order), the URL's port when it names one, and `-i` only when
    // core.sshCommand does. Anything else would describe a different push.
    let cache_key = format!(
        "ssh|{}|{}|{}|{}",
        alias,
        remote.user.as_deref().unwrap_or(""),
        remote.port.map(|p| p.to_string()).unwrap_or_default(),
        key_from_git.as_deref().unwrap_or("")
    );
    let login = match cached(&cache_key) {
        Some(v) => v,
        None => {
            let v = github_login_over_ssh(alias, remote.user.as_deref(), remote.port, key_from_git.as_deref()).await;
            if v.is_some() {
                remember(cache_key, v.clone()); // a failure is retried next time
            }
            v
        }
    };
    id.detail = match (&login, &key) {
        (Some(_), Some(k)) => format!("github.com accepted the key {}.", base(k)),
        (Some(_), None) => format!("github.com accepted the key ssh offered for {}.", alias),
        (None, Some(k)) => format!("github.com did not accept the key {} (or could not be reached).", base(k)),
        (None, None) => format!("github.com accepted none of the keys ssh offered for {} (or could not be reached).", alias),
    };
    id.login = login;
    Ok(id)
}

/// `ssh -T [user@]alias`: GitHub greets an accepted key with `Hi <login>!` and
/// exits 1 (no shell). BatchMode: no prompt of any kind.
async fn github_login_over_ssh(alias: &str, user: Option<&str>, port: Option<u16>, key: Option<&str>) -> Option<String> {
    let mut cmd = crate::spawn::async_command("ssh");
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=5"]);
    if let Some(p) = port {
        cmd.arg("-p").arg(p.to_string());
    }
    if let Some(k) = key {
        cmd.args(["-i", k, "-o", "IdentitiesOnly=yes"]);
    }
    cmd.arg(match user {
        Some(u) => format!("{}@{}", u, alias),
        None => alias.to_string(),
    });
    cmd.env("SSH_ASKPASS_REQUIRE", "never").env_remove("SSH_ASKPASS");
    let out = tokio::time::timeout(Duration::from_secs(12), cmd.output()).await.ok()?.ok()?;
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    crate::ssh_keys::interpret_ssh_probe(&combined).flatten()
}

async fn via_https(repo: &str, url: &str, mut id: PushIdentity) -> Result<PushIdentity, AppError> {
    id.via = "https-credential".into();
    let Some((host, path)) = https_host_and_path(url) else {
        id.via = "unknown".into();
        id.detail = "The HTTPS remote URL could not be read.".into();
        return Ok(id);
    };
    id.host = Some(host.clone());
    // What git's credential helpers would hand to this push. `interactive=
    // false` (git ≥ 2.40) and the hardened environment together mean "answer
    // or fail, never ask". The answer is a secret: its output is kept out of
    // the command log altogether.
    let input = format!("protocol=https\nhost={}\npath={}\n", host, path);
    let out = GitCmd::at(repo)
        .cfg("credential.interactive", "false")
        .args(["credential", "fill"])
        .stdin_bytes(input.into_bytes())
        .private_output()
        .timeout(Duration::from_secs(10))
        .run()
        .await;
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            id.detail = format!("The credential helper did not answer: {}", e);
            return Ok(id);
        }
    };
    if !out.ok() {
        id.detail = format!(
            "No credential is stored for {}. git would prompt for one, which cannot happen inside an app, so the push would fail — sign in with `gh auth login`, push once from a terminal, or switch the remote to SSH.",
            host
        );
        return Ok(id);
    }
    let text = out.text();
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).filter(|v| !v.is_empty());
    let username = field("username=");
    let password = field("password=");
    // A token sometimes travels as the USER name (`https://TOKEN:x-oauth-basic@…`,
    // a form GitHub documents and credential stores keep): then it is the
    // secret and never a name to show.
    let token_as_user = username.as_deref().map(looks_like_token).unwrap_or(false)
        || matches!(password.as_deref().map(|p| p.to_ascii_lowercase()).as_deref(), Some("x-oauth-basic") | Some("x"));
    let (token, shown_user) = if token_as_user { (username.clone(), None) } else { (password.clone(), username.clone()) };
    id.account = shown_user.clone().map(|u| crate::cmdlog::redact(&u));

    let is_github = host.eq_ignore_ascii_case("github.com");
    if is_github {
        // The login comes from GitHub's own answer to the token, never from the
        // stored user name: a name is whatever was typed when the credential
        // was saved, and an expired token reads as an account that cannot push.
        let (login, detail) = match token {
            None => (None, format!("A credential for {} is stored without a token, so which account it belongs to cannot be confirmed.", host)),
            Some(t) => {
                let cache_key = format!("https|{}|{}", host, crate::lfs_sha(&t));
                let answer = match cached(&cache_key) {
                    Some(v) => Ok(v),
                    None => {
                        let a = github_login_for_token(&t).await;
                        if let Ok(v) = &a {
                            remember(cache_key, v.clone());
                        }
                        a
                    }
                };
                match answer {
                    Ok(Some(login)) => (Some(login), format!("A stored HTTPS credential for {}.", host)),
                    Ok(None) => (None, "The stored credential for github.com is no longer accepted (expired or revoked), so the push would be refused. Sign in again with `gh auth login`.".to_string()),
                    Err(()) => (None, "GitHub could not be reached to confirm which account the stored credential belongs to.".to_string()),
                }
            }
        };
        id.login = login;
        id.detail = detail;
    } else {
        id.login = shown_user.filter(|u| !is_placeholder_user(u) && !looks_like_token(u));
        id.detail = match &id.login {
            Some(_) => format!("A stored HTTPS credential for {}.", host),
            None => format!("A credential for {} is stored, but which account it belongs to cannot be confirmed.", host),
        };
    }
    id.detail = crate::cmdlog::redact(&id.detail);
    Ok(id)
}

/// A GitHub token by its prefix (`ghp_`, `github_pat_`, …).
pub fn looks_like_token(s: &str) -> bool {
    crate::cmdlog::TOKEN_PREFIXES.iter().any(|p| s.starts_with(p))
}

/// `GET /user` with the token: `Ok(Some(login))`, `Ok(None)` when GitHub
/// rejects the token, `Err(())` when GitHub could not be asked.
async fn github_login_for_token(token: &str) -> Result<Option<String>, ()> {
    let client = crate::github::http_client().map_err(|_| ())?;
    let resp = client
        .get(format!("{}/user", crate::remote_repos::api_base()))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|_| ())?;
    match resp.status().as_u16() {
        200..=299 => {
            let v: serde_json::Value = resp.json().await.map_err(|_| ())?;
            Ok(v.get("login").and_then(|l| l.as_str()).map(String::from))
        }
        401 | 403 => Ok(None),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_read_out_of_a_core_ssh_command_however_it_is_quoted() {
        assert_eq!(key_in_ssh_command("ssh -i /home/me/.ssh/id_work -o IdentitiesOnly=yes").as_deref(), Some("/home/me/.ssh/id_work"));
        assert_eq!(key_in_ssh_command("ssh -i '/Users/me/My Keys/id' -o IdentitiesOnly=yes").as_deref(), Some("/Users/me/My Keys/id"));
        assert_eq!(key_in_ssh_command(r#"ssh -i "/Users/me/My Keys/id""#).as_deref(), Some("/Users/me/My Keys/id"));
        assert_eq!(key_in_ssh_command(r"ssh -i /Users/me/My\ Keys/id").as_deref(), Some("/Users/me/My Keys/id"));
        assert_eq!(key_in_ssh_command("ssh -i/k -v").as_deref(), Some("/k"));
        assert_eq!(key_in_ssh_command("ssh -o IdentitiesOnly=yes"), None);
        assert_eq!(key_in_ssh_command(""), None);
    }

    #[test]
    fn a_token_is_recognised_by_its_prefix_wherever_it_travels() {
        assert!(looks_like_token("ghp_16C7e42F292c6912E7710c838347Ae178B4a"));
        assert!(looks_like_token("github_pat_11AAAAAA"));
        assert!(!looks_like_token("octocat"));
    }

    #[test]
    fn placeholder_user_names_are_told_from_real_logins() {
        for p in ["x-access-token", "git", "token", "oauth2", "PersonalAccessToken", ""] {
            assert!(is_placeholder_user(p), "{p}");
        }
        for real in ["octocat", "Ann-Example", "x-access"] {
            assert!(!is_placeholder_user(real), "{real}");
        }
    }

    #[test]
    fn host_and_path_come_out_of_an_https_remote_without_its_credentials() {
        assert_eq!(https_host_and_path("https://github.com/acme/x.git"), Some(("github.com".into(), "acme/x.git".into())));
        assert_eq!(https_host_and_path("https://user:ghp_x@github.com/acme/x"), Some(("github.com".into(), "acme/x".into())));
        assert_eq!(https_host_and_path("https://ghe.example.com:8443/acme/x.git"), Some(("ghe.example.com".into(), "acme/x.git".into())));
        assert_eq!(https_host_and_path("git@github.com:acme/x.git"), None);
    }
}
