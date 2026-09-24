//! "All repositories" — every repo a signed-in GitHub account can reach, with
//! enough local knowledge to clone each one as the right profile.
//!
//! Nothing here is specific to any account or organization: accounts come
//! from the GitHub CLI and GitSwitch sign-ins, and every suggestion (which
//! profile, which SSH link form) is derived from what already exists on disk.

use crate::error::AppError;
use crate::profiles::{self, Profile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 50 pages × 100 = 5,000 repos per account before we stop and say so.
const MAX_PAGES: usize = 50;

/// Overridable so the paging/SSO logic can be tested against a local fake API.
fn api_base() -> String {
    std::env::var("GITSWITCH_GITHUB_API")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://api.github.com".into())
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug, Deserialize)]
struct ApiOwner {
    login: String,
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Debug, Deserialize, Default)]
struct ApiPerms {
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    maintain: bool,
    #[serde(default)]
    push: bool,
}

#[derive(Debug, Deserialize)]
struct ApiRepo {
    name: String,
    full_name: String,
    owner: ApiOwner,
    description: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    fork: bool,
    default_branch: Option<String>,
    ssh_url: String,
    pushed_at: Option<String>,
    permissions: Option<ApiPerms>,
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    login: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct RemoteRepo {
    pub full_name: String,
    pub owner: String,
    pub name: String,
    pub owner_is_org: bool,
    pub description: Option<String>,
    pub private: bool,
    pub archived: bool,
    pub fork: bool,
    pub default_branch: String,
    /// SSH link to clone with (see `clone_link`).
    pub clone_url: String,
    pub pushed_at: Option<String>,
    /// "admin" | "write" | "read"
    pub permission: String,
    /// Already cloned inside a profile folder here.
    pub local_path: Option<String>,
    /// Profile this owner's repos already use on this machine.
    pub suggested_profile_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoListing {
    pub account: String,
    pub login: String,
    /// Profile named after this login, if any — used for the account's OWN
    /// repos when there's no local evidence. Never applied to other owners.
    pub suggested_profile_id: Option<String>,
    pub repos: Vec<RemoteRepo>,
    /// Stopped at MAX_PAGES.
    pub truncated: bool,
    /// Organizations GitHub left out because the token isn't SSO-authorized.
    pub sso_hidden_orgs: usize,
}

/// What is actually on this machine right now. Cheap (local disk only), so it
/// can be re-checked whenever the user returns to the app — folders come and go
/// behind the app's back, while the GitHub listing itself changes rarely.
#[derive(Debug, Serialize)]
pub struct LocalClones {
    /// "owner/name" (lowercase) -> local path
    pub clones: HashMap<String, String>,
    /// owner (lowercase) -> profile its local repos belong to
    pub owner_profiles: HashMap<String, String>,
    /// owner (lowercase) -> "org-<id>" SSH user seen in local remotes
    pub org_ssh_users: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct RepoAccount {
    /// "gh:<account>" or "profile:<profile id>"
    pub key: String,
    pub label: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// The `rel="next"` URL from a GitHub `Link` header.
pub fn next_link(link_header: &str) -> Option<String> {
    link_header.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let url = pieces.next()?.trim();
        let is_next = pieces.any(|p| p.trim() == r#"rel="next""#);
        (is_next && url.starts_with('<') && url.ends_with('>'))
            .then(|| url[1..url.len() - 1].to_string())
    })
}

/// How many organizations `X-GitHub-SSO: partial-results; organizations=1,2`
/// says were left out.
pub fn sso_hidden_org_count(header: &str) -> usize {
    if !header.contains("partial-results") {
        return 0;
    }
    header
        .split(';')
        .find_map(|p| p.trim().strip_prefix("organizations="))
        .map(|ids| ids.split(',').filter(|s| !s.trim().is_empty()).count())
        .unwrap_or(0)
}

fn permission_label(p: Option<&ApiPerms>) -> &'static str {
    match p {
        Some(p) if p.admin => "admin",
        Some(p) if p.maintain || p.push => "write",
        _ => "read",
    }
}

/// `org-<id>@github.com:…` when this machine already clones that owner's repos
/// that way (organizations with SSH certificate authorities), otherwise the
/// standard `git@github.com:…` link GitHub's API returns.
pub fn clone_link(owner: &str, name: &str, api_ssh_url: &str, org_ssh_users: &HashMap<String, String>) -> String {
    match org_ssh_users.get(&owner.to_ascii_lowercase()) {
        Some(user) => format!("{}@github.com:{}/{}.git", user, owner, name),
        None => api_ssh_url.to_string(),
    }
}

/// `org-123` from `org-123@github.com:Owner/repo.git`, with the owner.
fn org_ssh_user(url: &str) -> Option<(String, String)> {
    let (user, rest) = url.trim().split_once('@')?;
    let path = rest.strip_prefix("github.com:")?;
    let owner = path.split('/').next()?;
    let is_org_user = user
        .strip_prefix("org-")
        .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()));
    (is_org_user && !owner.is_empty()).then(|| (owner.to_ascii_lowercase(), user.to_string()))
}

/// Highest count wins; ties go to the lexically smallest id (deterministic).
fn majority(counts: &HashMap<String, usize>) -> Option<String> {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(id, _)| id.clone())
}

// ---------------------------------------------------------------------------
// Local knowledge: what's already cloned, and as which profile
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LocalIndex {
    /// "owner/name" (lowercase) -> local path
    clones: HashMap<String, String>,
    /// owner (lowercase) -> profile id -> number of local repos
    owner_profiles: HashMap<String, HashMap<String, usize>>,
    /// owner (lowercase) -> "org-<id>" SSH user seen in local remotes
    org_ssh_users: HashMap<String, String>,
}

async fn local_index(profs: &[Profile]) -> LocalIndex {
    let mut idx = LocalIndex::default();
    let mut seen = std::collections::HashSet::new();
    for p in profs {
        for dir in &p.directories {
            if !std::path::Path::new(dir).is_dir() {
                continue;
            }
            for r in crate::repo_scan::scan_repos(dir.clone()).await.unwrap_or_default() {
                if !seen.insert(r.path.clone()) || r.owner.is_empty() {
                    continue;
                }
                let key = format!("{}/{}", r.owner, r.name).to_ascii_lowercase();
                idx.clones.entry(key).or_insert_with(|| r.path.clone());
                if let Some(owning) = crate::sparse::resolve_profile(&r.path, profs) {
                    *idx.owner_profiles
                        .entry(r.owner.to_ascii_lowercase())
                        .or_default()
                        .entry(owning.id.clone())
                        .or_default() += 1;
                }
                if let Some((owner, user)) = org_ssh_user(&r.remote_url) {
                    idx.org_ssh_users.entry(owner).or_insert(user);
                }
            }
        }
    }
    idx
}

// ---------------------------------------------------------------------------
// Accounts and listing
// ---------------------------------------------------------------------------

/// Accounts that can list repositories: GitHub CLI logins, plus profiles
/// signed in through GitSwitch's own GitHub sign-in.
pub async fn accounts() -> Vec<RepoAccount> {
    let mut out = Vec::new();
    if let Ok(names) = crate::gh_cli::gh_list_accounts().await {
        for n in names {
            out.push(RepoAccount {
                key: format!("gh:{}", n),
                label: n,
                source: "GitHub CLI".into(),
            });
        }
    }
    if let Ok(store) = profiles::load_profiles() {
        for p in store.profiles {
            if matches!(crate::credentials::get_token(&p.id), Ok(Some(_))) {
                out.push(RepoAccount {
                    key: format!("profile:{}", p.id),
                    label: format!("{} (GitSwitch sign-in)", p.name),
                    source: "GitSwitch sign-in".into(),
                });
            }
        }
    }
    out
}

async fn token_for(account: &str) -> Result<String, AppError> {
    if let Some(name) = account.strip_prefix("gh:") {
        return crate::gh_cli::gh_get_token(name).await;
    }
    if let Some(id) = account.strip_prefix("profile:") {
        return crate::credentials::get_token(id)?
            .ok_or_else(|| AppError::NotFound("This profile has no saved GitHub sign-in.".into()));
    }
    Err(AppError::Config(format!("Unknown account \"{}\"", account)))
}

/// Re-read only the local side of the picture: no GitHub call, no token.
pub async fn local_clones() -> Result<LocalClones, AppError> {
    let store = profiles::load_profiles()?;
    let idx = local_index(&store.profiles).await;
    Ok(LocalClones {
        clones: idx.clones,
        owner_profiles: idx
            .owner_profiles
            .iter()
            .filter_map(|(owner, counts)| majority(counts).map(|p| (owner.clone(), p)))
            .collect(),
        org_ssh_users: idx.org_ssh_users,
    })
}

pub async fn list_repos(account: &str) -> Result<RepoListing, AppError> {
    let token = token_for(account).await?;
    list_repos_with_token(account, &token).await
}

fn api_error(status: u16, what: &str) -> AppError {
    AppError::Command(match status {
        401 => "GitHub rejected this account's token (expired or revoked). Sign in again — e.g. `gh auth login`.".into(),
        403 | 429 => format!("GitHub refused to {} (rate limit, or the token needs more access). Try again in a few minutes.", what),
        s => format!("GitHub returned {} while trying to {}.", s, what),
    })
}

pub async fn list_repos_with_token(account: &str, token: &str) -> Result<RepoListing, AppError> {
    let client = crate::github::http_client()?;
    let base = api_base();
    let get = |url: String| {
        client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "GitSwitch/0.1.0")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
    };

    let me = get(format!("{}/user", base))
        .await
        .map_err(|e| AppError::Command(format!("Couldn't reach GitHub: {}", e)))?;
    if !me.status().is_success() {
        return Err(api_error(me.status().as_u16(), "read this account"));
    }
    let login = me
        .json::<ApiUser>()
        .await
        .map_err(|e| AppError::Command(format!("Unexpected reply from GitHub: {}", e)))?
        .login;

    let mut url = format!(
        "{}/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
        base
    );
    let mut api_repos: Vec<ApiRepo> = Vec::new();
    let (mut pages, mut truncated, mut sso_hidden) = (0usize, false, 0usize);
    loop {
        let resp = get(url.clone())
            .await
            .map_err(|e| AppError::Command(format!("Couldn't reach GitHub: {}", e)))?;
        if !resp.status().is_success() {
            return Err(api_error(resp.status().as_u16(), "list repositories"));
        }
        if let Some(h) = resp.headers().get("x-github-sso").and_then(|v| v.to_str().ok()) {
            sso_hidden = sso_hidden.max(sso_hidden_org_count(h));
        }
        let next = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(next_link);
        let page: Vec<ApiRepo> = resp
            .json()
            .await
            .map_err(|e| AppError::Command(format!("Unexpected reply from GitHub: {}", e)))?;
        api_repos.extend(page);
        pages += 1;
        match next {
            // Never send the token anywhere but the API we started with.
            Some(n) if n.starts_with(&format!("{}/", base)) => {
                if pages >= MAX_PAGES {
                    truncated = true;
                    break;
                }
                url = n;
            }
            _ => break,
        }
    }

    let store = profiles::load_profiles()?;
    let idx = local_index(&store.profiles).await;

    let repos = map_repos(api_repos, &idx);

    // Deliberately NOT "wherever most of this account's repos live": an account
    // that also sees a company's repos would then pull its personal repos
    // toward the company profile.
    let suggested_profile_id = store
        .profiles
        .iter()
        .find(|p| p.git_name.trim().eq_ignore_ascii_case(&login))
        .map(|p| p.id.clone());

    Ok(RepoListing {
        account: account.to_string(),
        login,
        suggested_profile_id,
        repos,
        truncated,
        sso_hidden_orgs: sso_hidden,
    })
}

/// One page of the listing, for a page that paints the first few repositories
/// at once and keeps loading the rest in the background.
#[derive(Debug, Serialize)]
pub struct RepoPage {
    /// `repos` holds THIS page only; `login`/`suggested_profile_id` are filled
    /// on page 1 and empty afterwards (the caller keeps the first page's).
    pub listing: RepoListing,
    pub page: u32,
    pub per_page: u32,
    /// GitHub says there is a next page (and we have not hit MAX_PAGES).
    pub has_more: bool,
}

/// The URL for one page of `/user/repos`, same ordering and affiliation as the
/// full listing so pages compose.
pub fn page_url(base: &str, page: u32, per_page: u32) -> String {
    format!(
        "{}/user/repos?per_page={}&page={}&sort=pushed&affiliation=owner,collaborator,organization_member",
        base,
        per_page.clamp(1, 100),
        page.max(1)
    )
}

pub async fn list_repos_page(account: &str, page: u32, per_page: u32) -> Result<RepoPage, AppError> {
    let token = token_for(account).await?;
    let client = crate::github::http_client()?;
    let base = api_base();
    let get = |url: String| {
        client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "GitSwitch/0.1.0")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
    };

    // The login is only needed once; later pages leave it empty.
    let login = if page <= 1 {
        let me = get(format!("{}/user", base))
            .await
            .map_err(|e| AppError::Command(format!("Couldn't reach GitHub: {}", e)))?;
        if !me.status().is_success() {
            return Err(api_error(me.status().as_u16(), "read this account"));
        }
        me.json::<ApiUser>()
            .await
            .map_err(|e| AppError::Command(format!("Unexpected reply from GitHub: {}", e)))?
            .login
    } else {
        String::new()
    };

    let resp = get(page_url(&base, page, per_page))
        .await
        .map_err(|e| AppError::Command(format!("Couldn't reach GitHub: {}", e)))?;
    if !resp.status().is_success() {
        return Err(api_error(resp.status().as_u16(), "list repositories"));
    }
    let sso_hidden = resp
        .headers()
        .get("x-github-sso")
        .and_then(|v| v.to_str().ok())
        .map(sso_hidden_org_count)
        .unwrap_or(0);
    let next = resp
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .and_then(next_link)
        .filter(|n| n.starts_with(&format!("{}/", base)));
    let api_repos: Vec<ApiRepo> = resp
        .json()
        .await
        .map_err(|e| AppError::Command(format!("Unexpected reply from GitHub: {}", e)))?;
    let truncated = next.is_some() && page as usize >= MAX_PAGES;
    let has_more = next.is_some() && !truncated;

    let store = profiles::load_profiles()?;
    let idx = local_index(&store.profiles).await;
    let repos = map_repos(api_repos, &idx);
    let suggested_profile_id = if page <= 1 {
        store
            .profiles
            .iter()
            .find(|p| p.git_name.trim().eq_ignore_ascii_case(&login))
            .map(|p| p.id.clone())
    } else {
        None
    };
    Ok(RepoPage {
        listing: RepoListing {
            account: account.to_string(),
            login,
            suggested_profile_id,
            repos,
            truncated,
            sso_hidden_orgs: sso_hidden,
        },
        page: page.max(1),
        per_page: per_page.clamp(1, 100),
        has_more,
    })
}

fn map_repos(api_repos: Vec<ApiRepo>, idx: &LocalIndex) -> Vec<RemoteRepo> {
    api_repos
        .into_iter()
        .map(|r| {
            let owner_l = r.owner.login.to_ascii_lowercase();
            let suggested = idx.owner_profiles.get(&owner_l).and_then(majority);
            RemoteRepo {
                clone_url: clone_link(&r.owner.login, &r.name, &r.ssh_url, &idx.org_ssh_users),
                local_path: idx.clones.get(&r.full_name.to_ascii_lowercase()).cloned(),
                permission: permission_label(r.permissions.as_ref()).into(),
                owner_is_org: r.owner.kind.eq_ignore_ascii_case("Organization"),
                default_branch: r.default_branch.unwrap_or_default(),
                suggested_profile_id: suggested,
                full_name: r.full_name,
                owner: r.owner.login,
                name: r.name,
                description: r.description,
                private: r.private,
                archived: r.archived,
                fork: r.fork,
                pushed_at: r.pushed_at,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_urls_compose_with_the_full_listing_and_stay_in_range() {
        assert_eq!(
            page_url("https://api.github.com", 1, 10),
            "https://api.github.com/user/repos?per_page=10&page=1&sort=pushed&affiliation=owner,collaborator,organization_member"
        );
        assert!(page_url("https://api.github.com", 0, 500).contains("per_page=100&page=1&"));
    }

    #[test]
    fn follows_only_the_next_link() {
        let h = r#"<https://api.github.com/user/repos?page=3&per_page=100>; rel="next", <https://api.github.com/user/repos?page=9&per_page=100>; rel="last""#;
        assert_eq!(next_link(h).as_deref(), Some("https://api.github.com/user/repos?page=3&per_page=100"));
        let last_page = r#"<https://api.github.com/user/repos?page=1>; rel="first", <https://api.github.com/user/repos?page=8>; rel="prev""#;
        assert_eq!(next_link(last_page), None);
        assert_eq!(next_link(""), None);
    }

    #[test]
    fn counts_organizations_hidden_by_sso() {
        assert_eq!(sso_hidden_org_count("partial-results; organizations=21955855,20582480"), 2);
        assert_eq!(sso_hidden_org_count("partial-results; organizations=1"), 1);
        assert_eq!(sso_hidden_org_count("required; url=https://github.com/orgs/x/sso?authorization_request=abc"), 0);
        assert_eq!(sso_hidden_org_count(""), 0);
    }

    #[test]
    fn permission_levels() {
        let p = |admin, maintain, push| ApiPerms { admin, maintain, push };
        assert_eq!(permission_label(Some(&p(true, true, true))), "admin");
        assert_eq!(permission_label(Some(&p(false, true, false))), "write");
        assert_eq!(permission_label(Some(&p(false, false, true))), "write");
        assert_eq!(permission_label(Some(&p(false, false, false))), "read");
        assert_eq!(permission_label(None), "read");
    }

    #[test]
    fn clone_link_uses_org_form_only_where_already_used() {
        let mut users = HashMap::new();
        users.insert("acmeorg".to_string(), "org-1234567".to_string());
        assert_eq!(
            clone_link("AcmeOrg", "widgets", "git@github.com:AcmeOrg/widgets.git", &users),
            "org-1234567@github.com:AcmeOrg/widgets.git"
        );
        assert_eq!(
            clone_link("someone", "dotfiles", "git@github.com:someone/dotfiles.git", &users),
            "git@github.com:someone/dotfiles.git"
        );
    }

    #[test]
    fn recognises_org_ssh_users_in_local_remotes() {
        assert_eq!(
            org_ssh_user("org-1234567@github.com:AcmeOrg/widgets.git"),
            Some(("acmeorg".into(), "org-1234567".into()))
        );
        assert_eq!(org_ssh_user("git@github.com:AcmeOrg/widgets.git"), None);
        assert_eq!(org_ssh_user("org-abc@github.com:AcmeOrg/widgets.git"), None);
        assert_eq!(org_ssh_user("https://github.com/AcmeOrg/widgets.git"), None);
        assert_eq!(org_ssh_user("org-1@gitlab.com:AcmeOrg/widgets.git"), None);
    }

    #[test]
    fn majority_is_deterministic() {
        let mut c = HashMap::new();
        c.insert("b".to_string(), 2);
        c.insert("a".to_string(), 2);
        c.insert("z".to_string(), 1);
        assert_eq!(majority(&c).as_deref(), Some("a"));
        assert_eq!(majority(&HashMap::new()), None);
    }

    #[test]
    fn parses_githubs_repo_shape() {
        let json = r#"[{"name":"widgets","full_name":"AcmeOrg/widgets","owner":{"login":"AcmeOrg","id":1,"type":"Organization"},
            "description":null,"private":true,"archived":false,"fork":false,"default_branch":"main",
            "ssh_url":"git@github.com:AcmeOrg/widgets.git","pushed_at":"2026-09-17T10:00:00Z",
            "permissions":{"admin":false,"maintain":false,"push":true,"triage":true,"pull":true},"size":10}]"#;
        let v: Vec<ApiRepo> = serde_json::from_str(json).unwrap();
        assert_eq!(v[0].owner.kind, "Organization");
        assert_eq!(permission_label(v[0].permissions.as_ref()), "write");
    }
}
