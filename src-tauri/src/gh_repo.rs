//! A repository on GitHub as GitSwitch's accounts see it — which of them can
//! act on it, and as what — and the people it is shared with.

use crate::error::AppError;
use crate::gh_api::{check_slug, check_username, github_message, Gh};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AccountAccess {
    /// The key commands take: "gh:<login>" or "signin:<login>".
    pub account: String,
    pub login: String,
    pub source: String,
    /// "admin" | "push" | "read" | "none" | "unknown"
    pub permission: String,
    pub private: Option<bool>,
    /// Why GitHub's answer proves nothing, when it does not.
    pub why: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct RepoContext {
    pub owner: Option<String>,
    pub name: Option<String>,
    /// Why there is no GitHub repository to show, when there is none.
    pub not_github: Option<String>,
    pub accounts: Vec<AccountAccess>,
    /// The account to act as: the profile's own login when it can see the
    /// repository, else the one with the most access.
    pub suggested: Option<String>,
    /// The login the repository's profile uses, when it says.
    pub profile_login: Option<String>,
}

fn rank(p: &str) -> u8 {
    match p {
        "admin" => 3,
        "push" => 2,
        "read" => 1,
        _ => 0,
    }
}

/// The account to act as.
pub(crate) fn suggest(accounts: &[AccountAccess], profile_login: Option<&str>) -> Option<String> {
    if let Some(l) = profile_login {
        if let Some(a) = accounts.iter().find(|a| a.login.eq_ignore_ascii_case(l) && rank(&a.permission) > 0) {
            return Some(a.account.clone());
        }
    }
    accounts.iter().filter(|a| rank(&a.permission) > 0).max_by_key(|a| (rank(&a.permission), std::cmp::Reverse(accounts.iter().position(|x| x == *a)))).map(|a| a.account.clone())
}

async fn remote_url(repo_path: &str) -> Option<String> {
    use crate::git_exec::GitCmd;
    if let Some(u) = GitCmd::at(repo_path).args(["remote", "get-url", "origin"]).ok_text().await {
        return Some(u.trim().to_string());
    }
    let first = GitCmd::at(repo_path).args(["remote"]).ok_text().await?.lines().next()?.trim().to_string();
    GitCmd::at(repo_path).args(["remote", "get-url", &first]).ok_text().await.map(|u| u.trim().to_string())
}

/// The GitHub repository behind a local clone (or named as "owner/name"),
/// and what each account GitSwitch knows may do there.
pub async fn context(repo_path: Option<String>, repo: Option<String>) -> Result<RepoContext, AppError> {
    let mut out = RepoContext::default();
    let (owner, name) = if let Some(r) = repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        let (o, n) = r.split_once('/').ok_or_else(|| AppError::Config(format!("\"{}\" is not owner/name", r)))?;
        (o.to_string(), n.to_string())
    } else if let Some(path) = repo_path.as_deref() {
        let Some(url) = remote_url(path).await else {
            out.not_github = Some("This repository has no remote.".into());
            return Ok(out);
        };
        match crate::peek::github_slug_resolved(&url).await {
            Some(s) => s,
            None => {
                out.not_github = Some(format!("Its remote ({}) is not on GitHub.", url));
                return Ok(out);
            }
        }
    } else {
        return Err(AppError::Config("Which repository?".into()));
    };
    check_slug(&owner, &name)?;

    if let Some(path) = repo_path.as_deref() {
        if let Some(p) = crate::git_status::profile_for_repo(path).await {
            out.profile_login = p.github_login.map(|l| l.trim().trim_start_matches('@').to_string()).filter(|l| !l.is_empty());
            if let (None, Some(key)) = (&out.profile_login, p.ssh_key_path.as_deref().filter(|k| !k.is_empty())) {
                out.profile_login = crate::ssh_keys::resolve_key_account(key).await.ok().flatten();
            }
        }
    }

    // One row per login: a GitHub CLI login and a sign-in of the same account are one.
    let mut seen = std::collections::HashSet::new();
    let accounts: Vec<_> = crate::remote_repos::accounts().await.into_iter().filter(|a| seen.insert(crate::gh_api::login_of(&a.key).to_lowercase())).collect();
    let mut set = tokio::task::JoinSet::new();
    for (i, a) in accounts.into_iter().enumerate() {
        let (o, n) = (owner.clone(), name.clone());
        set.spawn(async move {
            let row = |permission: &str, private: Option<bool>, why: Option<String>| AccountAccess {
                account: a.key.clone(),
                login: crate::gh_api::login_of(&a.key).to_string(),
                source: a.source.clone(),
                permission: permission.into(),
                private,
                why,
            };
            let r = match crate::remote_repos::token_for(&a.key).await {
                Err(e) => row("unknown", None, Some(e.to_string())),
                Ok(token) => match crate::github::repo_access_detail(&token, &o, &n).await {
                    Ok((crate::github::RepoAccess::Visible(p), private)) => row(if p.admin { "admin" } else if p.push { "push" } else { "read" }, private, None),
                    Ok((crate::github::RepoAccess::Invisible, _)) => row("none", None, None),
                    Ok((crate::github::RepoAccess::Unknown(why), _)) => row("unknown", None, Some(why)),
                    Err(e) => row("unknown", None, Some(e.to_string())),
                },
            };
            (i, r)
        });
    }
    let mut rows = Vec::new();
    while let Some(j) = set.join_next().await {
        if let Ok(r) = j {
            rows.push(r);
        }
    }
    rows.sort_by_key(|(i, _)| *i);
    out.accounts = rows.into_iter().map(|(_, r)| r).collect();
    out.suggested = suggest(&out.accounts, out.profile_login.as_deref());
    out.owner = Some(owner);
    out.name = Some(name);
    Ok(out)
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Collaborator {
    pub login: String,
    /// GitHub's role: admin, maintain, write, triage, read (or a custom one).
    pub role: String,
    pub is_owner: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Invitation {
    pub id: u64,
    pub login: String,
    pub role: String,
    pub created_at: String,
    pub inviter: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct People {
    pub collaborators: Vec<Collaborator>,
    /// None when this account may not see them (only admins can).
    pub invitations: Option<Vec<Invitation>>,
    pub note: Option<String>,
}

pub(crate) fn role_of(v: &Value) -> String {
    if let Some(r) = v.get("role_name").and_then(|r| r.as_str()).filter(|r| !r.is_empty()) {
        return r.to_string();
    }
    let p = v.get("permissions");
    let f = |k: &str| p.and_then(|p| p.get(k)).and_then(|x| x.as_bool()).unwrap_or(false);
    if f("admin") {
        "admin"
    } else if f("maintain") {
        "maintain"
    } else if f("push") {
        "write"
    } else if f("triage") {
        "triage"
    } else {
        "read"
    }
    .into()
}

/// Invitation permissions are spelled differently from roles ("write" is "push" there…).
fn invite_role(p: &str) -> String {
    match p {
        "write" => "write".into(),
        "push" => "write".into(),
        "pull" | "read" => "read".into(),
        other => other.to_string(),
    }
}

pub async fn people(gh: &Gh, owner: &str, name: &str) -> Result<People, AppError> {
    check_slug(owner, name)?;
    let base = format!("/repos/{}/{}", owner, name);
    let mut collaborators = Vec::new();
    for page in 1..=10u32 {
        let r = gh.get(&format!("{}/collaborators", base), &[("affiliation", "direct".into()), ("per_page", "100".into()), ("page", page.to_string())]).await?;
        if !r.ok() {
            return Err(gh.refused(&r, &format!("list who {}/{} is shared with", owner, name)));
        }
        for v in r.json.as_array().into_iter().flatten() {
            let login = v.get("login").and_then(|l| l.as_str()).unwrap_or("").to_string();
            collaborators.push(Collaborator { is_owner: login.eq_ignore_ascii_case(owner), role: role_of(v), login });
        }
        if !r.has_next {
            break;
        }
    }
    let r = gh.get(&format!("{}/invitations", base), &[("per_page", "100".into())]).await?;
    let (invitations, note) = if r.ok() {
        let list = r
            .json
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| Invitation {
                id: v.get("id").and_then(|i| i.as_u64()).unwrap_or(0),
                login: v.get("invitee").and_then(|i| i.get("login")).and_then(|l| l.as_str()).unwrap_or("").to_string(),
                role: invite_role(v.get("permissions").and_then(|p| p.as_str()).unwrap_or("")),
                created_at: v.get("created_at").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                inviter: v.get("inviter").and_then(|i| i.get("login")).and_then(|l| l.as_str()).map(str::to_string),
            })
            .collect();
        (Some(list), None)
    } else if r.status == 403 || r.status == 404 {
        (None, Some(format!("Only admins of {}/{} see pending invitations.", owner, name)))
    } else {
        (None, Some(gh.refused(&r, "read the pending invitations").to_string()))
    };
    Ok(People { collaborators, invitations, note })
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AddOutcome {
    /// "invited" (they must accept) | "added" (they had access, or joined at once)
    pub status: String,
    pub message: String,
    pub invitation_id: Option<u64>,
}

const ROLES: [&str; 5] = ["pull", "triage", "push", "maintain", "admin"];

/// Invite someone. A personal repository has one role for collaborators
/// (write), so the role asked for applies to organization repositories only.
pub async fn add(gh: &Gh, owner: &str, name: &str, username: &str, permission: &str) -> Result<AddOutcome, AppError> {
    check_slug(owner, name)?;
    let username = username.trim().trim_start_matches('@');
    check_username(username)?;
    if !ROLES.contains(&permission) {
        return Err(AppError::Config(format!("Unknown role \"{}\"", permission)));
    }
    let who = gh.get(&format!("/users/{}", username), &[]).await?;
    if who.status == 404 {
        return Err(AppError::Command(format!("There is no GitHub user called {}.", username)));
    }
    let shown = who.json.get("login").and_then(|l| l.as_str()).unwrap_or(username).to_string();
    let r = gh.call(Method::PUT, &format!("/repos/{}/{}/collaborators/{}", owner, name, username), &[], Some(&json!({ "permission": permission }))).await?;
    match r.status {
        201 => Ok(AddOutcome {
            status: "invited".into(),
            message: format!("Invited {} — they get access once they accept the invitation GitHub sends them.", shown),
            invitation_id: r.json.get("id").and_then(|i| i.as_u64()),
        }),
        204 => Ok(AddOutcome { status: "added".into(), message: format!("{} has access now.", shown), invitation_id: None }),
        422 => Err(AppError::Command(format!("GitHub would not invite {}: {}", shown, github_message(&r.json)))),
        _ => Err(gh.refused(&r, &format!("invite {}", shown))),
    }
}

pub async fn remove(gh: &Gh, owner: &str, name: &str, username: &str) -> Result<(), AppError> {
    check_slug(owner, name)?;
    check_username(username)?;
    let r = gh.call(Method::DELETE, &format!("/repos/{}/{}/collaborators/{}", owner, name, username), &[], None).await?;
    if r.ok() {
        Ok(())
    } else {
        Err(gh.refused(&r, &format!("remove {}", username)))
    }
}

pub async fn cancel_invitation(gh: &Gh, owner: &str, name: &str, id: u64) -> Result<(), AppError> {
    check_slug(owner, name)?;
    let r = gh.call(Method::DELETE, &format!("/repos/{}/{}/invitations/{}", owner, name, id), &[], None).await?;
    if r.ok() {
        Ok(())
    } else {
        Err(gh.refused(&r, "cancel the invitation"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(account: &str, permission: &str) -> AccountAccess {
        AccountAccess { account: account.into(), login: crate::gh_api::login_of(account).into(), source: "GitHub CLI".into(), permission: permission.into(), private: None, why: None }
    }

    #[test]
    fn the_profiles_own_login_acts_when_it_can_see_the_repository() {
        let list = vec![acc("gh:work", "admin"), acc("gh:Me", "read"), acc("gh:other", "none")];
        assert_eq!(suggest(&list, Some("me")).as_deref(), Some("gh:Me"));
        assert_eq!(suggest(&list, Some("other")).as_deref(), Some("gh:work"));
        assert_eq!(suggest(&list, None).as_deref(), Some("gh:work"));
        let tie = vec![acc("gh:a", "push"), acc("gh:b", "push")];
        assert_eq!(suggest(&tie, None).as_deref(), Some("gh:a"));
        assert_eq!(suggest(&[acc("gh:x", "none"), acc("gh:y", "unknown")], None), None);
    }

    #[test]
    fn roles_read_from_role_name_or_permissions() {
        assert_eq!(role_of(&json!({"role_name": "maintain"})), "maintain");
        assert_eq!(role_of(&json!({"permissions": {"admin": false, "push": true, "pull": true}})), "write");
        assert_eq!(role_of(&json!({"permissions": {"pull": true}})), "read");
        assert_eq!(invite_role("push"), "write");
        assert_eq!(invite_role("triage"), "triage");
    }
}
