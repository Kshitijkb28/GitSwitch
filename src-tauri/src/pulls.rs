//! Pull requests on GitHub: lists (one repository's, or across repositories
//! by search), one pull request with its files, commits, reviews and checks,
//! sending a review, and merging — always as the account the user picked.

use crate::error::AppError;
use crate::gh_api::{check_slug, github_message, Gh};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};

/// Rows per page of a list.
const PAGE: usize = 30;
/// Closed pages read at most for one page of merged (or closed-unmerged) rows.
const FILTER_PAGES: u32 = 5;
/// Files read at most: GitHub itself stops listing at 3000.
const MAX_FILE_PAGES: u32 = 30;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PullItem {
    /// "owner/name"
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub draft: bool,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    /// The branches (a repository's own list has them; a search does not).
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub comments: Option<u64>,
    pub labels: Vec<String>,
    pub html_url: String,
}

#[derive(Debug, Serialize, Default)]
pub struct PullPage {
    pub items: Vec<PullItem>,
    /// The page to ask for next, when there is more.
    pub next_page: Option<u32>,
    /// What a search counted in all (searches only).
    pub total: Option<u64>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn opt_s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_string)
}
fn u(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).unwrap_or(0)
}
fn login(v: &Value) -> String {
    v.get("user").map(|u| s(u, "login")).filter(|l| !l.is_empty()).unwrap_or_else(|| "ghost".into())
}
fn labels(v: &Value) -> Vec<String> {
    v.get("labels").and_then(|l| l.as_array()).map(|a| a.iter().map(|l| s(l, "name")).filter(|n| !n.is_empty()).collect()).unwrap_or_default()
}

/// One row from `GET /repos/{o}/{r}/pulls`.
pub(crate) fn item_from_pull(repo: &str, v: &Value) -> PullItem {
    let merged_at = opt_s(v, "merged_at");
    let state = if merged_at.is_some() { "merged".into() } else { s(v, "state") };
    PullItem {
        repo: repo.to_string(),
        number: u(v, "number"),
        title: s(v, "title"),
        author: login(v),
        draft: v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        state,
        created_at: s(v, "created_at"),
        updated_at: s(v, "updated_at"),
        merged_at,
        closed_at: opt_s(v, "closed_at"),
        head_ref: v.get("head").map(|h| s(h, "ref")),
        base_ref: v.get("base").map(|b| s(b, "ref")),
        comments: None,
        labels: labels(v),
        html_url: s(v, "html_url"),
    }
}

/// One row from `GET /search/issues` (pull requests only).
pub(crate) fn item_from_search(v: &Value) -> PullItem {
    let repo = s(v, "repository_url").rsplit("/repos/").next().unwrap_or("").to_string();
    let merged_at = v.get("pull_request").and_then(|p| opt_s(p, "merged_at"));
    let state = if merged_at.is_some() { "merged".into() } else { s(v, "state") };
    PullItem {
        repo,
        number: u(v, "number"),
        title: s(v, "title"),
        author: login(v),
        draft: v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        state,
        created_at: s(v, "created_at"),
        updated_at: s(v, "updated_at"),
        merged_at,
        closed_at: opt_s(v, "closed_at"),
        head_ref: None,
        base_ref: None,
        comments: v.get("comments").and_then(|c| c.as_u64()),
        labels: labels(v),
        html_url: s(v, "html_url"),
    }
}

fn check_state(state: &str) -> Result<(), AppError> {
    if matches!(state, "open" | "merged" | "closed" | "all") {
        Ok(())
    } else {
        Err(AppError::Config(format!("Unknown pull request state \"{}\"", state)))
    }
}

/// One page of a repository's pull requests, newest activity first. "merged"
/// and "closed" (closed without merging) are picked out of GitHub's closed
/// list, reading on until a page is full or five pages were read.
pub async fn list(gh: &Gh, owner: &str, name: &str, state: &str, page: u32) -> Result<PullPage, AppError> {
    check_slug(owner, name)?;
    check_state(state)?;
    let repo = format!("{}/{}", owner, name);
    let path = format!("/repos/{}/{}/pulls", owner, name);
    let page = page.max(1);
    if state == "open" || state == "all" {
        let q = [("state", state.to_string()), ("sort", "updated".into()), ("direction", "desc".into()), ("per_page", PAGE.to_string()), ("page", page.to_string())];
        let r = gh.get(&path, &q).await?;
        if !r.ok() {
            return Err(gh.refused(&r, &format!("list the pull requests of {}", repo)));
        }
        let items = r.json.as_array().map(|a| a.iter().map(|v| item_from_pull(&repo, v)).collect()).unwrap_or_default();
        return Ok(PullPage { items, next_page: r.has_next.then_some(page + 1), total: None });
    }
    let want_merged = state == "merged";
    let mut out = PullPage::default();
    let mut p = page;
    loop {
        let q = [("state", "closed".to_string()), ("sort", "updated".into()), ("direction", "desc".into()), ("per_page", "50".into()), ("page", p.to_string())];
        let r = gh.get(&path, &q).await?;
        if !r.ok() {
            return Err(gh.refused(&r, &format!("list the pull requests of {}", repo)));
        }
        for v in r.json.as_array().into_iter().flatten() {
            let it = item_from_pull(&repo, v);
            if (it.state == "merged") == want_merged {
                out.items.push(it);
            }
        }
        if !r.has_next {
            break;
        }
        p += 1;
        if out.items.len() >= PAGE || p - page >= FILTER_PAGES {
            out.next_page = Some(p);
            break;
        }
    }
    Ok(out)
}

/// The search for the Pull requests page: whose (created by, review asked of,
/// assigned to, or involving the account), in which state, optionally in one
/// repository.
pub(crate) fn search_query(scope: &str, state: &str, repo: Option<&str>) -> Result<String, AppError> {
    let who = match scope {
        "created" => "author:@me",
        "review_requested" => "review-requested:@me",
        "assigned" => "assignee:@me",
        "involves" => "involves:@me",
        other => return Err(AppError::Config(format!("Unknown scope \"{}\"", other))),
    };
    check_state(state)?;
    let mut q = format!("is:pr {} archived:false", who);
    match state {
        "open" => q.push_str(" is:open"),
        "merged" => q.push_str(" is:merged"),
        "closed" => q.push_str(" is:closed is:unmerged"),
        _ => {}
    }
    if let Some(r) = repo.map(str::trim).filter(|r| !r.is_empty()) {
        let (o, n) = r.split_once('/').ok_or_else(|| AppError::Config(format!("\"{}\" is not owner/name", r)))?;
        check_slug(o, n)?;
        q.push_str(&format!(" repo:{}/{}", o, n));
    }
    Ok(q)
}

pub async fn search(gh: &Gh, scope: &str, state: &str, repo: Option<&str>, page: u32) -> Result<PullPage, AppError> {
    let q = search_query(scope, state, repo)?;
    let page = page.max(1);
    let r = gh
        .get("/search/issues", &[("q", q), ("sort", "updated".into()), ("order", "desc".into()), ("per_page", PAGE.to_string()), ("page", page.to_string())])
        .await?;
    if !r.ok() {
        return Err(gh.refused(&r, "search pull requests"));
    }
    let total = r.json.get("total_count").and_then(|t| t.as_u64());
    let items: Vec<PullItem> = r.json.get("items").and_then(|i| i.as_array()).map(|a| a.iter().map(item_from_search).collect()).unwrap_or_default();
    // GitHub's search returns the first 1000 results at most.
    let more = total.is_some_and(|t| (page as u64) * (PAGE as u64) < t.min(1000)) && items.len() == PAGE;
    Ok(PullPage { items, next_page: more.then_some(page + 1), total })
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Review {
    pub login: String,
    /// "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED"
    pub state: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    /// "success" | "failure" | "pending" | "neutral" | "skipped"
    pub state: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PullDetail {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub draft: bool,
    pub merged_by: Option<String>,
    pub merged_at: Option<String>,
    /// None while GitHub is still working it out.
    pub mergeable: Option<bool>,
    /// GitHub's word: clean, blocked, behind, dirty (conflicts), unstable, draft, unknown…
    pub mergeable_state: String,
    pub head_ref: String,
    pub head_sha: String,
    /// The repository the branch lives in; None when that fork is gone.
    pub head_repo: Option<String>,
    pub base_ref: String,
    pub default_branch: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub commits: u64,
    pub comments: u64,
    pub created_at: String,
    pub updated_at: String,
    pub html_url: String,
    /// Merge methods the repository allows ("merge", "squash", "rebase").
    pub merge_methods: Vec<String>,
    /// False when GitHub did not say (it tells admins only): all are offered and GitHub decides.
    pub merge_methods_known: bool,
    pub can_push: bool,
    pub is_admin: bool,
    /// The account acting.
    pub viewer: String,
    /// Each reviewer's latest word.
    pub reviews: Vec<Review>,
    pub checks: Vec<Check>,
    /// Why checks or reviews could not be read, when they could not.
    pub notes: Vec<String>,
    /// The branch can be deleted after merging: it lives in this repository and is not its default.
    pub can_delete_branch: bool,
}

/// Each reviewer's latest decision; a comment after an approval does not undo it.
pub(crate) fn latest_reviews(list: &[Value]) -> Vec<Review> {
    let mut out: Vec<Review> = Vec::new();
    for v in list {
        let state = s(v, "state");
        if state == "PENDING" {
            continue;
        }
        let who = login(v);
        let r = Review { login: who.clone(), state: state.clone(), submitted_at: opt_s(v, "submitted_at") };
        match out.iter_mut().find(|x| x.login.eq_ignore_ascii_case(&who)) {
            Some(prev) if state != "COMMENTED" || prev.state == "COMMENTED" => *prev = r,
            Some(_) => {}
            None => out.push(r),
        }
    }
    out
}

/// Check runs and commit statuses, in one list.
pub(crate) fn checks_from(runs: &Value, statuses: &Value) -> Vec<Check> {
    let mut out = Vec::new();
    for c in runs.get("check_runs").and_then(|c| c.as_array()).into_iter().flatten() {
        let state = match (s(c, "status").as_str(), s(c, "conclusion").as_str()) {
            ("completed", "success") => "success",
            ("completed", "neutral") => "neutral",
            ("completed", "skipped") => "skipped",
            ("completed", _) => "failure",
            _ => "pending",
        };
        out.push(Check { name: s(c, "name"), state: state.into(), url: opt_s(c, "html_url") });
    }
    for st in statuses.get("statuses").and_then(|c| c.as_array()).into_iter().flatten() {
        let state = match s(st, "state").as_str() {
            "success" => "success",
            "pending" => "pending",
            _ => "failure",
        };
        out.push(Check { name: s(st, "context"), state: state.into(), url: opt_s(st, "target_url") });
    }
    out
}

fn repo_name(v: Option<&Value>) -> Option<String> {
    v.filter(|r| !r.is_null()).map(|r| s(r, "full_name")).filter(|n| !n.is_empty())
}

fn check_number(n: u64) -> Result<(), AppError> {
    if n == 0 {
        Err(AppError::Config("Pull request numbers start at 1".into()))
    } else {
        Ok(())
    }
}

pub async fn detail(gh: &Gh, owner: &str, name: &str, number: u64) -> Result<PullDetail, AppError> {
    check_slug(owner, name)?;
    check_number(number)?;
    let repo = format!("{}/{}", owner, name);
    let base = format!("/repos/{}/{}", owner, name);
    let pr = gh.get_ok(&format!("{}/pulls/{}", base, number), &[], &format!("read pull request #{} of {}", number, repo)).await?;
    let rp = gh.get(&base, &[]).await?;
    let repo_json = if rp.ok() { rp.json } else { Value::Null };
    let perms = repo_json.get("permissions");
    let flag = |k: &str| perms.and_then(|p| p.get(k)).and_then(|v| v.as_bool()).unwrap_or(false);
    let allow = |k: &str| repo_json.get(k).and_then(|v| v.as_bool());
    let known = ["allow_merge_commit", "allow_squash_merge", "allow_rebase_merge"].iter().any(|k| allow(k).is_some());
    let merge_methods: Vec<String> = [("allow_merge_commit", "merge"), ("allow_squash_merge", "squash"), ("allow_rebase_merge", "rebase")]
        .iter()
        .filter(|(k, _)| allow(k).unwrap_or(!known))
        .map(|(_, m)| m.to_string())
        .collect();
    let default_branch = s(&repo_json, "default_branch");

    let head = pr.get("head").cloned().unwrap_or(Value::Null);
    let head_sha = s(&head, "sha");
    let head_ref = s(&head, "ref");
    let head_repo = repo_name(head.get("repo"));
    let merged = pr.get("merged").and_then(|m| m.as_bool()).unwrap_or(false);

    let mut notes = Vec::new();
    let reviews = match gh.get(&format!("{}/pulls/{}/reviews", base, number), &[("per_page", "100".into())]).await {
        Ok(r) if r.ok() => latest_reviews(r.json.as_array().map(|a| a.as_slice()).unwrap_or(&[])),
        Ok(r) => {
            notes.push(format!("Reviews: {}", gh.refused(&r, "read the reviews")));
            Vec::new()
        }
        Err(e) => {
            notes.push(format!("Reviews: {}", e));
            Vec::new()
        }
    };
    let mut checks = Vec::new();
    if !head_sha.is_empty() {
        let runs = gh.get(&format!("{}/commits/{}/check-runs", base, head_sha), &[("per_page", "100".into())]).await;
        let statuses = gh.get(&format!("{}/commits/{}/status", base, head_sha), &[]).await;
        let (mut rv, mut sv) = (Value::Null, Value::Null);
        match runs {
            Ok(r) if r.ok() => rv = r.json,
            Ok(r) if r.status == 403 || r.status == 404 => notes.push("Checks: this account's token may not read check runs.".into()),
            Ok(r) => notes.push(format!("Checks: {}", gh.refused(&r, "read the checks"))),
            Err(e) => notes.push(format!("Checks: {}", e)),
        }
        if let Ok(r) = statuses {
            if r.ok() {
                sv = r.json;
            }
        }
        checks = checks_from(&rv, &sv);
    }

    Ok(PullDetail {
        repo: repo.clone(),
        number,
        title: s(&pr, "title"),
        body: s(&pr, "body"),
        author: login(&pr),
        state: if merged { "merged".into() } else { s(&pr, "state") },
        draft: pr.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        merged_by: pr.get("merged_by").filter(|m| !m.is_null()).map(|m| s(m, "login")),
        merged_at: opt_s(&pr, "merged_at"),
        mergeable: pr.get("mergeable").and_then(|m| m.as_bool()),
        mergeable_state: s(&pr, "mergeable_state"),
        can_delete_branch: head_repo.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(&repo)) && !head_ref.is_empty() && head_ref != default_branch,
        head_ref,
        head_sha,
        head_repo,
        base_ref: pr.get("base").map(|b| s(b, "ref")).unwrap_or_default(),
        default_branch,
        additions: u(&pr, "additions"),
        deletions: u(&pr, "deletions"),
        changed_files: u(&pr, "changed_files"),
        commits: u(&pr, "commits"),
        comments: u(&pr, "comments") + u(&pr, "review_comments"),
        created_at: s(&pr, "created_at"),
        updated_at: s(&pr, "updated_at"),
        html_url: s(&pr, "html_url"),
        merge_methods,
        merge_methods_known: known,
        can_push: flag("push") || flag("admin") || flag("maintain"),
        is_admin: flag("admin"),
        viewer: gh.login.clone(),
        reviews,
        checks,
        notes,
    })
}

#[derive(Debug, Serialize)]
pub struct PullFile {
    pub path: String,
    pub previous_path: Option<String>,
    /// added | removed | modified | renamed | copied | changed | unchanged
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub diff: crate::git_status::FileDiff,
}

#[derive(Debug, Serialize)]
pub struct PullFiles {
    pub files: Vec<PullFile>,
    /// GitHub lists 3000 files at most; more were changed.
    pub truncated: bool,
}

pub(crate) fn file_from(v: &Value) -> PullFile {
    let path = s(v, "filename");
    let status = s(v, "status");
    let diff = match v.get("patch").and_then(|p| p.as_str()) {
        Some(p) => crate::git_status::render_diff(p.as_bytes().to_vec(), &path, false, "No changed lines."),
        None => {
            let why = if status == "renamed" { "Renamed only — no changed lines." } else { "GitHub shows no diff for this file (binary, or too large)." };
            crate::git_status::render_diff(Vec::new(), &path, false, why)
        }
    };
    PullFile { previous_path: opt_s(v, "previous_filename"), additions: u(v, "additions"), deletions: u(v, "deletions"), status, path, diff }
}

pub async fn files(gh: &Gh, owner: &str, name: &str, number: u64) -> Result<PullFiles, AppError> {
    check_slug(owner, name)?;
    check_number(number)?;
    let path = format!("/repos/{}/{}/pulls/{}/files", owner, name, number);
    let mut out = PullFiles { files: Vec::new(), truncated: false };
    for page in 1..=MAX_FILE_PAGES {
        let r = gh.get(&path, &[("per_page", "100".into()), ("page", page.to_string())]).await?;
        if !r.ok() {
            return Err(gh.refused(&r, &format!("read the files of pull request #{}", number)));
        }
        out.files.extend(r.json.as_array().into_iter().flatten().map(file_from));
        if !r.has_next {
            return Ok(out);
        }
    }
    out.truncated = true;
    Ok(out)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct PullCommit {
    pub sha: String,
    /// The first line of the message.
    pub title: String,
    pub author: String,
    pub date: Option<String>,
}

pub async fn commits(gh: &Gh, owner: &str, name: &str, number: u64) -> Result<Vec<PullCommit>, AppError> {
    check_slug(owner, name)?;
    check_number(number)?;
    let path = format!("/repos/{}/{}/pulls/{}/commits", owner, name, number);
    let mut out = Vec::new();
    for page in 1..=3u32 {
        let r = gh.get(&path, &[("per_page", "100".into()), ("page", page.to_string())]).await?;
        if !r.ok() {
            return Err(gh.refused(&r, &format!("read the commits of pull request #{}", number)));
        }
        for v in r.json.as_array().into_iter().flatten() {
            let c = v.get("commit").cloned().unwrap_or(Value::Null);
            let msg = c.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let author = v
                .get("author")
                .filter(|a| !a.is_null())
                .map(|a| s(a, "login"))
                .filter(|l| !l.is_empty())
                .or_else(|| c.get("author").map(|a| s(a, "name")))
                .unwrap_or_default();
            out.push(PullCommit { sha: s(v, "sha"), title: msg.lines().next().unwrap_or("").to_string(), author, date: c.get("author").and_then(|a| opt_s(a, "date")) });
        }
        if !r.has_next {
            break;
        }
    }
    Ok(out)
}

/// Approve, ask for changes, or comment. Asking for changes and commenting need words.
pub async fn review(gh: &Gh, owner: &str, name: &str, number: u64, event: &str, body: &str) -> Result<Review, AppError> {
    check_slug(owner, name)?;
    check_number(number)?;
    if !matches!(event, "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
        return Err(AppError::Config(format!("Unknown review \"{}\"", event)));
    }
    if event != "APPROVE" && body.trim().is_empty() {
        return Err(AppError::Config(if event == "COMMENT" { "Write the comment first.".into() } else { "Write what should change first.".into() }));
    }
    let mut payload = json!({ "event": event });
    if !body.trim().is_empty() {
        payload["body"] = Value::String(body.to_string());
    }
    let r = gh.call(Method::POST, &format!("/repos/{}/{}/pulls/{}/reviews", owner, name, number), &[], Some(&payload)).await?;
    if r.ok() {
        return Ok(Review { login: login(&r.json), state: s(&r.json, "state"), submitted_at: opt_s(&r.json, "submitted_at") });
    }
    let said = github_message(&r.json).to_lowercase();
    if r.status == 422 && said.contains("your own pull request") {
        return Err(AppError::Command(format!(
            "GitHub does not let @{} {} their own pull request. Leave a comment instead, or review it as another account.",
            gh.login,
            if event == "APPROVE" { "approve" } else { "ask for changes on" }
        )));
    }
    Err(gh.refused(&r, "send the review"))
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MergeOutcome {
    pub merged: bool,
    /// The commit GitHub made (or the branch tip, for a rebase).
    pub sha: String,
    pub message: String,
    /// Some when deleting the branch was asked: whether it went.
    pub branch_deleted: Option<bool>,
    pub branch_note: Option<String>,
}

/// Merge with the head commit the user reviewed: when someone pushed after,
/// GitHub refuses and nothing unreviewed is merged.
#[allow(clippy::too_many_arguments)]
pub async fn merge(gh: &Gh, owner: &str, name: &str, number: u64, method: &str, sha: &str, title: Option<&str>, message: Option<&str>, delete_branch: bool) -> Result<MergeOutcome, AppError> {
    check_slug(owner, name)?;
    check_number(number)?;
    if !matches!(method, "merge" | "squash" | "rebase") {
        return Err(AppError::Config(format!("Unknown merge method \"{}\"", method)));
    }
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::Config("The reviewed commit is missing — refresh the pull request first.".into()));
    }
    let base = format!("/repos/{}/{}", owner, name);
    let mut payload = json!({ "merge_method": method, "sha": sha });
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        payload["commit_title"] = Value::String(t.to_string());
    }
    if let Some(m) = message.map(str::trim).filter(|m| !m.is_empty()) {
        payload["commit_message"] = Value::String(m.to_string());
    }
    let r = gh.call(Method::PUT, &format!("{}/pulls/{}/merge", base, number), &[], Some(&payload)).await?;
    if !r.ok() {
        let said = github_message(&r.json);
        return Err(AppError::Command(match r.status {
            409 => "New commits were pushed to this branch after you reviewed it, so nothing was merged. Refresh, look at them, and merge again.".into(),
            405 => format!("GitHub would not merge it{}.", if said.is_empty() { String::new() } else { format!(": {}", said.trim_end_matches('.')) }),
            _ => crate::gh_api::refusal(&r, &format!("merge pull request #{}", number), &gh.login),
        }));
    }
    let mut out = MergeOutcome { merged: r.json.get("merged").and_then(|m| m.as_bool()).unwrap_or(true), sha: s(&r.json, "sha"), message: s(&r.json, "message"), branch_deleted: None, branch_note: None };
    if delete_branch {
        // Asked again rather than trusted from the page: only a branch of this
        // repository, and never its default branch.
        let pr = gh.get(&format!("{}/pulls/{}", base, number), &[]).await?;
        let rp = gh.get(&base, &[]).await?;
        let head = pr.json.get("head").cloned().unwrap_or(Value::Null);
        let head_ref = s(&head, "ref");
        let same_repo = repo_name(head.get("repo")).is_some_and(|h| h.eq_ignore_ascii_case(&format!("{}/{}", owner, name)));
        let default = s(&rp.json, "default_branch");
        if !pr.ok() || !rp.ok() || head_ref.is_empty() {
            out.branch_deleted = Some(false);
            out.branch_note = Some("Merged. The branch was kept: GitHub could not be asked about it again.".into());
        } else if !same_repo {
            out.branch_deleted = Some(false);
            out.branch_note = Some("Merged. The branch lives in a fork, so it was left there.".into());
        } else if head_ref == default {
            out.branch_deleted = Some(false);
            out.branch_note = Some(format!("Merged. {} is the default branch, so it was kept.", head_ref));
        } else {
            let d = gh.call(Method::DELETE, &format!("{}/git/refs/heads/{}", base, head_ref), &[], None).await?;
            if d.ok() || (d.status == 422 && github_message(&d.json).to_lowercase().contains("does not exist")) {
                out.branch_deleted = Some(true);
                out.branch_note = Some(format!("Branch {} deleted.", head_ref));
            } else {
                out.branch_deleted = Some(false);
                out.branch_note = Some(format!("Merged, but the branch was kept: {}", gh.refused(&d, &format!("delete branch {}", head_ref))));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_merged_from_merged_at() {
        let v = json!({"number": 7, "title": "T", "user": {"login": "al"}, "state": "closed", "merged_at": "2026-01-02T00:00:00Z", "head": {"ref": "f"}, "base": {"ref": "main"}, "labels": [{"name": "bug"}], "html_url": "h"});
        let it = item_from_pull("o/r", &v);
        assert_eq!((it.state.as_str(), it.head_ref.as_deref(), it.labels.clone()), ("merged", Some("f"), vec!["bug".to_string()]));
        let v = json!({"number": 8, "state": "closed", "merged_at": null, "user": null});
        assert_eq!((item_from_pull("o/r", &v).state.as_str(), item_from_pull("o/r", &v).author.as_str()), ("closed", "ghost"));
        let v = json!({"number": 9, "state": "closed", "repository_url": "https://api.github.com/repos/acme/tool", "pull_request": {"merged_at": "x"}, "comments": 3});
        let it = item_from_search(&v);
        assert_eq!((it.repo.as_str(), it.state.as_str(), it.comments), ("acme/tool", "merged", Some(3)));
    }

    #[test]
    fn search_queries() {
        assert_eq!(search_query("created", "open", None).unwrap(), "is:pr author:@me archived:false is:open");
        assert_eq!(search_query("review_requested", "merged", Some("a/b")).unwrap(), "is:pr review-requested:@me archived:false is:merged repo:a/b");
        assert_eq!(search_query("assigned", "closed", Some(" ")).unwrap(), "is:pr assignee:@me archived:false is:closed is:unmerged");
        assert_eq!(search_query("involves", "all", None).unwrap(), "is:pr involves:@me archived:false");
        assert!(search_query("everyone", "open", None).is_err());
        assert!(search_query("created", "open", Some("a/b c")).is_err());
        assert!(search_query("created", "draft", None).is_err());
    }

    #[test]
    fn a_reviewers_comment_after_approving_keeps_the_approval() {
        let list = vec![
            json!({"user": {"login": "a"}, "state": "CHANGES_REQUESTED"}),
            json!({"user": {"login": "a"}, "state": "APPROVED"}),
            json!({"user": {"login": "A"}, "state": "COMMENTED"}),
            json!({"user": {"login": "b"}, "state": "COMMENTED"}),
            json!({"user": {"login": "c"}, "state": "PENDING"}),
        ];
        let r = latest_reviews(&list);
        assert_eq!(r.iter().map(|x| (x.login.as_str(), x.state.as_str())).collect::<Vec<_>>(), vec![("a", "APPROVED"), ("b", "COMMENTED")]);
    }

    #[test]
    fn checks_merge_runs_and_statuses() {
        let runs = json!({"check_runs": [
            {"name": "build", "status": "completed", "conclusion": "success"},
            {"name": "lint", "status": "completed", "conclusion": "failure"},
            {"name": "e2e", "status": "in_progress", "conclusion": null},
            {"name": "docs", "status": "completed", "conclusion": "skipped"}
        ]});
        let statuses = json!({"statuses": [{"context": "ci/legacy", "state": "error"}, {"context": "deploy", "state": "pending"}]});
        let c = checks_from(&runs, &statuses);
        assert_eq!(
            c.iter().map(|x| (x.name.as_str(), x.state.as_str())).collect::<Vec<_>>(),
            vec![("build", "success"), ("lint", "failure"), ("e2e", "pending"), ("docs", "skipped"), ("ci/legacy", "failure"), ("deploy", "pending")]
        );
    }

    #[test]
    fn a_files_patch_becomes_the_diff_panels_lines() {
        let f = file_from(&json!({"filename": "a.txt", "status": "modified", "additions": 1, "deletions": 1, "patch": "@@ -1 +1 @@\n-old\n+new"}));
        assert_eq!((f.diff.added, f.diff.removed, f.diff.lines.len()), (1, 1, 3));
        assert_eq!(f.diff.lines[0].kind, "hunk");
        let f = file_from(&json!({"filename": "img.png", "status": "added"}));
        assert!(f.diff.lines.is_empty() && f.diff.empty_reason.as_deref().unwrap().contains("binary, or too large"));
        let f = file_from(&json!({"filename": "n.txt", "previous_filename": "o.txt", "status": "renamed"}));
        assert_eq!((f.previous_path.as_deref(), f.diff.empty_reason.as_deref()), (Some("o.txt"), Some("Renamed only — no changed lines.")));
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    /// A local GitHub answering each connection with the next canned reply
    /// (made knowing its own address, so paging links can point at it);
    /// every request recorded as it arrived.
    fn github_with(make: impl FnOnce(&str) -> Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
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

    fn github(answers: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        github_with(|_| answers)
    }

    fn reply(status: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut head = format!("HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n", status, body.len());
        for (k, v) in headers {
            head.push_str(&format!("{}: {}\r\n", k, v));
        }
        format!("{}\r\n{}", head, body)
    }

    fn first_line(req: &str) -> String {
        req.lines().next().unwrap_or("").to_string()
    }

    fn closed_pages() -> (String, String) {
        let pull = |n: u64, merged: bool| json!({"number": n, "state": "closed", "merged_at": if merged { json!("2026-01-01T00:00:00Z") } else { Value::Null }, "user": {"login": "x"}});
        let page1: Vec<Value> = (1..=50).map(|n| pull(n, n % 5 == 0)).collect(); // 10 merged
        let page2: Vec<Value> = (51..=100).map(|n| pull(n, n % 2 == 0)).collect(); // 25 merged
        (serde_json::to_string(&page1).unwrap(), serde_json::to_string(&page2).unwrap())
    }

    #[tokio::test]
    async fn merged_rows_fill_a_page_across_closed_pages_and_the_next_page_is_where_it_stopped() {
        let (p1, p2) = closed_pages();
        let (api, seen) = github_with(|api| {
            vec![
                reply("200 OK", &[("link", &format!("<{}/repos/o/r/pulls?page=2>; rel=\"next\"", api))], &p1),
                reply("200 OK", &[("link", &format!("<{}/repos/o/r/pulls?page=3>; rel=\"next\"", api))], &p2),
            ]
        });
        let got = list(&Gh::at(&api, "t", "me"), "o", "r", "merged", 1).await.unwrap();
        assert_eq!((got.items.len(), got.next_page), (35, Some(3)));
        assert!(got.items.iter().all(|i| i.state == "merged"));
        let reqs = seen.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2);
        assert!(first_line(&reqs[0]).starts_with("GET /repos/o/r/pulls?state=closed&sort=updated&direction=desc&per_page=50&page=1 "), "{}", reqs[0]);
        assert!(first_line(&reqs[1]).contains("&page=2 "), "{}", reqs[1]);
        assert!(reqs[0].contains("authorization: Bearer t"));
    }

    #[tokio::test]
    async fn a_next_link_to_another_host_is_not_followed() {
        let (p1, _) = closed_pages();
        let (api, seen) = github(vec![reply("200 OK", &[("link", "<https://evil.example/repos/o/r/pulls?page=2>; rel=\"next\"")], &p1)]);
        let got = list(&Gh::at(&api, "t", "me"), "o", "r", "closed", 1).await.unwrap();
        assert_eq!((got.items.len(), got.next_page), (40, None));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn merge_sends_the_reviewed_commit_and_words_a_push_after_review() {
        let (api, seen) = github(vec![reply("409 Conflict", &[], r#"{"message":"Head branch was modified. Review and try the merge again."}"#)]);
        let gh = Gh::at(&api, "t", "me");
        let e = merge(&gh, "o", "r", 4, "squash", "abc1234def", Some("Title"), None, false).await.unwrap_err().to_string();
        assert!(e.contains("New commits were pushed"), "{}", e);
        let req = seen.lock().unwrap()[0].clone();
        assert!(first_line(&req).starts_with("PUT /repos/o/r/pulls/4/merge "), "{}", req);
        let body: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"merge_method": "squash", "sha": "abc1234def", "commit_title": "Title"}));

        let (api, _) = github(vec![reply("405 Method Not Allowed", &[], r#"{"message":"Required status check \"build\" is expected."}"#)]);
        let e = merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "merge", "abc1234", None, None, false).await.unwrap_err().to_string();
        assert!(e.ends_with("GitHub would not merge it: Required status check \"build\" is expected."), "{}", e);
        assert!(merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "octopus", "abc1234", None, None, false).await.is_err());
        assert!(merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "merge", "", None, None, false).await.is_err());
    }

    #[tokio::test]
    async fn the_branch_is_deleted_only_when_it_lives_here_and_is_not_the_default() {
        let merged = reply("200 OK", &[], r#"{"merged":true,"sha":"m1","message":"Pull Request successfully merged"}"#);
        let pr = |repo: &str, r: &str| reply("200 OK", &[], &json!({"head": {"ref": r, "repo": {"full_name": repo}}}).to_string());
        let repo = reply("200 OK", &[], r#"{"default_branch":"main"}"#);

        let (api, seen) = github(vec![merged.clone(), pr("o/r", "feat/x"), repo.clone(), reply("204 No Content", &[], "")]);
        let out = merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "merge", "abc1234", None, None, true).await.unwrap();
        assert_eq!((out.merged, out.branch_deleted), (true, Some(true)));
        assert!(first_line(&seen.lock().unwrap()[3]).starts_with("DELETE /repos/o/r/git/refs/heads/feat/x "));

        let (api, seen) = github(vec![merged.clone(), pr("someone/r", "feat/x"), repo.clone()]);
        let out = merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "merge", "abc1234", None, None, true).await.unwrap();
        assert_eq!(out.branch_deleted, Some(false));
        assert!(out.branch_note.unwrap().contains("fork"));
        assert_eq!(seen.lock().unwrap().len(), 3);

        let (api, _) = github(vec![merged, pr("o/r", "main"), repo]);
        let out = merge(&Gh::at(&api, "t", "me"), "o", "r", 4, "merge", "abc1234", None, None, true).await.unwrap();
        assert!(out.branch_note.unwrap().contains("default branch"));
    }

    #[tokio::test]
    async fn approving_your_own_pull_request_is_worded() {
        let (api, seen) = github(vec![reply("422 Unprocessable Entity", &[], r#"{"message":"Unprocessable Entity","errors":["Can not approve your own pull request"]}"#)]);
        let e = review(&Gh::at(&api, "t", "me"), "o", "r", 2, "APPROVE", "").await.unwrap_err().to_string();
        assert!(e.contains("does not let @me approve their own pull request"), "{}", e);
        let req = seen.lock().unwrap()[0].clone();
        assert!(first_line(&req).starts_with("POST /repos/o/r/pulls/2/reviews "));
        assert!(review(&Gh::at(&api, "t", "me"), "o", "r", 2, "REQUEST_CHANGES", "  ").await.is_err());
    }

    #[tokio::test]
    async fn detail_reads_merge_options_reviews_and_checks() {
        let pr = json!({"title": "T", "body": "B", "user": {"login": "al"}, "state": "open", "draft": false, "merged": false, "mergeable": null, "mergeable_state": "unknown",
            "head": {"ref": "f", "sha": "abc1234", "repo": {"full_name": "o/r"}}, "base": {"ref": "main"}, "additions": 3, "deletions": 1, "changed_files": 2, "commits": 1, "comments": 1, "review_comments": 2});
        let (api, _) = github(vec![
            reply("200 OK", &[], &pr.to_string()),
            reply("200 OK", &[], r#"{"default_branch":"main","allow_merge_commit":false,"allow_squash_merge":true,"allow_rebase_merge":true,"permissions":{"admin":false,"push":true}}"#),
            reply("200 OK", &[], r#"[{"user":{"login":"bo"},"state":"APPROVED"}]"#),
            reply("403 Forbidden", &[], r#"{"message":"Resource not accessible by integration"}"#),
            reply("200 OK", &[], r#"{"statuses":[{"context":"ci","state":"success"}]}"#),
        ]);
        let d = detail(&Gh::at(&api, "t", "me"), "o", "r", 5).await.unwrap();
        assert_eq!(d.mergeable, None);
        assert_eq!(d.merge_methods, vec!["squash", "rebase"]);
        assert!(d.merge_methods_known && d.can_push && !d.is_admin && d.can_delete_branch);
        assert_eq!((d.comments, d.reviews.len(), d.checks.len()), (3, 1, 1));
        assert!(d.notes.iter().any(|n| n.contains("check runs")));
    }
}
