//! A repository on GitHub as GitSwitch's accounts see it — which of them can
//! act on it, and as what — and the people it is shared with.

use crate::error::AppError;
use crate::gh_api::{check_new_repo_name, check_slug, check_username, github_message, oauth_restricted, scope_missing, Gh};
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
        out.profile_login = Box::pin(profile_login(path)).await;
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

/// What a key signs in as, by key path, with when github.com said so: only a
/// definite answer is kept (ten minutes), so a dialog asks once, not each time.
fn key_logins() -> &'static std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String)>> {
    static K: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String)>>> = std::sync::OnceLock::new();
    K.get_or_init(Default::default)
}

const KEY_LOGIN_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn cached_key_login(key: &str) -> Option<String> {
    let k = key_logins().lock().ok()?;
    k.get(key).filter(|(at, _)| at.elapsed() < KEY_LOGIN_TTL).map(|(_, l)| l.clone())
}

fn profile_github_login(p: &crate::profiles::Profile) -> Option<String> {
    p.github_login.as_deref().map(|l| l.trim().trim_start_matches('@').to_string()).filter(|l| !l.is_empty())
}

/// The GitHub login a folder's profile pushes as, when known without asking
/// github.com: the profile's own `github_login`, else what its key was
/// found to sign in as lately. The flag: it has a key nobody asked about yet
/// (`profile_login` asks).
pub(crate) async fn profile_login_known(repo_path: &str) -> (Option<String>, bool) {
    let Some(p) = crate::git_status::profile_for_repo(repo_path).await else { return (None, false) };
    if let Some(l) = profile_github_login(&p) {
        return (Some(l), false);
    }
    match p.ssh_key_path.filter(|k| !k.is_empty()) {
        Some(key) => match cached_key_login(&key) {
            Some(l) => (Some(l), false),
            None => (None, true),
        },
        None => (None, false),
    }
}

/// The GitHub login a folder's profile pushes as: the profile's own
/// `github_login`, else the account GitHub says its SSH key signs in as
/// (asked over ssh at most once in ten minutes, and never longer than 25 s).
pub(crate) async fn profile_login(repo_path: &str) -> Option<String> {
    let p = crate::git_status::profile_for_repo(repo_path).await?;
    if let Some(l) = profile_github_login(&p) {
        return Some(l);
    }
    let key = p.ssh_key_path.filter(|k| !k.is_empty())?;
    if let Some(l) = cached_key_login(&key) {
        return Some(l);
    }
    let found = tokio::time::timeout(std::time::Duration::from_secs(25), crate::ssh_keys::resolve_key_account(&key))
        .await
        .ok()?
        .ok()
        .flatten()?;
    if let Ok(mut k) = key_logins().lock() {
        k.insert(key, (std::time::Instant::now(), found.clone()));
    }
    Some(found)
}

/// Somewhere a repository can be created: the account itself, or one of
/// its organizations.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Owner {
    pub login: String,
    /// "user" | "org"
    pub kind: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct Owners {
    pub owners: Vec<Owner>,
    /// Organizations GitHub left out: the token isn't authorized for their single sign-on.
    pub sso_hidden: usize,
}

/// The account itself first, then its organizations (A–Z). Whether an
/// organization lets this member create repositories, GitHub says when asked.
pub async fn owners(gh: &Gh) -> Result<Owners, AppError> {
    let mut out = Owners { owners: vec![Owner { login: gh.login.clone(), kind: "user".into() }], sso_hidden: 0 };
    let mut orgs: Vec<String> = Vec::new();
    for page in 1..=10u32 {
        let r = gh.get("/user/orgs", &[("per_page", "100".to_string()), ("page", page.to_string())]).await?;
        if !r.ok() {
            return Err(gh.refused(&r, "list your organizations"));
        }
        out.sso_hidden = out.sso_hidden.max(r.sso().map(crate::remote_repos::sso_hidden_org_count).unwrap_or(0));
        let rows = r.json.as_array().cloned().unwrap_or_default();
        orgs.extend(rows.iter().filter_map(|o| o.get("login").and_then(|l| l.as_str())).map(str::to_string));
        if !r.has_next || rows.is_empty() {
            break;
        }
    }
    orgs.retain(|l| check_username(l).is_ok() && !l.eq_ignore_ascii_case(&gh.login));
    orgs.sort_by_key(|l| l.to_lowercase());
    orgs.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out.owners.extend(orgs.into_iter().map(|login| Owner { login, kind: "org".into() }));
    Ok(out)
}

/// A repository GitHub just made, as it said.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct CreatedRepo {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub ssh_url: String,
    /// Built from `full_name`: never an address the answer could choose.
    pub html_url: String,
    pub clone_url: String,
    pub private: bool,
    /// GitHub named it differently from what was asked.
    pub renamed: bool,
}

/// A description as a repository shows it: one line, trimmed, at most 350
/// characters; None when nothing is left.
pub(crate) fn one_line_description(d: &str) -> Option<String> {
    let one = d.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = one.chars().take(350).collect();
    (!cut.is_empty()).then_some(cut)
}

/// Create an empty repository (`auto_init: false`, so the history published
/// into it is its only one) under the account itself or one of its
/// organizations. Never sent twice: when the answer is lost on the way,
/// `GET /repos/{owner}/{name}` says whether it was made.
pub async fn create(gh: &Gh, owner: &str, name: &str, description: &str, private: bool) -> Result<CreatedRepo, AppError> {
    check_username(owner)?;
    check_new_repo_name(name)?;
    let mut body = json!({ "name": name, "private": private, "auto_init": false });
    if let Some(d) = one_line_description(description) {
        body["description"] = json!(d);
    }
    let own = owner.eq_ignore_ascii_case(&gh.login);
    let path = if own { "/user/repos".to_string() } else { format!("/orgs/{}/repos", owner) };
    let r = match gh.call(Method::POST, &path, &[], Some(&body)).await {
        Ok(r) => r,
        Err(lost) => return Box::pin(lost_answer(gh, owner, name, &lost.to_string())).await,
    };
    let said = github_message(&r.json);
    match r.status {
        201 => created_from(&r.json, owner, name),
        422 if said.to_lowercase().contains("already exists") => Err(AppError::Command(format!(
            "There's already a repository {}/{} on GitHub. Pick another name, or publish to that one with \"Use an address\" (only if it's empty).",
            owner, name
        ))),
        422 => Err(AppError::Command(format!("GitHub wouldn't create {}/{}: {}", owner, name, said))),
        403 | 404 if !own && r.sso().is_none() && !scope_missing(&r) && !oauth_restricted(&r) => Err(AppError::Command(format!(
            "@{} can't create repositories in {}: the organization lets only some members do that (or there's no such organization).{}",
            gh.login,
            owner,
            if said.is_empty() { String::new() } else { format!(" GitHub said: \"{}\".", said) }
        ))),
        _ => Err(gh.refused(&r, &format!("create {}/{}", owner, name))),
    }
}

/// The request went out but no answer came back, so it may have been made:
/// look, rather than send it again.
async fn lost_answer(gh: &Gh, owner: &str, name: &str, lost: &str) -> Result<CreatedRepo, AppError> {
    match gh.get(&format!("/repos/{}/{}", owner, name), &[]).await {
        Ok(r) if r.ok() && r.json.get("size").and_then(|s| s.as_u64()) == Some(0) => created_from(&r.json, owner, name),
        Ok(r) if r.ok() => Err(AppError::Command(format!(
            "GitHub's answer was lost, and {}/{} exists but isn't empty, so it isn't the one just asked for. Nothing was connected.",
            owner, name
        ))),
        Ok(r) if r.status == 404 => Err(AppError::Command(format!("Couldn't create {}/{}: {}. Nothing was created; try again.", owner, name, lost))),
        _ => Err(AppError::Command(format!(
            "GitHub's answer was lost ({}), and GitSwitch couldn't tell whether {}/{} was created. Look on GitHub before trying again.",
            lost, owner, name
        ))),
    }
}

/// What GitHub made, checked: under the owner asked for, with an SSH address
/// that is that repository's. Nothing else is ever saved as a remote.
fn created_from(v: &Value, owner: &str, name: &str) -> Result<CreatedRepo, AppError> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let full_name = s("full_name");
    let (got_owner, got_name) = full_name.split_once('/').map(|(o, n)| (o.to_string(), n.to_string())).unwrap_or_default();
    let ssh_url = s("ssh_url");
    let ssh_matches = crate::repo_scan::parse_github_owner_name(&ssh_url).is_some_and(|(o, n)| o.eq_ignore_ascii_case(&got_owner) && n.eq_ignore_ascii_case(&got_name));
    if !got_owner.eq_ignore_ascii_case(owner) || check_slug(&got_owner, &got_name).is_err() || !ssh_matches {
        let shown = if full_name.is_empty() { "nothing recognisable".to_string() } else { full_name.clone() };
        return Err(AppError::Command(format!("GitHub answered with a repository GitSwitch didn't ask for ({}), so nothing was connected.", shown)));
    }
    Ok(CreatedRepo {
        html_url: format!("https://github.com/{}", full_name),
        clone_url: format!("https://github.com/{}.git", full_name),
        renamed: !got_name.eq_ignore_ascii_case(name),
        private: v.get("private").and_then(|p| p.as_bool()).unwrap_or(false),
        owner: got_owner,
        name: got_name,
        full_name,
        ssh_url,
    })
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

#[cfg(test)]
mod create_tests {
    use super::*;
    use crate::gh_api::fake_github::{first_line, github, reply};

    fn body(req: &str) -> &str {
        req.split("\r\n\r\n").nth(1).unwrap_or("")
    }

    const MADE: &str = r#"{"full_name":"me/app","ssh_url":"git@github.com:me/app.git","html_url":"https://github.com/me/app","private":true,"size":0}"#;

    #[tokio::test]
    async fn ones_own_repository_is_created_empty_and_private_under_user_repos() {
        let (api, seen) = github(vec![reply("201 Created", &[], MADE)]);
        let made = create(&Gh::at(&api, "t", "me"), "me", "app", "  ", true).await.unwrap();
        assert_eq!(
            (made.full_name.as_str(), made.ssh_url.as_str(), made.html_url.as_str(), made.private, made.renamed),
            ("me/app", "git@github.com:me/app.git", "https://github.com/me/app", true, false)
        );
        let reqs = seen.lock().unwrap().clone();
        assert_eq!(reqs.len(), 1);
        assert!(first_line(&reqs[0]).starts_with("POST /user/repos "), "{}", reqs[0]);
        let b = body(&reqs[0]);
        assert!(b.contains("\"auto_init\":false") && b.contains("\"private\":true") && b.contains("\"name\":\"app\"") && !b.contains("description"), "{}", b);
    }

    #[tokio::test]
    async fn an_organizations_repository_goes_to_its_endpoint_with_a_one_line_description() {
        let made = r#"{"full_name":"Acme/tool","ssh_url":"git@github.com:Acme/tool.git","private":false,"size":0}"#;
        let (api, seen) = github(vec![reply("201 Created", &[], made)]);
        let got = create(&Gh::at(&api, "t", "me"), "Acme", "tool", " A tool\nfor  things ", false).await.unwrap();
        assert_eq!((got.full_name.as_str(), got.private), ("Acme/tool", false));
        let reqs = seen.lock().unwrap().clone();
        assert!(first_line(&reqs[0]).starts_with("POST /orgs/Acme/repos "), "{}", reqs[0]);
        assert!(body(&reqs[0]).contains("\"description\":\"A tool for things\""), "{}", reqs[0]);
    }

    #[tokio::test]
    async fn names_github_would_refuse_never_reach_it() {
        let (api, seen) = github(vec![]);
        let gh = Gh::at(&api, "t", "me");
        for (o, n) in [("me", "a b"), ("me", "app.git"), ("me", ".."), ("a/b", "x"), ("-me", "x"), ("me", "")] {
            assert!(create(&gh, o, n, "", true).await.is_err(), "{o}/{n}");
        }
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refusals_say_what_happened() {
        let (api, _) = github(vec![
            reply("422 Unprocessable Entity", &[], r#"{"message":"Repository creation failed.","errors":[{"resource":"Repository","code":"custom","field":"name","message":"name already exists on this account"}]}"#),
            reply("403 Forbidden", &[], r#"{"message":"Although you appear to have the correct authorization credentials, the `Acme` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited."}"#),
            reply("403 Forbidden", &[], r#"{"message":"You need admin access to the organization before adding a repository to it."}"#),
        ]);
        let mut gh = Gh::at(&api, "t", "me");
        let e = create(&gh, "me", "app", "", true).await.unwrap_err().to_string();
        assert!(e.contains("There's already a repository me/app"), "{e}");
        gh.signin = true;
        let e = create(&gh, "Acme", "app", "", true).await.unwrap_err().to_string();
        assert!(e.contains("restricts third-party apps") && e.contains("GitHub CLI login"), "{e}");
        let e = create(&gh, "Acme", "app", "", true).await.unwrap_err().to_string();
        assert!(e.contains("@me can't create repositories in Acme") && e.contains("admin access"), "{e}");
    }

    #[tokio::test]
    async fn an_answer_naming_another_repository_is_not_trusted() {
        let (api, _) = github(vec![reply("201 Created", &[], r#"{"full_name":"me/app","ssh_url":"git@github.com:evil/app.git","size":0}"#)]);
        let e = create(&Gh::at(&api, "t", "me"), "me", "app", "", true).await.unwrap_err().to_string();
        assert!(e.contains("didn't ask for"), "{e}");
    }

    #[tokio::test]
    async fn a_lost_answer_is_looked_up_never_sent_again() {
        let (api, seen) = github(vec![String::new(), reply("200 OK", &[], MADE)]);
        let made = create(&Gh::at(&api, "t", "me"), "me", "app", "", true).await.unwrap();
        assert_eq!(made.full_name, "me/app");
        let reqs = seen.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2, "{reqs:?}");
        assert!(first_line(&reqs[0]).starts_with("POST /user/repos ") && first_line(&reqs[1]).starts_with("GET /repos/me/app "), "{reqs:?}");
    }

    #[tokio::test]
    async fn owners_are_the_account_then_its_organizations() {
        let (api, seen) = github(vec![reply("200 OK", &[], r#"[{"login":"zeta"},{"login":"Acme"},{"login":"bad/name"}]"#)]);
        let o = owners(&Gh::at(&api, "t", "me")).await.unwrap();
        let names: Vec<(&str, &str)> = o.owners.iter().map(|x| (x.login.as_str(), x.kind.as_str())).collect();
        assert_eq!(names, vec![("me", "user"), ("Acme", "org"), ("zeta", "org")]);
        assert!(first_line(&seen.lock().unwrap()[0]).starts_with("GET /user/orgs?per_page=100&page=1 "));
    }
}
