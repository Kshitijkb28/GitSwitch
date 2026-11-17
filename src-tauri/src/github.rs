use crate::error::AppError;
use serde::{Deserialize, Serialize};

/// All GitHub API calls share a client with a hard timeout so one stalled
/// connection can never hang a command (and with it the UI) indefinitely.
/// Built once: a reqwest client owns a connection pool and a TLS context, and
/// building one per request threw both away between the pages of a listing.
pub(crate) fn http_client() -> Result<reqwest::Client, AppError> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| AppError::Command(format!("Failed to build HTTP client: {}", e)))
}

/// The client for requests that change something (POST, PUT, PATCH, DELETE):
/// it never follows a redirect. GitHub answers a renamed or transferred
/// repository's old address with a 307 that repeats the same method at the
/// new one, so a delete aimed at one repository could land on whatever now
/// answers there. The caller sees the redirect and says so instead.
pub(crate) fn http_client_for_writes() -> Result<reqwest::Client, AppError> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| AppError::Command(format!("Failed to build HTTP client: {}", e)))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    /// The public address, if the account shows one.
    pub email: Option<String>,
    pub avatar_url: String,
    /// The account's verified addresses, primary first — readable with the
    /// user:email scope, which private addresses need.
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub primary_email: Option<String>,
    /// The token cannot read the account's addresses (no user:email scope).
    #[serde(default)]
    pub email_scope_missing: bool,
}

/// One entry of GET /user/emails.
#[derive(Debug, Deserialize)]
pub struct UserEmail {
    email: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    verified: bool,
}

/// The primary verified address, and every verified one (primary first,
/// once each whatever its letter case). An unverified address is not the
/// account's yet: GitHub would not attribute commits to it.
pub fn pick_emails(list: &[UserEmail]) -> (Option<String>, Vec<String>) {
    let primary = list.iter().find(|e| e.primary && e.verified).map(|e| e.email.clone());
    let mut all: Vec<String> = Vec::new();
    for e in primary.iter().cloned().chain(list.iter().filter(|e| e.verified).map(|e| e.email.clone())) {
        if !all.iter().any(|a| a.eq_ignore_ascii_case(&e)) {
            all.push(e);
        }
    }
    (primary, all)
}

/// Whether a token's scopes (X-OAuth-Scopes) let it read the account's
/// addresses: `user:email`, or the whole `user`. None when GitHub sends no
/// such header (a fine-grained token): then only asking tells.
pub fn has_email_scope(x_oauth_scopes: Option<&str>) -> Option<bool> {
    let h = x_oauth_scopes?;
    Some(h.split(',').map(str::trim).any(|s| s == "user:email" || s == "user"))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubKey {
    pub id: u64,
    pub title: String,
    pub key: String,
}

pub async fn verify_token(token: &str) -> Result<GitHubUser, AppError> {
    verify_token_at(&crate::remote_repos::api_base(), token).await
}

async fn verify_token_at(api: &str, token: &str) -> Result<GitHubUser, AppError> {
    let client = http_client()?;
    let resp = client
        .get(format!("{}/user", api))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Command(format!(
            "GitHub API returned {}",
            resp.status()
        )));
    }
    let scopes = resp.headers().get("x-oauth-scopes").and_then(|v| v.to_str().ok()).map(str::to_string);

    let mut user = resp
        .json::<GitHubUser>()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))?;

    // The account's own addresses, which /user leaves out when they are
    // private. Never a reason for the sign-in itself to fail.
    if has_email_scope(scopes.as_deref()) == Some(false) {
        user.email_scope_missing = true;
    } else {
        let emails = client
            .get(format!("{}/user/emails", api))
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "GitSwitch/0.1.0")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await;
        match emails {
            Ok(r) if r.status().is_success() => {
                if let Ok(list) = r.json::<Vec<UserEmail>>().await {
                    let (primary, all) = pick_emails(&list);
                    user.primary_email = primary;
                    user.emails = all;
                }
            }
            Ok(r) if r.status() == reqwest::StatusCode::FORBIDDEN || r.status() == reqwest::StatusCode::NOT_FOUND => {
                user.email_scope_missing = true;
            }
            _ => {}
        }
    }
    Ok(user)
}

/// POST /user/keys: GitHub's status, its body, and the scope it says the
/// call needs (X-Accepted-OAuth-Scopes), for `key_upload_outcome` to word.
pub async fn post_ssh_key(token: &str, title: &str, public_key: &str) -> Result<(u16, String, Option<String>), AppError> {
    post_ssh_key_at(&crate::remote_repos::api_base(), token, title, public_key).await
}

async fn post_ssh_key_at(api: &str, token: &str, title: &str, public_key: &str) -> Result<(u16, String, Option<String>), AppError> {
    let client = http_client()?;
    let body = serde_json::json!({
        "title": title,
        "key": public_key
    });
    let resp = client
        .post(format!("{}/user/keys", api))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;
    let status = resp.status().as_u16();
    let accepted = resp.headers().get("x-accepted-oauth-scopes").and_then(|v| v.to_str().ok()).map(str::to_string);
    Ok((status, resp.text().await.unwrap_or_default(), accepted))
}

pub async fn upload_ssh_key(token: &str, title: &str, public_key: &str) -> Result<GitHubKey, AppError> {
    upload_ssh_key_at(&crate::remote_repos::api_base(), token, title, public_key).await
}

async fn upload_ssh_key_at(api: &str, token: &str, title: &str, public_key: &str) -> Result<GitHubKey, AppError> {
    let (status, body, _) = post_ssh_key_at(api, token, title, public_key).await?;
    if !(200..300).contains(&status) {
        return Err(AppError::Command(format!("GitHub API returned {}: {}", status, body)));
    }
    serde_json::from_str::<GitHubKey>(&body).map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))
}

/// What a key upload's answer means, in words a person can act on.
/// `on_this_account`: on a 422 "already in use", whether the key is among
/// this account's own keys; `holder`: who it authenticates as otherwise.
pub fn key_upload_outcome(
    account: &str,
    status: u16,
    body: &str,
    needed_scope: Option<&str>,
    on_this_account: Option<bool>,
    holder: Option<&str>,
) -> Result<String, AppError> {
    let scope = needed_scope.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.split(',').map(str::trim).find(|x| !x.is_empty())).unwrap_or("admin:public_key");
    match status {
        200..=299 => Ok(format!("Registered on {}", account)),
        401 => Err(AppError::Command(format!("gh's token for {} is no longer accepted — sign in again with gh auth login.", account))),
        403 | 404 => Err(AppError::Command(format!(
            "This gh login can't add SSH keys to {}. Grant it once with: gh auth refresh -h github.com -s {}, then press Register again.",
            account, scope
        ))),
        422 if body.contains("already in use") || body.contains("key is already") => match (on_this_account, holder) {
            (Some(true), _) => Ok(format!("Already registered on {}", account)),
            (Some(false), Some(h)) if !h.eq_ignore_ascii_case(account) => Err(AppError::Command(format!(
                "This key is already on {}. GitHub lets a key belong to one account only: remove it there, or generate a separate key for {}.",
                h, account
            ))),
            _ => Err(AppError::Command(
                "This key is already on another GitHub account, or is a repository's deploy key. A key can belong to one account only: remove it there, or generate a separate key.".into(),
            )),
        },
        _ => Err(AppError::Command(format!("GitHub returned {}: {}", status, body))),
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct RepoPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub pull: bool,
}

/// What access does this token's account have on owner/repo?
/// Ok(None) = repo not visible to this account (GitHub answers 404 for that).
/// 403/429 are NOT "no access" — they mean rate limiting or SAML-SSO
/// enforcement, i.e. the check itself failed and the caller must not conclude
/// anything about access.
pub async fn repo_permission(
    token: &str,
    owner: &str,
    repo: &str,
) -> Result<Option<RepoPermissions>, AppError> {
    match repo_access(token, owner, repo).await? {
        RepoAccess::Visible(p) => Ok(Some(p)),
        RepoAccess::Invisible => Ok(None),
        RepoAccess::Unknown(why) => Err(AppError::Command(format!("Access could not be verified: {}", why))),
    }
}

/// What GitHub says about one account seeing one repository.
#[derive(Debug, Clone, PartialEq)]
pub enum RepoAccess {
    Visible(RepoPermissions),
    /// A "not found" to a token that could see a private repository: the
    /// account really cannot see it.
    Invisible,
    /// GitHub's answer proves nothing either way, with why.
    Unknown(String),
}

/// GitHub answers "not found" both for a repository that does not exist and
/// for one the token may not see — so a 404 means "cannot see" only from a
/// token with the full `repo` scope. A token that single sign-on has not
/// authorized for the organization is told apart too.
pub fn classify_access(status: u16, oauth_scopes: Option<&str>, sso: Option<&str>, perms: Option<RepoPermissions>) -> RepoAccess {
    if sso.is_some() {
        return RepoAccess::Unknown("single sign-on: the token is not authorized for this organization".into());
    }
    match status {
        200..=299 => {
            let mut p = perms.unwrap_or_default();
            p.pull = true; // seeing it at all is reading it
            RepoAccess::Visible(p)
        }
        404 if oauth_scopes.is_some_and(|h| h.split(',').map(str::trim).any(|s| s == "repo")) => RepoAccess::Invisible,
        404 => RepoAccess::Unknown("this token cannot see private repositories, so GitHub's \"not found\" proves nothing".into()),
        401 => RepoAccess::Unknown("the token is no longer accepted".into()),
        403 | 429 => RepoAccess::Unknown("GitHub refused or rate-limited the question".into()),
        s => RepoAccess::Unknown(format!("GitHub answered {}", s)),
    }
}

/// Ask GitHub whether this token's account can see owner/repo. Err only when
/// GitHub could not be reached.
pub async fn repo_access(token: &str, owner: &str, repo: &str) -> Result<RepoAccess, AppError> {
    repo_access_at(&crate::remote_repos::api_base(), token, owner, repo).await
}

async fn repo_access_at(api: &str, token: &str, owner: &str, repo: &str) -> Result<RepoAccess, AppError> {
    repo_access_detail_at(api, token, owner, repo).await.map(|(access, _)| access)
}

/// `repo_access`, and whether GitHub calls the repository private — only the
/// accounts it was shared with can read it — when the account can see it.
pub async fn repo_access_detail(token: &str, owner: &str, repo: &str) -> Result<(RepoAccess, Option<bool>), AppError> {
    repo_access_detail_at(&crate::remote_repos::api_base(), token, owner, repo).await
}

async fn repo_access_detail_at(api: &str, token: &str, owner: &str, repo: &str) -> Result<(RepoAccess, Option<bool>), AppError> {
    let client = http_client()?;
    let resp = client
        .get(format!("{}/repos/{}/{}", api, owner, repo))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;
    let status = resp.status().as_u16();
    let header = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
    let (scopes, sso) = (header("x-oauth-scopes"), header("x-github-sso"));
    let body = if (200..300).contains(&status) { resp.json::<serde_json::Value>().await.ok() } else { None };
    let perms = body
        .as_ref()
        .and_then(|b| b.get("permissions").cloned())
        .and_then(|p| serde_json::from_value::<RepoPermissions>(p).ok());
    let private = body.as_ref().and_then(|b| b.get("private")).and_then(|v| v.as_bool());
    Ok((classify_access(status, scopes.as_deref(), sso.as_deref(), perms), private))
}

pub async fn list_ssh_keys_github(token: &str) -> Result<Vec<GitHubKey>, AppError> {
    list_ssh_keys_at(&crate::remote_repos::api_base(), token).await
}

async fn list_ssh_keys_at(api: &str, token: &str) -> Result<Vec<GitHubKey>, AppError> {
    let client = http_client()?;
    let resp = client
        .get(format!("{}/user/keys", api))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Command(format!(
            "GitHub API returned {}",
            resp.status()
        )));
    }

    resp.json::<Vec<GitHubKey>>()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))
}

#[cfg(test)]
mod email_tests {
    use super::*;

    fn e(email: &str, primary: bool, verified: bool) -> UserEmail {
        UserEmail { email: email.into(), primary, verified }
    }

    #[test]
    fn the_primary_verified_address_leads_and_unverified_ones_are_left_out() {
        let (p, all) = pick_emails(&[e("old@home.test", false, false), e("alt@home.test", false, true), e("me@home.test", true, true), e("ME@home.test", false, true)]);
        assert_eq!(p.as_deref(), Some("me@home.test"));
        assert_eq!(all, vec!["me@home.test".to_string(), "alt@home.test".to_string()]);
        assert_eq!(pick_emails(&[]), (None, vec![]));
    }

    #[test]
    fn the_email_scope_is_read_from_the_tokens_scopes() {
        assert_eq!(has_email_scope(Some("repo, read:org")), Some(false));
        assert_eq!(has_email_scope(Some("repo, user:email")), Some(true));
        assert_eq!(has_email_scope(Some("user")), Some(true));
        assert_eq!(has_email_scope(Some("")), Some(false));
        assert_eq!(has_email_scope(None), None);
    }
}

#[cfg(test)]
mod key_upload_tests {
    use super::*;

    #[test]
    fn each_answer_to_a_key_upload_is_worded_for_what_to_do() {
        assert!(key_upload_outcome("alice", 201, "{}", None, None, None).unwrap().contains("alice"));
        for status in [403, 404] {
            let e = key_upload_outcome("alice", status, "Not Found", None, None, None).unwrap_err().to_string();
            assert!(e.contains("gh auth refresh -h github.com -s admin:public_key"), "{e}");
        }
        let e = key_upload_outcome("alice", 404, "", Some("write:public_key, admin:public_key"), None, None).unwrap_err().to_string();
        assert!(e.contains("-s write:public_key"), "{e}");
        let used = r#"{"message":"Validation Failed","errors":[{"message":"key is already in use"}]}"#;
        assert!(key_upload_outcome("alice", 422, used, None, Some(true), None).is_ok());
        let e = key_upload_outcome("alice", 422, used, None, Some(false), Some("bob")).unwrap_err().to_string();
        assert!(e.contains("bob"), "{e}");
        assert!(key_upload_outcome("alice", 422, used, None, None, None).is_err());
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;

    #[test]
    fn a_not_found_proves_no_access_only_from_a_token_that_could_see_private_repositories() {
        assert_eq!(classify_access(404, Some("repo, read:org"), None, None), RepoAccess::Invisible);
        assert!(matches!(classify_access(404, Some("read:user"), None, None), RepoAccess::Unknown(_)));
        assert!(matches!(classify_access(404, None, None, None), RepoAccess::Unknown(_)));
        assert!(matches!(classify_access(403, Some("repo"), Some("required; url=https://github.com/orgs/acme/sso"), None), RepoAccess::Unknown(w) if w.contains("single sign-on")));
        let seen = classify_access(200, Some("repo"), None, Some(RepoPermissions { admin: false, push: true, pull: false }));
        assert_eq!(seen, RepoAccess::Visible(RepoPermissions { admin: false, push: true, pull: true }));
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    /// A local GitHub: one canned answer per connection, in order; every
    /// request as it arrived (head and body).
    fn github(answers: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let _ = s.write_all(answer.as_bytes());
            }
        });
        (base, seen)
    }

    fn reply(status: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut head = format!("HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n", status, body.len());
        for (k, v) in headers {
            head.push_str(&format!("{}: {}\r\n", k, v));
        }
        format!("{}\r\n{}", head, body)
    }

    #[tokio::test]
    async fn a_token_is_read_back_as_its_account_and_a_refused_one_says_so() {
        let user = r#"{"id":7,"login":"octo","name":null,"email":null,"avatar_url":""}"#;
        let (api, seen) = github(vec![reply("200 OK", &[("x-oauth-scopes", "repo")], user), reply("401 Unauthorized", &[], "{}")]);
        let u = verify_token_at(&api, "tok-1").await.unwrap();
        assert_eq!(u.login, "octo");
        // No user:email scope: the addresses are not asked for, and the form is told why.
        assert!(u.email_scope_missing);
        let first = seen.lock().unwrap()[0].to_ascii_lowercase();
        assert!(first.starts_with("get /user "), "{first}");
        assert!(first.contains("authorization: bearer tok-1"), "{first}");
        let e = verify_token_at(&api, "tok-2").await.unwrap_err().to_string();
        assert!(e.contains("GitHub API returned 401"), "{e}");
    }

    #[tokio::test]
    async fn the_account_addresses_are_read_when_the_scope_allows() {
        let user = r#"{"id":7,"login":"octo","name":null,"email":null,"avatar_url":""}"#;
        let emails = r#"[{"email":"b@x.test","primary":false,"verified":true},{"email":"a@x.test","primary":true,"verified":true},{"email":"c@x.test","primary":false,"verified":false}]"#;
        let (api, seen) = github(vec![reply("200 OK", &[("x-oauth-scopes", "repo, user:email")], user), reply("200 OK", &[], emails)]);
        let u = verify_token_at(&api, "tok").await.unwrap();
        assert_eq!(u.primary_email.as_deref(), Some("a@x.test"));
        assert_eq!(u.emails, vec!["a@x.test", "b@x.test"]);
        assert!(seen.lock().unwrap()[1].to_ascii_lowercase().starts_with("get /user/emails "));
    }

    #[tokio::test]
    async fn a_key_upload_sends_title_and_key_and_a_refusal_keeps_githubs_words() {
        let (api, seen) = github(vec![
            reply("201 Created", &[], r#"{"id":5,"title":"Mac","key":"ssh-ed25519 AAAA"}"#),
            reply("422 Unprocessable Entity", &[], r#"{"message":"key is already in use"}"#),
        ]);
        let k = upload_ssh_key_at(&api, "tok", "Mac", "ssh-ed25519 AAAA").await.unwrap();
        assert_eq!((k.id, k.title.as_str()), (5, "Mac"));
        let sent = seen.lock().unwrap()[0].clone();
        assert!(sent.starts_with("POST /user/keys "), "{sent}");
        assert!(sent.contains(r#""title":"Mac""#) && sent.contains(r#""key":"ssh-ed25519 AAAA""#), "{sent}");
        let e = upload_ssh_key_at(&api, "tok", "Mac", "ssh-ed25519 AAAA").await.unwrap_err().to_string();
        assert!(e.contains("422") && e.contains("already in use"), "{e}");
    }

    #[tokio::test]
    async fn repository_access_is_only_concluded_from_answers_that_prove_it() {
        let (api, seen) = github(vec![
            reply("200 OK", &[], r#"{"permissions":{"push":true}}"#),
            reply("404 Not Found", &[("x-oauth-scopes", "repo, read:org")], "{}"),
            reply("404 Not Found", &[("x-oauth-scopes", "read:user")], "{}"),
            reply("403 Forbidden", &[("x-github-sso", "required; url=https://github.com/orgs/acme/sso")], "{}"),
            reply("429 Too Many Requests", &[], "{}"),
        ]);
        assert_eq!(repo_access_at(&api, "t", "acme", "a").await.unwrap(), RepoAccess::Visible(RepoPermissions { admin: false, push: true, pull: true }));
        assert_eq!(repo_access_at(&api, "t", "acme", "b").await.unwrap(), RepoAccess::Invisible);
        assert!(matches!(repo_access_at(&api, "t", "acme", "c").await.unwrap(), RepoAccess::Unknown(_)));
        assert!(matches!(repo_access_at(&api, "t", "acme", "d").await.unwrap(), RepoAccess::Unknown(w) if w.contains("single sign-on")));
        assert!(matches!(repo_access_at(&api, "t", "acme", "e").await.unwrap(), RepoAccess::Unknown(w) if w.contains("rate-limited")));
        assert!(seen.lock().unwrap()[1].starts_with("GET /repos/acme/b "));
    }

    #[tokio::test]
    async fn the_account_keys_are_listed_and_a_server_error_is_named() {
        let (api, _) = github(vec![
            reply("200 OK", &[], r#"[{"id":1,"title":"Mac","key":"ssh-ed25519 AAAA"}]"#),
            reply("500 Internal Server Error", &[], "{}"),
        ]);
        let keys = list_ssh_keys_at(&api, "tok").await.unwrap();
        assert_eq!(keys.len(), 1);
        let e = list_ssh_keys_at(&api, "tok").await.unwrap_err().to_string();
        assert!(e.contains("GitHub API returned 500"), "{e}");
    }
}
