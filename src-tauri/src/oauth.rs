use crate::error::AppError;
use serde::{Deserialize, Serialize};

// https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow
// GitSwitch OAuth App (Device Flow enabled). Frontend passes this explicitly;
// kept here as the fallback default too.
const GITHUB_CLIENT_ID: &str = "Ov23lieKPekuiZoTUNvb";

/// Where the device flow is asked. A constant: the tests point the `_at`
/// functions at a local server, and a release build talks to GitHub only.
const GITHUB_WEB: &str = "https://github.com";

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
    /// Sent with `slow_down`: the interval GitHub now wants, in seconds.
    interval: Option<u64>,
}

pub async fn request_device_code(client_id: Option<&str>) -> Result<DeviceCodeResponse, AppError> {
    request_device_code_at(GITHUB_WEB, client_id).await
}

/// The sign-in's scopes. A narrower approval asks for its own (`SCOPED`).
const SIGN_IN_SCOPE: &str = "read:user user:email admin:public_key repo read:org";

/// The only other scopes the device flow may ask for: deleting a repository
/// (repo_admin's approval, kept in memory, never stored).
const SCOPED: &[&str] = &["repo delete_repo"];

/// github.com, or in a debug build GITSWITCH_GITHUB_WEB (the suites' fake).
fn web_base() -> String {
    #[cfg(debug_assertions)]
    if let Some(s) = std::env::var("GITSWITCH_GITHUB_WEB").ok().filter(|s| !s.trim().is_empty()) {
        return s.trim_end_matches('/').to_string();
    }
    GITHUB_WEB.to_string()
}

/// A device code for one of the allowed narrower approvals, never the
/// frontend's to choose.
pub(crate) async fn request_device_code_scoped(scope: &str) -> Result<DeviceCodeResponse, AppError> {
    if !SCOPED.contains(&scope) {
        return Err(AppError::Config(format!("GitSwitch doesn't ask GitHub for \"{}\"", scope)));
    }
    request_device_code_with(&web_base(), None, scope).await
}

/// One poll for a scoped approval's token (kept by the backend).
pub(crate) async fn poll_for_token_web(device_code: &str) -> Result<OAuthTokenResponse, AppError> {
    poll_for_token_at(&web_base(), device_code, None).await
}

/// The shared client gives every request a time limit: a GitHub that accepts
/// the connection and never answers ends the request instead of leaving the
/// page waiting for ever.
async fn request_device_code_at(base: &str, client_id: Option<&str>) -> Result<DeviceCodeResponse, AppError> {
    request_device_code_with(base, client_id, SIGN_IN_SCOPE).await
}

async fn request_device_code_with(base: &str, client_id: Option<&str>, scope: &str) -> Result<DeviceCodeResponse, AppError> {
    let cid = client_id.unwrap_or(GITHUB_CLIENT_ID);
    let client = crate::github::http_client()?;

    let resp = client
        .post(format!("{}/login/device/code", base))
        .header("Accept", "application/json")
        .header("User-Agent", "GitSwitch/0.1.0")
        .form(&[
            ("client_id", cid),
            ("scope", scope),
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
    poll_for_token_at(GITHUB_WEB, device_code, client_id).await
}

async fn poll_for_token_at(base: &str, device_code: &str, client_id: Option<&str>) -> Result<OAuthTokenResponse, AppError> {
    let cid = client_id.unwrap_or(GITHUB_CLIENT_ID);
    let client = crate::github::http_client()?;

    let resp = client
        .post(format!("{}/login/oauth/access_token", base))
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
    token_result(poll_resp)
}

/// GitHub's answer to one poll, as the page reads it: a token, or GitHub's
/// own error word. `slow_down` carries the interval GitHub asked for, so the
/// page can wait that long rather than guess.
fn token_result(r: TokenPollResponse) -> Result<OAuthTokenResponse, AppError> {
    if let Some(error) = r.error {
        return Err(AppError::Command(match (error.as_str(), r.interval) {
            ("slow_down", Some(n)) => format!("slow_down interval={}", n),
            _ => error,
        }));
    }
    match (r.access_token, r.token_type, r.scope) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn poll(json: &str) -> Result<OAuthTokenResponse, AppError> {
        token_result(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_slow_down_carries_the_interval_github_asked_for() {
        let e = poll(r#"{"error":"slow_down","interval":10}"#).unwrap_err().to_string();
        assert!(e.contains("slow_down") && e.contains("interval=10"), "{e}");
        let e = poll(r#"{"error":"authorization_pending"}"#).unwrap_err().to_string();
        assert!(e.contains("authorization_pending"), "{e}");
        let t = poll(r#"{"access_token":"gho_x","token_type":"bearer","scope":"read:user"}"#).unwrap();
        assert_eq!(t.access_token, "gho_x");
    }

    /// A GitHub that accepts the connection and never answers: each request
    /// ends with an error instead of hanging (it used to wait for ever, and
    /// the page with it).
    #[tokio::test]
    async fn a_github_that_never_answers_ends_each_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // Accept, read nothing, write nothing: the connection just sits there.
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for s in listener.incoming().flatten() {
                held.push(s);
            }
        });
        let both = tokio::time::timeout(std::time::Duration::from_secs(40), async {
            tokio::join!(poll_for_token_at(&base, "dc", None), request_device_code_at(&base, None))
        })
        .await
        .expect("a request was still waiting after 40 s");
        let (poll, code) = both;
        assert!(poll.unwrap_err().to_string().contains("Failed to poll for token"));
        assert!(code.unwrap_err().to_string().contains("Failed to request device code"));
    }

    #[tokio::test]
    async fn a_server_error_is_named_by_its_status() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            }
        });
        let e = poll_for_token_at(&base, "dc", None).await.unwrap_err().to_string();
        assert!(e.contains("GitHub returned 502"), "{e}");
    }
}
