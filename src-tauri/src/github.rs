use crate::error::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
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
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();
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

pub async fn list_ssh_keys_github(token: &str) -> Result<Vec<GitHubKey>, AppError> {
    let client = reqwest::Client::new();
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
