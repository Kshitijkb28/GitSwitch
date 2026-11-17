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
    /// A GitSwitch sign-in ("signin:…"), not a GitHub CLI login: how its
    /// token is renewed differs, and so does what a refusal tells the person.
    pub signin: bool,
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

    /// Whether the token has scope `s`: None when GitHub didn't list the
    /// token's scopes (a fine-grained token, or an app's).
    pub(crate) fn has_scope(&self, s: &str) -> Option<bool> {
        self.scopes.as_deref().map(|h| h.split(',').map(str::trim).any(|x| x == s))
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

/// A name for a new repository: what `check_slug` allows, and nothing GitHub
/// would refuse or rewrite (".", "..", a ".git" at the end).
pub(crate) fn check_new_repo_name(name: &str) -> Result<(), AppError> {
    let fine = !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && !name.to_ascii_lowercase().ends_with(".git")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if fine {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "\"{}\" can't be a repository name: use letters, digits, '-', '_' and '.' (at most 100), not ending in .git",
            name
        )))
    }
}

/// GitHub refused because the organization restricts third-party OAuth apps
/// (GitHub's default for new organizations) and hasn't approved this one.
pub(crate) fn oauth_restricted(r: &Reply) -> bool {
    r.status == 403 && github_message(&r.json).to_lowercase().contains("oauth app access restrictions")
}

/// The token's scopes include none of those the endpoint accepts.
pub(crate) fn scope_missing(r: &Reply) -> bool {
    let needs: Vec<&str> = r.accepted.as_deref().map(|h| h.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let has = |s: &str| r.scopes.as_deref().is_some_and(|h| h.split(',').map(str::trim).any(|x| x == s));
    r.scopes.is_some() && !needs.is_empty() && needs.iter().all(|s| !has(s))
}

impl Gh {
    /// The account's token, for the real API (or the debug override).
    pub async fn for_account(account: &str) -> Result<Gh, AppError> {
        let token = crate::remote_repos::token_for(account).await?;
        let mut gh = Gh::at(&crate::remote_repos::api_base(), &token, login_of(account));
        gh.signin = account.starts_with("signin:");
        Ok(gh)
    }

    pub fn at(api: &str, token: &str, login: &str) -> Gh {
        Gh { api: api.trim_end_matches('/').to_string(), token: token.to_string(), login: login.to_string(), signin: false }
    }

    /// One request. Err only when GitHub could not be reached or answered
    /// something that is not JSON; a refusal comes back as a `Reply`.
    pub async fn call(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Reply, AppError> {
        // A request that changes something never follows a redirect.
        let client = if method == Method::GET || method == Method::HEAD { crate::github::http_client()? } else { crate::github::http_client_for_writes()? };
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
        AppError::Command(refusal_as(r, what, &self.login, self.signin))
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
    refusal_as(r, what, login, false)
}

/// `refusal`, worded for the kind of account: a GitSwitch sign-in is renewed
/// on the GitHub Auth page, a GitHub CLI login with `gh`.
pub(crate) fn refusal_as(r: &Reply, what: &str, login: &str, signin: bool) -> String {
    let said = github_message(&r.json);
    let said_part = if said.is_empty() { String::new() } else { format!(" GitHub said: \"{}\".", said) };
    if r.sso.is_some() && signin {
        return format!(
            "Couldn't {}: the organization uses single sign-on and @{}'s GitSwitch sign-in is not authorized for it. Sign in again on the GitHub Auth page and authorize the organization when GitHub asks, then try again.",
            what, login
        );
    }
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
        401 if signin => format!("GitHub rejected @{}'s GitSwitch sign-in (expired or revoked). Sign in again on the GitHub Auth page.", login),
        401 => format!("GitHub rejected @{}'s token (expired or revoked). Sign in again — e.g. `gh auth login`.", login),
        403 | 429 if r.rate_left.as_deref() == Some("0") || said.to_lowercase().contains("rate limit") => {
            format!("Couldn't {}: GitHub's rate limit for @{} is used up. Try again in a few minutes.", what, login)
        }
        403 | 404 if signin && r.scopes.is_some() && !needs.is_empty() && missing.len() == needs.len() => format!(
            "Couldn't {}: @{}'s GitSwitch sign-in lacks the {} scope. Sign in again on the GitHub Auth page: GitSwitch asks for it.",
            what,
            login,
            needs.join(" or ")
        ),
        403 | 404 if r.scopes.is_some() && !needs.is_empty() && missing.len() == needs.len() => format!(
            "Couldn't {}: @{}'s token lacks the {} scope. Run `gh auth refresh -h github.com -s {}` and try again.",
            what,
            login,
            needs.join(" or "),
            needs[0]
        ),
        403 if oauth_restricted(r) => format!(
            "Couldn't {}: the organization restricts third-party apps and hasn't approved {}. Use a GitHub CLI login for @{} (add one on the GitHub Auth page), or ask an owner of the organization to approve it.",
            what,
            if signin { "GitSwitch" } else { "this login's app" },
            login
        ),
        403 => format!("Couldn't {}: GitHub does not let @{} do that here (it needs more access to this repository).{}", what, login, said_part),
        404 => format!("Couldn't {}: GitHub shows nothing there to @{} — it doesn't exist, or this account can't see it.", what, login),
        301 | 302 | 303 | 307 | 308 => format!("Couldn't {}: GitHub says it was renamed or moved. Nothing was changed. Refresh and try again.", what),
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

    #[test]
    fn new_repository_names_are_checked_before_github_sees_them() {
        for good in ["app", "Subtitle_Maker", "my.site", "a-b_c.d", &"x".repeat(100)] {
            assert!(check_new_repo_name(good).is_ok(), "{}", good);
        }
        for bad in ["", ".", "..", "a b", "a/b", "x.git", "X.GIT", "é", "a?b", &"x".repeat(101)] {
            assert!(check_new_repo_name(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn a_gitswitch_sign_in_is_told_to_sign_in_again_not_to_run_gh() {
        assert!(refusal_as(&reply(401, Value::Null), "x", "me", true).contains("Sign in again on the GitHub Auth page"));
        let mut r = reply(403, Value::Null);
        r.scopes = Some("read:org".into());
        r.accepted = Some("repo".into());
        assert!(scope_missing(&r));
        let t = refusal_as(&r, "create me/app", "me", true);
        assert!(t.contains("lacks the repo scope") && t.contains("GitHub Auth page") && !t.contains("gh auth"), "{}", t);
        let restricted = reply(403, serde_json::json!({"message": "Although you appear to have the correct authorization credentials, the `Acme` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited."}));
        assert!(oauth_restricted(&restricted) && !scope_missing(&restricted));
        let t = refusal_as(&restricted, "create Acme/app", "me", true);
        assert!(t.contains("restricts third-party apps") && t.contains("GitHub CLI login"), "{}", t);
        assert!(!oauth_restricted(&reply(403, serde_json::json!({"message": "Must have admin rights to Repository."}))));
    }
}

/// A local GitHub for unit tests: each connection gets the next canned reply
/// (made knowing its own address, so paging links can point at it), and every
/// request is recorded as it arrived. An empty reply closes the connection
/// unanswered: an answer lost on the way.
#[cfg(test)]
pub(crate) mod fake_github {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    pub(crate) fn github_with(make: impl FnOnce(&str) -> Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let answers = make(&base);
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

    pub(crate) fn github(answers: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        github_with(|_| answers)
    }

    pub(crate) fn reply(status: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut head = format!("HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n", status, body.len());
        for (k, v) in headers {
            head.push_str(&format!("{}: {}\r\n", k, v));
        }
        format!("{}\r\n{}", head, body)
    }

    pub(crate) fn first_line(req: &str) -> String {
        req.lines().next().unwrap_or("").to_string()
    }
}
