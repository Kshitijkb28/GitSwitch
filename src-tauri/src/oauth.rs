use crate::error::AppError;
use serde::{Deserialize, Serialize};

// https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow
const GITHUB_CLIENT_ID: &str = "Ov23li0000000000000";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeApiResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenPollResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    error: Option<String>,
}

pub async fn request_device_code(client_id: Option<&str>) -> Result<DeviceCodeResponse, AppError> {
    let cid = client_id.unwrap_or(GITHUB_CLIENT_ID);
    let client = reqwest::Client::new();

    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "GitSwitch/0.1.0")
        .form(&[
            ("client_id", cid),
            ("scope", "read:user user:email admin:public_key"),
        ])
        .send()
        .await
        .map_err(|e| AppError::Command(format!("Failed to request device code: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Command(format!(
            "GitHub returned {}: {}",
            status, body
        )));
    }

    let api_resp: DeviceCodeApiResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse device code response: {}", e)))?;

    Ok(DeviceCodeResponse {
        device_code: api_resp.device_code,
        user_code: api_resp.user_code,
        verification_uri: api_resp.verification_uri,
        expires_in: api_resp.expires_in,
        interval: api_resp.interval,
    })
}

pub async fn poll_for_token(
    device_code: &str,
    client_id: Option<&str>,
) -> Result<OAuthTokenResponse, AppError> {
    let cid = client_id.unwrap_or(GITHUB_CLIENT_ID);
    let client = reqwest::Client::new();

    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("User-Agent", "GitSwitch/0.1.0")
        .form(&[
            ("client_id", cid),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| AppError::Command(format!("Failed to poll for token: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Command(format!(
            "GitHub returned {}: {}",
            status, body
        )));
    }

    let poll_resp: TokenPollResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse token response: {}", e)))?;

    if let Some(error) = poll_resp.error {
        return Err(AppError::Command(error));
    }

    match (poll_resp.access_token, poll_resp.token_type, poll_resp.scope) {
        (Some(token), Some(token_type), Some(scope)) => Ok(OAuthTokenResponse {
            access_token: token,
            token_type,
            scope,
        }),
        _ => Err(AppError::Command(
            "Incomplete token response from GitHub".to_string(),
        )),
    }
}
