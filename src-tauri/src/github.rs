use crate::error::AppError;
use serde::{Deserialize, Serialize};

/// All GitHub API calls share a client with a hard timeout so one stalled
/// connection can never hang a command (and with it the UI) indefinitely.
fn http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Command(format!("Failed to build HTTP client: {}", e)))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubKey {
    pub id: u64,
    pub title: String,
    pub key: String,
}

pub async fn verify_token(token: &str) -> Result<GitHubUser, AppError> {
    let client = http_client()?;
    let resp = client
        .get("https://api.github.com/user")
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

    resp.json::<GitHubUser>()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))
}

pub async fn upload_ssh_key(token: &str, title: &str, public_key: &str) -> Result<GitHubKey, AppError> {
    let client = http_client()?;
    let body = serde_json::json!({
        "title": title,
        "key": public_key
    });

    let resp = client
        .post("https://api.github.com/user/keys")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(AppError::Command(format!(
            "GitHub API returned {}: {}",
            status, body_text
        )));
    }

    resp.json::<GitHubKey>()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))
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
        .get("https://api.github.com/user/keys")
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
