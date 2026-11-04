//! One way to ask GitHub's REST API something as one of the accounts GitSwitch
//! knows (a GitHub CLI login or a GitSwitch sign-in). The token is looked up
//! here and never leaves this process; the frontend names the account only.

use crate::error::AppError;
use reqwest::Method;
use serde_json::Value;

/// An account's token and the API it talks to (a local fake in tests).
pub(crate) struct Gh {
    api: String,
    token: String,
    /// The login the account key names ("gh:alice" → "alice").
    pub login: String,
}

/// GitHub's answer: status, the headers worth reading, and the JSON body
/// (`Value::Null` when there was none).
pub(crate) struct Reply {
    pub status: u16,
    pub json: Value,
    /// A `rel="next"` page exists (on the same API).
    pub has_next: bool,
    sso: Option<String>,
    scopes: Option<String>,
    accepted: Option<String>,
    rate_left: Option<String>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// GitHub's `X-GitHub-SSO` header: single sign-on withheld something.
    pub fn sso(&self) -> Option<&str> {
        self.sso.as_deref()
    }
}

/// The login an account key names: "gh:<login>" or "signin:<login>".
pub(crate) fn login_of(account: &str) -> &str {
    account.split_once(':').map(|(_, l)| l).unwrap_or(account)
}

/// An owner or repository name as GitHub allows it — so nothing the frontend
/// sends can turn into another path of the API.
pub(crate) fn check_slug(owner: &str, name: &str) -> Result<(), AppError> {
    let fine = |s: &str| !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if fine(owner) && fine(name) {
        Ok(())
    } else {
        Err(AppError::Config(format!("\"{}/{}\" is not a GitHub repository name", owner, name)))
    }
}

/// A GitHub username: letters, digits and single hyphens, at most 39.
pub(crate) fn check_username(u: &str) -> Result<(), AppError> {
    let fine = !u.is_empty() && u.len() <= 39 && !u.starts_with('-') && !u.ends_with('-') && !u.contains("--") && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if fine {
        Ok(())
    } else {
        Err(AppError::Config(format!("\"{}\" is not a GitHub username", u)))
    }
}

impl Gh {
    /// The account's token, for the real API (or the debug override).
    pub async fn for_account(account: &str) -> Result<Gh, AppError> {
        let token = crate::remote_repos::token_for(account).await?;
        Ok(Gh::at(&crate::remote_repos::api_base(), &token, login_of(account)))
    }

    pub fn at(api: &str, token: &str, login: &str) -> Gh {
        Gh { api: api.trim_end_matches('/').to_string(), token: token.to_string(), login: login.to_string() }
    }

    /// One request. Err only when GitHub could not be reached or answered
    /// something that is not JSON; a refusal comes back as a `Reply`.
    pub async fn call(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Reply, AppError> {
        let client = crate::github::http_client()?;
        let mut req = client
            .request(method, format!("{}{}", self.api, path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "GitSwitch/0.1.0")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| AppError::Command(format!("Couldn't reach GitHub: {}", e)))?;
        let status = resp.status().as_u16();
        let header = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        // Paging links are followed only on the API this started with.
        let has_next = header("link")
            .as_deref()
            .and_then(crate::remote_repos::next_link)
            .is_some_and(|n| n.starts_with(&format!("{}/", self.api)));
        let (sso, scopes, accepted, rate_left) = (header("x-github-sso"), header("x-oauth-scopes"), header("x-accepted-oauth-scopes"), header("x-ratelimit-remaining"));
        let bytes = resp.bytes().await.map_err(|e| AppError::Command(format!("GitHub's answer was cut off: {}", e)))?;
        let json = if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            Value::Null
        } else {
            match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(_) if !(200..300).contains(&status) => Value::Null,
                Err(e) => return Err(AppError::Command(format!("Unexpected reply from GitHub: {}", e))),
            }
        };
        Ok(Reply { status, json, has_next, sso, scopes, accepted, rate_left })
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, AppError> {
        self.call(Method::GET, path, query, None).await
    }

    /// A GET that must succeed: a refusal becomes a worded error.
    pub async fn get_ok(&self, path: &str, query: &[(&str, String)], what: &str) -> Result<Value, AppError> {
        let r = self.get(path, query).await?;
        if r.ok() {
            Ok(r.json)
        } else {
            Err(self.refused(&r, what))
        }
    }

    /// GitHub said no: why, in words, with what to do when there is something.
    pub fn refused(&self, r: &Reply, what: &str) -> AppError {
        AppError::Command(refusal(r, what, &self.login))
    }
}

/// GitHub's own message, with the field errors a 422 lists.
pub(crate) fn github_message(json: &Value) -> String {
    let mut msg = json.get("message").and_then(|m| m.as_str()).unwrap_or("").trim().to_string();
    if let Some(errs) = json.get("errors").and_then(|e| e.as_array()) {
        let extra: Vec<String> = errs
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()).map(str::to_string).or_else(|| e.as_str().map(str::to_string)))
            .filter(|m| !m.is_empty())
            .collect();
        if !extra.is_empty() {
            msg = if msg.is_empty() { extra.join("; ") } else { format!("{}: {}", msg, extra.join("; ")) };
        }
    }
    msg
}

pub(crate) fn refusal(r: &Reply, what: &str, login: &str) -> String {
    let said = github_message(&r.json);
    let said_part = if said.is_empty() { String::new() } else { format!(" GitHub said: \"{}\".", said) };
    if r.sso.is_some() {
        return format!(
            "Couldn't {}: the organization uses single sign-on and @{}'s token is not authorized for it. Authorize it (GitHub › Settings › Tokens › Configure SSO, or `gh auth refresh -h github.com`) and try again.",
            what, login
        );
    }
    let has_scope = |s: &str| r.scopes.as_deref().is_some_and(|h| h.split(',').map(str::trim).any(|x| x == s));
    let needs = r
        .accepted
        .as_deref()
        .map(|h| h.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    let missing: Vec<&String> = needs.iter().filter(|s| !has_scope(s)).collect();
    match r.status {
        401 => format!("GitHub rejected @{}'s token (expired or revoked). Sign in again — e.g. `gh auth login`.", login),
        403 | 429 if r.rate_left.as_deref() == Some("0") || said.to_lowercase().contains("rate limit") => {
            format!("Couldn't {}: GitHub's rate limit for @{} is used up. Try again in a few minutes.", what, login)
        }
        403 | 404 if r.scopes.is_some() && !needs.is_empty() && missing.len() == needs.len() => format!(
            "Couldn't {}: @{}'s token lacks the {} scope. Run `gh auth refresh -h github.com -s {}` and try again.",
            what,
            login,
            needs.join(" or "),
            needs[0]
        ),
        403 => format!("Couldn't {}: GitHub does not let @{} do that here (it needs more access to this repository).{}", what, login, said_part),
        404 => format!("Couldn't {}: GitHub shows nothing there to @{} — it doesn't exist, or this account can't see it.", what, login),
        s => format!("Couldn't {}: GitHub answered {}.{}", what, s, said_part),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, json: Value) -> Reply {
        Reply { status, json, has_next: false, sso: None, scopes: None, accepted: None, rate_left: None }
    }

    #[test]
    fn names_that_could_reach_another_api_path_are_refused() {
        assert!(check_slug("octo-org", "my.repo_1").is_ok());
        for (o, n) in [("a/b", "c"), ("a", "b?x=1"), ("..", "x"), ("a", ""), ("a", "b c"), ("a", "%2e%2e")] {
            assert!(check_slug(o, n).is_err(), "{}/{}", o, n);
        }
        assert!(check_username("some-user9").is_ok());
        for u in ["", "-x", "x-", "a--b", "a_b", "a/b", &"x".repeat(40)] {
            assert!(check_username(u).is_err(), "{}", u);
        }
        assert_eq!(login_of("gh:Alice"), "Alice");
        assert_eq!(login_of("signin:bob"), "bob");
    }

    #[test]
    fn refusals_say_why_and_what_to_do() {
        let mut r = reply(403, serde_json::json!({"message": "Resource protected by organization SAML enforcement."}));
        r.sso = Some("required; url=https://github.com/orgs/x/sso".into());
        assert!(refusal(&r, "merge it", "me").contains("single sign-on"));

        let mut r = reply(404, Value::Null);
        r.scopes = Some("read:org, gist".into());
        r.accepted = Some("repo".into());
        let t = refusal(&r, "list pull requests", "me");
        assert!(t.contains("lacks the repo scope") && t.contains("gh auth refresh -h github.com -s repo"), "{}", t);

        let mut r = reply(403, serde_json::json!({"message": "API rate limit exceeded"}));
        r.rate_left = Some("0".into());
        assert!(refusal(&r, "x", "me").contains("rate limit"));

        let r = reply(422, serde_json::json!({"message": "Validation Failed", "errors": [{"message": "Can not approve your own pull request"}]}));
        assert!(refusal(&r, "send the review", "me").contains("Validation Failed: Can not approve your own pull request"));
        assert!(refusal(&reply(401, Value::Null), "x", "me").contains("rejected @me's token"));
        assert!(refusal(&reply(404, Value::Null), "x", "me").contains("doesn't exist, or this account can't see it"));
    }
}
