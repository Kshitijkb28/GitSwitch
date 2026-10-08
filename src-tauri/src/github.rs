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
    let client = http_client()?;
    let api = crate::remote_repos::api_base();
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
    let client = http_client()?;
    let body = serde_json::json!({
        "title": title,
        "key": public_key
    });
    let resp = client
        .post(format!("{}/user/keys", crate::remote_repos::api_base()))
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
    let (status, body, _) = post_ssh_key(token, title, public_key).await?;
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

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
    let client = http_client()?;
    let resp = client
        .get(format!("https://api.github.com/repos/{}/{}", owner, repo))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    match resp.status().as_u16() {
        404 => Ok(None),
        403 | 429 => Err(AppError::Command(
            "GitHub returned 403/429 — rate limited or the token needs SSO \
             authorization for this organization; access could not be verified"
                .into(),
        )),
        s if (200..300).contains(&s) => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))?;
            let mut perms: RepoPermissions = body
                .get("permissions")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| AppError::Command(format!("Failed to parse permissions: {}", e)))?
                .unwrap_or_default();
            // Visibility implies at least read access.
            perms.pull = true;
            Ok(Some(perms))
        }
        s => Err(AppError::Command(format!("GitHub API returned {}", s))),
    }
}

pub async fn list_ssh_keys_github(token: &str) -> Result<Vec<GitHubKey>, AppError> {
    let client = http_client()?;
    let resp = client
        .get(format!("{}/user/keys", crate::remote_repos::api_base()))
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
