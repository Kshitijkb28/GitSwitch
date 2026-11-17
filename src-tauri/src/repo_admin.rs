//! Who can see a repository on GitHub, and deleting one, as one of GitSwitch's
//! accounts. Both act only on the repository the dialog showed: its id is
//! checked first, writes never follow a redirect (a renamed or transferred
//! repository is refused with a word, not changed under its new name), and
//! nothing is retried by itself.
//!
//! Deleting needs GitHub's `delete_repo` scope, which no sign-in asks for by
//! default (a leaked token could otherwise delete everything). GitSwitch asks
//! GitHub for it only when the person deletes: a device-code approval for the
//! same login, whose token is kept in memory for ten minutes, never stored,
//! and never handed to the window.

use crate::error::AppError;
use crate::gh_api::{check_slug, check_username, github_message, refusal_as, Gh, Reply};
use reqwest::Method;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the dialogs show before anything changes: the repository as GitHub
/// has it now, with the numbers a change would cost.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct RepoFacts {
    pub id: u64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    /// "user" | "org"
    pub owner_kind: String,
    pub private: bool,
    /// "public" | "private" | "internal"
    pub visibility: String,
    pub fork: bool,
    /// "owner/name" of the repository it was forked from.
    pub parent: Option<String>,
    pub archived: bool,
    /// This account may change its settings (who can see it, deleting it).
    pub admin: bool,
    pub stars: u64,
    pub watchers: u64,
    pub forks: u64,
    pub has_pages: bool,
    pub html_url: String,
    /// Whether this account can delete it now: true with the delete_repo
    /// scope (or an approval kept from a moment ago), false when GitHub
    /// listed the token's scopes without it, None when GitHub didn't say.
    pub can_delete: Option<bool>,
    /// Branches on GitHub whose last commit isn't in the folder on this
    /// computer (when one was named): deleting would lose them.
    pub missing_here: Vec<String>,
    /// A folder on this computer was named and read.
    pub local_checked: bool,
    /// GitHub has more branches than were compared with that folder (the
    /// first 1,000): the others weren't checked.
    pub branches_capped: bool,
}

fn facts_from(r: &Reply) -> RepoFacts {
    let j = &r.json;
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let n = |k: &str| j.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let b = |k: &str| j.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let full_name = s("full_name");
    let (owner, name) = full_name.split_once('/').map(|(o, n)| (o.to_string(), n.to_string())).unwrap_or_default();
    let private = b("private");
    RepoFacts {
        id: n("id"),
        owner_kind: if j.pointer("/owner/type").and_then(|v| v.as_str()) == Some("Organization") { "org" } else { "user" }.to_string(),
        visibility: j
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| if private { "private" } else { "public" }.to_string()),
        fork: b("fork"),
        parent: j.pointer("/parent/full_name").and_then(|v| v.as_str()).map(str::to_string),
        archived: b("archived"),
        admin: j.pointer("/permissions/admin").and_then(|v| v.as_bool()).unwrap_or(false),
        stars: n("stargazers_count"),
        watchers: n("subscribers_count"),
        forks: n("forks_count"),
        has_pages: b("has_pages"),
        html_url: format!("https://github.com/{}", full_name),
        can_delete: r.has_scope("delete_repo"),
        private,
        full_name,
        owner,
        name,
        missing_here: Vec::new(),
        local_checked: false,
        branches_capped: false,
    }
}

/// `owner/name` as GitHub has it now, and refused unless it is still the
/// repository with id `expect_id` under that same name (0: any id, for the
/// dialog's first read).
async fn same_repo(gh: &Gh, owner: &str, name: &str, expect_id: u64, what: &str) -> Result<RepoFacts, AppError> {
    let asked = format!("{}/{}", owner, name);
    let r = gh.get(&format!("/repos/{}/{}", owner, name), &[]).await?;
    if !r.ok() {
        return Err(gh.refused(&r, &format!("{} {}", what, asked)));
    }
    let f = facts_from(&r);
    if !f.full_name.eq_ignore_ascii_case(&asked) {
        return Err(AppError::Command(format!(
            "{} was renamed or moved on GitHub: it is {} now. Nothing was changed. Refresh and try again.",
            asked, f.full_name
        )));
    }
    if expect_id != 0 && f.id != expect_id {
        return Err(AppError::Command(format!(
            "{} on GitHub is no longer the repository this page showed (it was deleted and made again). Nothing was changed. Refresh and try again.",
            asked
        )));
    }
    Ok(f)
}

/// Branch pages compared with the folder: 10 of 100.
const BRANCH_PAGES: u32 = 10;

/// The repository as GitHub has it now. With `local`, also which of its
/// branches the folder on this computer lacks the last commit of: every
/// branch up to the first 1,000 (`branches_capped` says when there were
/// more), all checked by one git. A page GitHub doesn't give, or a folder git
/// can't read, leaves `local_checked` false: nothing is claimed.
pub async fn facts(gh: &Gh, owner: &str, name: &str, local: Option<&str>) -> Result<RepoFacts, AppError> {
    check_slug(owner, name)?;
    let mut f = same_repo(gh, owner, name, 0, "read").await?;
    let Some(repo) = local.filter(|p| !p.trim().is_empty()) else {
        return Ok(f);
    };
    let mut tips: Vec<(String, String)> = Vec::new();
    let mut page = 1u32;
    loop {
        let r = gh
            .get(&format!("/repos/{}/{}/branches", owner, name), &[("per_page", "100".to_string()), ("page", page.to_string())])
            .await?;
        if !r.ok() {
            return Ok(f);
        }
        tips.extend(r.json.as_array().into_iter().flatten().filter_map(|b| {
            let name = b.get("name")?.as_str()?.to_string();
            let sha = b.pointer("/commit/sha")?.as_str()?.to_ascii_lowercase();
            (matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit())).then_some((name, sha))
        }));
        if !r.has_next {
            break;
        }
        if page >= BRANCH_PAGES {
            f.branches_capped = true;
            break;
        }
        page += 1;
    }
    // "<sha> commit <size>" for each tip the folder has, "<sha> missing" for
    // the others. No fetch: a partial clone isn't asked to fill one in.
    let input: String = tips.iter().map(|(_, sha)| format!("{}\n", sha)).collect();
    let Ok(out) = crate::git_exec::GitCmd::at(repo)
        .args(["cat-file", "--batch-check"])
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin_bytes(input.into_bytes())
        .run()
        .await
    else {
        return Ok(f);
    };
    if !out.ok() {
        return Ok(f);
    }
    let have: std::collections::HashSet<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            let sha = p.next()?;
            (p.next()? == "commit").then(|| sha.to_ascii_lowercase())
        })
        .collect();
    f.missing_here = tips.into_iter().filter(|(_, sha)| !have.contains(sha)).map(|(b, _)| b).collect();
    f.local_checked = true;
    Ok(f)
}

/// Make it private or public. Only what was asked is sent (`visibility`);
/// GitHub's answer is read back.
pub async fn set_visibility(gh: &Gh, owner: &str, name: &str, private: bool, expect_id: u64) -> Result<RepoFacts, AppError> {
    check_slug(owner, name)?;
    let want = if private { "private" } else { "public" };
    let now = same_repo(gh, owner, name, expect_id, &format!("make {}", want)).await?;
    if now.private == private && now.visibility != "internal" {
        return Ok(now);
    }
    if now.archived {
        return Err(AppError::Command(format!(
            "{} is archived on GitHub, so its settings can't change. Unarchive it on GitHub first.",
            now.full_name
        )));
    }
    if now.fork {
        // GitHub: "A fork's visibility depends on the upstream repository. You
        // cannot change the visibility of a fork by itself." Either way. Nor
        // does it follow its parent later: a parent made private or public
        // leaves its forks as they are, split off.
        return Err(AppError::Command(match now.parent.as_deref() {
            Some(parent) => format!(
                "{} is a fork of {}, and GitHub doesn't let a fork's visibility change. Nothing was changed.",
                now.full_name, parent
            ),
            None => format!("{} is a fork, and GitHub doesn't let a fork's visibility change. Nothing was changed.", now.full_name),
        }));
    }
    let r = gh.call(Method::PATCH, &format!("/repos/{}/{}", owner, name), &[], Some(&json!({ "visibility": want }))).await?;
    if r.ok() {
        let after = facts_from(&r);
        if after.private != private {
            return Err(AppError::Command(format!("GitHub answered, but {} is still {}.", now.full_name, after.visibility)));
        }
        return Ok(RepoFacts { can_delete: now.can_delete, ..after });
    }
    Err(AppError::Command(visibility_refusal(gh, &r, &now, want)))
}

fn moved(r: &Reply) -> bool {
    matches!(r.status, 301 | 302 | 303 | 307 | 308)
}

fn visibility_refusal(gh: &Gh, r: &Reply, now: &RepoFacts, want: &str) -> String {
    let said = github_message(&r.json);
    let said_part = if said.is_empty() { String::new() } else { format!(" GitHub said: \"{}\".", said) };
    if moved(r) {
        return format!("{} was renamed or moved on GitHub. Nothing was changed. Refresh and try again.", now.full_name);
    }
    if r.status == 422 && now.owner_kind == "org" {
        return format!(
            "{} lets only its owners change who can see its repositories, so {} stays {}. Ask an owner of {}.{}",
            now.owner, now.full_name, now.visibility, now.owner, said_part
        );
    }
    if r.status == 422 {
        return format!("GitHub would not make {} {}.{}", now.full_name, want, said_part);
    }
    refusal_as(r, &format!("make {} {}", now.full_name, want), &gh.login, gh.signin)
}

/// What deleting did, when it wasn't refused outright.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct DeleteOutcome {
    /// "deleted" | "needs-permission" (GitHub wants the person's approval
    /// first: nothing was sent)
    pub outcome: String,
    pub message: String,
    /// Deleted: the page on github.com where it can be restored (its owner's
    /// Deleted repositories), read from the same answer as `message`. None
    /// when it can't be restored from settings (a fork network), or when
    /// nothing was deleted.
    pub restore_url: Option<String>,
}

/// Delete it on GitHub. Nothing is sent unless `confirm` is its owner/name
/// (letter case and spaces around it aside) and it is still the repository
/// the dialog showed; nothing is sent either when the token is known to lack
/// the delete_repo scope (the person approves that first).
pub async fn delete(gh: &Gh, owner: &str, name: &str, confirm: &str, expect_id: u64) -> Result<DeleteOutcome, AppError> {
    check_slug(owner, name)?;
    let asked = format!("{}/{}", owner, name);
    if !confirm.trim().eq_ignore_ascii_case(&asked) {
        return Err(AppError::Config(format!("Nothing was deleted: type {} to confirm.", asked)));
    }
    let now = same_repo(gh, owner, name, expect_id, "delete").await?;
    if now.can_delete == Some(false) {
        return Ok(needs_permission(&gh.login));
    }
    let r = gh.call(Method::DELETE, &format!("/repos/{}/{}", owner, name), &[], None).await?;
    if r.status == 204 || r.ok() {
        // A personal repository is restored by its owner (the only admin it
        // has), an organization's by an owner of the organization.
        // Where, from the same facts: the note shown after it never contradicts the message.
        let (restore, restore_url) = if now.fork || now.forks > 0 {
            ("It can't be restored from settings (it was part of a fork network): only GitHub Support can bring it back, on paid plans.".to_string(), None)
        } else if now.owner_kind == "org" {
            (
                format!(
                    "Changed your mind? GitHub keeps it for 90 days: an owner of {} can restore it from {}'s settings › Deleted repositories (it can take up to an hour to show there).",
                    now.owner, now.owner
                ),
                Some(format!("https://github.com/organizations/{}/settings/deleted_repositories", now.owner)),
            )
        } else {
            (
                format!(
                    "Changed your mind? GitHub keeps it for 90 days: restore it from @{}'s settings on GitHub › Repositories › Deleted repositories (it can take up to an hour to show there).",
                    now.owner
                ),
                Some("https://github.com/settings/deleted_repositories".to_string()),
            )
        };
        return Ok(DeleteOutcome { outcome: "deleted".into(), message: format!("Deleted {} on GitHub. {}", now.full_name, restore), restore_url });
    }
    if matches!(r.status, 403 | 404) && r.has_scope("delete_repo") == Some(false) {
        return Ok(needs_permission(&gh.login));
    }
    let said = github_message(&r.json);
    if moved(&r) {
        return Err(AppError::Command(format!("{} was renamed or moved on GitHub. Nothing was deleted. Refresh and try again.", now.full_name)));
    }
    if r.status == 403 && said.to_lowercase().contains("cannot delete repositories") {
        return Err(AppError::Command(format!(
            "{} lets only its owners delete its repositories, so {} was not deleted. Ask an owner of {}.",
            now.owner, now.full_name, now.owner
        )));
    }
    if r.status == 403 && !now.admin {
        return Err(AppError::Command(format!(
            "@{} isn't an admin of {}, and only admins can delete a repository. Nothing was deleted.",
            gh.login, now.full_name
        )));
    }
    Err(AppError::Command(refusal_as(&r, &format!("delete {}", now.full_name), &gh.login, gh.signin)))
}

fn needs_permission(login: &str) -> DeleteOutcome {
    DeleteOutcome {
        outcome: "needs-permission".into(),
        message: format!(
            "GitHub asks for your approval before an app can delete repositories as @{}. Approve it once: GitSwitch keeps the permission in memory for 10 minutes and never saves it. Nothing was deleted.",
            login
        ),
        restore_url: None,
    }
}

// ---- The approval to delete: a device code for the same login ----

/// Exactly what the approval asks GitHub for: reading private repositories
/// (to see the one to delete) and deleting.
const GRANT_SCOPE: &str = "repo delete_repo";
const GRANT_TTL: Duration = Duration::from_secs(10 * 60);

/// What the dialog shows while GitHub waits for the person's approval.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GrantPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

struct GrantJob {
    ticket: u64,
    login: String,
    device_code: String,
    interval: u64,
    until: Instant,
}

fn job() -> &'static Mutex<Option<GrantJob>> {
    static J: OnceLock<Mutex<Option<GrantJob>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(None))
}

static TICKETS: AtomicU64 = AtomicU64::new(0);

/// Approvals kept: lowercased login → (when, token). Memory only.
fn grants() -> &'static Mutex<HashMap<String, (Instant, String)>> {
    static G: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();
    G.get_or_init(Default::default)
}

/// The token an approval for `login` gave, while it is fresh.
pub(crate) fn granted_token(login: &str) -> Option<String> {
    let mut g = grants().lock().ok()?;
    g.retain(|_, (at, _)| at.elapsed() < GRANT_TTL);
    g.get(&login.to_lowercase()).map(|(_, t)| t.clone())
}

/// The account to act as: an approval kept for its login, else its own token.
pub(crate) async fn gh_for(account: &str) -> Result<Gh, AppError> {
    let login = crate::gh_api::login_of(account);
    match granted_token(login) {
        Some(t) => {
            let mut gh = Gh::at(&crate::remote_repos::api_base(), &t, login);
            gh.signin = account.starts_with("signin:");
            Ok(gh)
        }
        None => Gh::for_account(account).await,
    }
}

/// Ask GitHub for a code the person enters on github.com to approve deleting
/// as `login`. A new request replaces one still waiting.
pub async fn grant_start(login: &str) -> Result<GrantPrompt, AppError> {
    check_username(login)?;
    let d = crate::oauth::request_device_code_scoped(GRANT_SCOPE).await?;
    let ticket = TICKETS.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut j) = job().lock() {
        *j = Some(GrantJob {
            ticket,
            login: login.to_string(),
            device_code: d.device_code,
            interval: d.interval.max(5),
            until: Instant::now() + Duration::from_secs(d.expires_in.max(60)),
        });
    }
    Ok(GrantPrompt { user_code: d.user_code, verification_uri: d.verification_uri, expires_in: d.expires_in })
}

/// Stop waiting for the approval.
pub fn grant_cancel() {
    if let Ok(mut j) = job().lock() {
        *j = None;
    }
}

fn inner(e: AppError) -> String {
    match e {
        AppError::Command(s) => s,
        other => other.to_string(),
    }
}

/// Wait for the person to approve on github.com. The token is kept only when
/// it carries delete_repo and GitHub says it belongs to the login asked for.
pub async fn grant_wait() -> Result<String, AppError> {
    let (ticket, login, device_code, mut interval, until) = {
        let j = job().lock().map_err(|_| AppError::Command("The approval could not be read.".into()))?;
        let j = j.as_ref().ok_or_else(|| AppError::Command("No approval is being waited for.".into()))?;
        (j.ticket, j.login.clone(), j.device_code.clone(), j.interval, j.until)
    };
    let current = |t: u64| job().lock().ok().and_then(|j| j.as_ref().map(|j| j.ticket)) == Some(t);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if !current(ticket) {
            return Err(AppError::Command("Stopped waiting for GitHub. Nothing was deleted.".into()));
        }
        if Instant::now() > until {
            grant_cancel();
            return Err(AppError::Command("The code expired before it was approved on GitHub. Nothing was deleted: start again.".into()));
        }
        let token = match crate::oauth::poll_for_token_web(&device_code).await {
            Ok(t) => t,
            Err(e) => {
                let s = inner(e);
                if s == "authorization_pending" {
                    continue;
                }
                if let Some(n) = s.strip_prefix("slow_down interval=") {
                    interval = n.trim().parse().unwrap_or(interval + 5);
                    continue;
                }
                if s == "slow_down" {
                    interval += 5;
                    continue;
                }
                grant_cancel();
                return Err(AppError::Command(match s.as_str() {
                    "access_denied" => "The approval was declined on GitHub. Nothing was deleted.".to_string(),
                    "expired_token" => "The code expired before it was approved on GitHub. Nothing was deleted: start again.".to_string(),
                    _ => format!("GitHub didn't give the approval: {}. Nothing was deleted.", s),
                }));
            }
        };
        if !current(ticket) {
            return Err(AppError::Command("Stopped waiting for GitHub. Nothing was deleted.".into()));
        }
        grant_cancel();
        if !token.scope.split([',', ' ']).any(|s| s.trim() == "delete_repo") {
            return Err(AppError::Command(
                "GitHub gave the approval without the permission to delete repositories. Nothing was deleted, and it was not kept.".into(),
            ));
        }
        let gh = Gh::at(&crate::remote_repos::api_base(), &token.access_token, &login);
        let me = gh.get("/user", &[]).await?;
        let got = me.json.get("login").and_then(|l| l.as_str()).unwrap_or("").to_string();
        if !me.ok() || !got.eq_ignore_ascii_case(&login) {
            let who = if got.is_empty() { "another account".to_string() } else { format!("@{}", got) };
            return Err(AppError::Command(format!(
                "That was approved as {}, not @{}. Nothing was deleted, and the approval was not kept.",
                who, login
            )));
        }
        if let Ok(mut g) = grants().lock() {
            g.insert(login.to_lowercase(), (Instant::now(), token.access_token));
        }
        return Ok(format!("Approved: GitSwitch can delete repositories as @{} for the next 10 minutes.", login));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh_api::fake_github::{first_line, github, reply};

    fn repo_json(full: &str, id: u64, private: bool, extra: &str) -> String {
        let owner_type = if full.starts_with("acme/") { "Organization" } else { "User" };
        format!(
            r#"{{"id":{},"full_name":"{}","private":{},"visibility":"{}","fork":false,"archived":false,"owner":{{"type":"{}"}},"permissions":{{"admin":true}},"stargazers_count":3,"subscribers_count":2,"forks_count":0,"has_pages":false{}}}"#,
            id,
            full,
            private,
            if private { "private" } else { "public" },
            owner_type,
            extra
        )
    }

    fn body(req: &str) -> &str {
        req.split("\r\n\r\n").nth(1).unwrap_or("")
    }

    #[tokio::test]
    async fn making_it_public_sends_only_the_visibility_and_reads_the_answer_back() {
        let (api, seen) = github(vec![
            reply("200 OK", &[("x-oauth-scopes", "repo, read:org")], &repo_json("me/app", 7, true, "")),
            reply("200 OK", &[], &repo_json("me/app", 7, false, "")),
        ]);
        let f = set_visibility(&Gh::at(&api, "t", "me"), "me", "app", false, 7).await.unwrap();
        assert!(!f.private && f.visibility == "public");
        let reqs = seen.lock().unwrap().clone();
        assert!(first_line(&reqs[0]).starts_with("GET /repos/me/app "));
        assert!(first_line(&reqs[1]).starts_with("PATCH /repos/me/app "), "{}", reqs[1]);
        assert_eq!(body(&reqs[1]), r#"{"visibility":"public"}"#);
    }

    #[tokio::test]
    async fn a_fork_never_changes_visibility_either_way() {
        let fork = |private: bool| repo_json("me/app", 7, private, r#","parent":{"full_name":"up/app"}"#).replace(r#""fork":false"#, r#""fork":true"#);
        for private in [true, false] {
            let (api, seen) = github(vec![reply("200 OK", &[], &fork(!private))]);
            let e = set_visibility(&Gh::at(&api, "t", "me"), "me", "app", private, 7).await.unwrap_err().to_string();
            assert!(e.contains("is a fork of up/app, and GitHub doesn't let a fork's visibility change") && e.contains("Nothing was changed"), "{e}");
            assert!(!e.contains("follows"), "a fork doesn't follow its parent: {e}");
            assert_eq!(seen.lock().unwrap().len(), 1, "no PATCH");
        }
    }

    #[tokio::test]
    async fn another_repository_under_the_same_name_is_never_changed() {
        let (api, seen) = github(vec![reply("200 OK", &[], &repo_json("me/app", 99, true, ""))]);
        let e = set_visibility(&Gh::at(&api, "t", "me"), "me", "app", false, 7).await.unwrap_err().to_string();
        assert!(e.contains("no longer the repository"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 1, "no PATCH");
    }

    #[tokio::test]
    async fn a_renamed_repository_is_refused_not_followed() {
        let (api, seen) = github(vec![reply("200 OK", &[], &repo_json("me/app-new", 7, true, ""))]);
        let e = set_visibility(&Gh::at(&api, "t", "me"), "me", "app", false, 7).await.unwrap_err().to_string();
        assert!(e.contains("renamed or moved") && e.contains("me/app-new"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_write_answered_with_a_redirect_is_not_followed() {
        let (api, seen) = crate::gh_api::fake_github::github_with(|base| {
            vec![
                reply("200 OK", &[], &repo_json("me/app", 7, true, "")),
                reply("307 Temporary Redirect", &[("location", &format!("{}/repositories/7", base))], r#"{"message":"Moved"}"#),
                reply("200 OK", &[], &repo_json("me/app", 7, false, "")),
            ]
        });
        let e = set_visibility(&Gh::at(&api, "t", "me"), "me", "app", false, 7).await.unwrap_err().to_string();
        assert!(e.contains("renamed or moved"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 2, "the redirect was not followed");
    }

    #[tokio::test]
    async fn an_organization_that_keeps_visibility_to_its_owners_is_named() {
        let (api, _) = github(vec![
            reply("200 OK", &[], &repo_json("acme/tool", 5, true, "")),
            reply("422 Unprocessable Entity", &[], r#"{"message":"Visibility can't be changed by members"}"#),
        ]);
        let e = set_visibility(&Gh::at(&api, "t", "me"), "acme", "tool", false, 5).await.unwrap_err().to_string();
        assert!(e.contains("acme lets only its owners change who can see its repositories") && e.contains("Ask an owner of acme"), "{e}");
    }

    #[tokio::test]
    async fn a_delete_without_the_typed_name_sends_nothing() {
        let (api, seen) = github(vec![]);
        let e = delete(&Gh::at(&api, "t", "me"), "me", "app", "me/ap", 7).await.unwrap_err().to_string();
        assert!(e.contains("type me/app"), "{e}");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_token_without_delete_repo_asks_for_approval_and_sends_no_delete() {
        let (api, seen) = github(vec![reply("200 OK", &[("x-oauth-scopes", "repo, read:org")], &repo_json("me/app", 7, true, ""))]);
        let out = delete(&Gh::at(&api, "t", "me"), "me", "app", " ME/app ", 7).await.unwrap();
        assert_eq!(out.outcome, "needs-permission");
        assert_eq!(out.restore_url, None);
        let reqs = seen.lock().unwrap().clone();
        assert_eq!(reqs.len(), 1);
        assert!(first_line(&reqs[0]).starts_with("GET "));
    }

    #[tokio::test]
    async fn with_delete_repo_it_is_deleted_and_the_restore_window_is_said() {
        let (api, seen) = github(vec![
            reply("200 OK", &[("x-oauth-scopes", "repo, delete_repo")], &repo_json("me/app", 7, true, "")),
            reply("204 No Content", &[], ""),
        ]);
        let out = delete(&Gh::at(&api, "t", "me"), "me", "app", "me/app", 7).await.unwrap();
        assert_eq!(out.outcome, "deleted");
        assert!(out.message.contains("90 days") && out.message.contains("@me's settings"), "{}", out.message);
        assert_eq!(out.restore_url.as_deref(), Some("https://github.com/settings/deleted_repositories"));
        let reqs = seen.lock().unwrap().clone();
        assert!(first_line(&reqs[1]).starts_with("DELETE /repos/me/app "), "{}", reqs[1]);
    }

    #[tokio::test]
    async fn a_deleted_fork_network_member_is_said_to_be_unrestorable() {
        let (api, _) = github(vec![
            reply("200 OK", &[("x-oauth-scopes", "delete_repo, repo")], &repo_json("me/app", 7, false, "").replace(r#""forks_count":0"#, r#""forks_count":2"#)),
            reply("204 No Content", &[], ""),
        ]);
        let out = delete(&Gh::at(&api, "t", "me"), "me", "app", "me/app", 7).await.unwrap();
        assert!(out.message.contains("can't be restored"), "{}", out.message);
        assert_eq!(out.restore_url, None, "no restore page for a fork network");
    }

    #[tokio::test]
    async fn an_organizations_repository_is_restored_by_its_owners() {
        let (api, _) = github(vec![
            reply("200 OK", &[("x-oauth-scopes", "repo, delete_repo")], &repo_json("acme/tool", 5, true, "")),
            reply("204 No Content", &[], ""),
        ]);
        let out = delete(&Gh::at(&api, "t", "me"), "acme", "tool", "acme/tool", 5).await.unwrap();
        assert!(out.message.contains("an owner of acme can restore it from acme's settings"), "{}", out.message);
        assert!(!out.message.contains("@me"), "{}", out.message);
        assert_eq!(out.restore_url.as_deref(), Some("https://github.com/organizations/acme/settings/deleted_repositories"));
    }

    #[tokio::test]
    async fn an_organization_that_stops_members_deleting_is_named() {
        let (api, _) = github(vec![
            reply("200 OK", &[("x-oauth-scopes", "repo, delete_repo")], &repo_json("acme/tool", 5, true, "")),
            reply("403 Forbidden", &[("x-oauth-scopes", "repo, delete_repo")], r#"{"message":"Organization members cannot delete repositories."}"#),
        ]);
        let e = delete(&Gh::at(&api, "t", "me"), "acme", "tool", "acme/tool", 5).await.unwrap_err().to_string();
        assert!(e.contains("acme lets only its owners delete its repositories"), "{e}");
    }

    #[tokio::test]
    async fn facts_name_the_branches_the_folder_lacks() {
        let dir = std::env::temp_dir().join(format!("gs-facts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("git").args(["init", "-q"]).current_dir(&dir).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return; // no git here: nothing to read
        }
        let branches = r#"[{"name":"main","commit":{"sha":"1111111111111111111111111111111111111111"}}]"#;
        let (api, _) = github(vec![reply("200 OK", &[], &repo_json("me/app", 7, true, "")), reply("200 OK", &[], branches)]);
        let f = facts(&Gh::at(&api, "t", "me"), "me", "app", Some(dir.to_str().unwrap())).await.unwrap();
        assert!(f.local_checked);
        assert_eq!(f.missing_here, vec!["main".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder with one commit: (its path, that commit), or None without git.
    fn folder_with_a_commit(tag: &str) -> Option<(std::path::PathBuf, String)> {
        let dir = std::env::temp_dir().join(format!("gs-facts-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        // No hooks of this computer's own run here.
        let git = |args: &[&str]| crate::spawn::command("git").arg("-c").arg("core.hooksPath=.no-hooks").args(args).current_dir(&dir).output().ok().filter(|o| o.status.success());
        git(&["init", "-q"])?;
        git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "x"])?;
        let head = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"])?.stdout).trim().to_string();
        Some((dir, head))
    }

    #[tokio::test]
    async fn facts_read_every_branch_page_and_check_them_all_at_once() {
        let Some((dir, head)) = folder_with_a_commit("pages") else { return };
        let page1 = format!(r#"[{{"name":"main","commit":{{"sha":"{}"}}}}]"#, head);
        let page2 = r#"[{"name":"dev","commit":{"sha":"2222222222222222222222222222222222222222"}}]"#;
        let (api, seen) = crate::gh_api::fake_github::github_with(|base| {
            vec![
                reply("200 OK", &[], &repo_json("me/app", 7, true, "")),
                reply("200 OK", &[("link", &format!("<{}/repos/me/app/branches?per_page=100&page=2>; rel=\"next\"", base))], &page1),
                reply("200 OK", &[], page2),
            ]
        });
        let f = facts(&Gh::at(&api, "t", "me"), "me", "app", Some(dir.to_str().unwrap())).await.unwrap();
        assert!(f.local_checked && !f.branches_capped);
        assert_eq!(f.missing_here, vec!["dev".to_string()], "main is in the folder, dev isn't");
        let reqs = seen.lock().unwrap().clone();
        assert!(first_line(&reqs[2]).contains("page=2"), "{}", reqs[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn facts_say_when_github_has_more_branches_than_were_compared() {
        let Some((dir, head)) = folder_with_a_commit("capped") else { return };
        let row = format!(r#"[{{"name":"b","commit":{{"sha":"{}"}}}}]"#, head);
        let (api, seen) = crate::gh_api::fake_github::github_with(|base| {
            let more = format!("<{}/repos/me/app/branches?page=99>; rel=\"next\"", base);
            let mut answers = vec![reply("200 OK", &[], &repo_json("me/app", 7, true, ""))];
            answers.extend((0..BRANCH_PAGES).map(|_| reply("200 OK", &[("link", &more)], &row)));
            answers
        });
        let f = facts(&Gh::at(&api, "t", "me"), "me", "app", Some(dir.to_str().unwrap())).await.unwrap();
        assert!(f.local_checked && f.branches_capped && f.missing_here.is_empty());
        assert_eq!(seen.lock().unwrap().len(), 1 + BRANCH_PAGES as usize);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_branch_page_github_refuses_leaves_the_folder_unchecked() {
        let Some((dir, head)) = folder_with_a_commit("refused") else { return };
        let page1 = format!(r#"[{{"name":"main","commit":{{"sha":"{}"}}}}]"#, head);
        let (api, _) = crate::gh_api::fake_github::github_with(|base| {
            vec![
                reply("200 OK", &[], &repo_json("me/app", 7, true, "")),
                reply("200 OK", &[("link", &format!("<{}/repos/me/app/branches?page=2>; rel=\"next\"", base))], &page1),
                reply("502 Bad Gateway", &[], r#"{"message":"Server Error"}"#),
            ]
        });
        let f = facts(&Gh::at(&api, "t", "me"), "me", "app", Some(dir.to_str().unwrap())).await.unwrap();
        assert!(!f.local_checked && f.missing_here.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
