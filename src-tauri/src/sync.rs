//! "Take the latest, keep my commits on top" — for any repository, submodules
//! included.
//!
//! A plain rebase pull either refuses (dirty tree, submodule pointer moved on
//! both sides) or leaves the submodules stale. This module does what a careful
//! person does by hand: fetch, assess, back up, rebase the superproject with
//! autostash, resolve each submodule-pointer stop by rebasing that submodule's
//! own commits onto the commit upstream recorded, fast-forward the rest,
//! verify — and refuse, by name, every state it cannot handle safely.
//!
//! Every decision is a pure function of measured facts (`plan`), every git
//! command is pinned (`GitCmd::pinned`) so a user's config cannot change what
//! it means, and progress lives in a state file inside the repository's git
//! directory so a paused sync survives page switches and app restarts.
//! Facts behind the choices are recorded in `scripts/verify/sync-facts.md`.

use crate::error::AppError;
use crate::git_exec::{GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT};
use crate::git_ops::{check, Intent, OpResult, Refusal, StatusFacts};
use crate::git_status::{self, RepoStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const BACKUP_PREFIX: &str = "gitswitch-before-sync-";
const STATE_FILE: &str = "gitswitch-sync.json";
const STATE_VERSION: u32 = 1;
const MAX_LISTED: usize = 30;

// ---------------------------------------------------------------------------
// Facts — measured, then handed to the pure planner
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CommitRef {
    pub oid: String,
    pub short: String,
    pub subject: String,
    /// Submodule paths whose pointer this commit moves.
    #[serde(default)]
    pub touches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SuperFacts {
    pub branch: String,
    pub upstream: String,
    pub upstream_ref: String,
    pub remote: String,
    pub head: String,
    pub upstream_oid: String,
    pub base_oid: String,
    pub ahead: usize,
    pub behind: usize,
    pub own_commits: Vec<CommitRef>,
    pub incoming: Vec<CommitRef>,
    pub file_overlap: Vec<String>,
    pub dirty_tracked: Vec<String>,
    pub untracked: usize,
    pub dirty_overlap: Vec<String>,
    pub untracked_overlap: Vec<String>,
    pub is_submodule_itself: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubFacts {
    pub path: String,
    pub name: Option<String>,
    pub mapped: bool,
    pub populated: bool,
    pub gitdir_valid: bool,
    pub head: Option<String>,
    pub recorded: String,
    /// What upstream's superproject records; `None` when upstream removed it.
    pub target: Option<String>,
    pub base_link: Option<String>,
    pub target_present: bool,
    pub head_is_ancestor_of_target: bool,
    pub target_is_ancestor_of_head: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub remote: Option<String>,
    /// The branch `refs/remotes/<remote>/HEAD` points at, when it exists.
    pub remote_head_branch: Option<String>,
    pub tip_ahead_of_target: bool,
    pub own_commits_moving_gitlink: usize,
    pub my_commits_move_gitlink: bool,
    pub upstream_moves_gitlink: bool,
    pub dirty_tracked: usize,
    pub untracked: usize,
    pub dirty_overlap: Vec<String>,
    pub conflicted: usize,
    pub operation: Option<String>,
    pub fetch_error: Option<String>,
    pub has_nested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skipped {
    pub path: String,
    /// "unmapped" | "not-initialised"
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncFacts {
    pub superproject: SuperFacts,
    pub submodules: Vec<SubFacts>,
    pub skipped: Vec<Skipped>,
}

// ---------------------------------------------------------------------------
// The plan — what will happen, or why it cannot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubPlan {
    pub path: String,
    pub name: Option<String>,
    /// none | fast-forward | rebase | ahead | blocked
    pub action: String,
    pub reason: String,
    pub head: Option<String>,
    pub recorded: String,
    pub target: Option<String>,
    pub rebase_onto: Option<String>,
    pub predicted_gitlink_conflict: bool,
    pub branch: Option<String>,
    pub detached: bool,
    /// Detached only: the branch it is re-attached to after its rebase when
    /// that branch stood where the checkout was (named from the remote's HEAD).
    pub reattach_to: Option<String>,
    pub dirty_tracked: usize,
    pub untracked: usize,
    pub fetch_error: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SuperPlan {
    pub branch: String,
    pub upstream: String,
    pub head: String,
    pub upstream_oid: String,
    pub ahead: usize,
    pub behind: usize,
    pub own_commits: Vec<CommitRef>,
    pub incoming: Vec<CommitRef>,
    pub file_overlap: Vec<String>,
    pub dirty_count: usize,
    pub untracked: usize,
    pub will_stash: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncPlan {
    pub can_run: bool,
    pub nothing_to_do: bool,
    pub refusal: Option<Refusal>,
    pub blockers: Vec<String>,
    pub superproject: SuperPlan,
    pub submodules: Vec<SubPlan>,
    pub skipped: Vec<Skipped>,
    /// Oids the run re-checks before touching anything: "" for the
    /// superproject's upstream, then one per submodule path (its target).
    pub fingerprint: Vec<(String, String)>,
    pub summary: String,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

/// Pure: facts in, plan out. Every ambiguous state is a named blocker.
pub fn plan(facts: &SyncFacts, stash: bool) -> SyncPlan {
    let s = &facts.superproject;
    let mut blockers: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    let dirty = !s.dirty_tracked.is_empty();
    if dirty && !stash {
        blockers.push(format!(
            "You have {} with uncommitted changes. Turn on stashing for the sync, or commit or stash them yourself first.",
            plural(s.dirty_tracked.len(), "file", "files")
        ));
    }
    if dirty && stash && !s.dirty_overlap.is_empty() {
        blockers.push(format!(
            "Your uncommitted changes to {} collide with incoming commits; a stash could not be re-applied cleanly. Commit or stash them yourself first.",
            s.dirty_overlap.join(", ")
        ));
    }
    if !s.untracked_overlap.is_empty() {
        blockers.push(format!(
            "Untracked {} would be overwritten by incoming commits: {}. Move or remove them first.",
            if s.untracked_overlap.len() == 1 { "file" } else { "files" },
            s.untracked_overlap.join(", ")
        ));
    }
    if !s.file_overlap.is_empty() {
        notes.push(format!(
            "Your commits and the incoming ones both touch {}; the rebase may stop there for you to resolve.",
            s.file_overlap.join(", ")
        ));
    }
    if s.is_submodule_itself {
        notes.push("This repository is itself a submodule; its parent will need a pointer update afterwards.".into());
    }

    let mut subs = Vec::new();
    for f in &facts.submodules {
        let mut p = SubPlan {
            path: f.path.clone(),
            name: f.name.clone(),
            action: "none".into(),
            reason: String::new(),
            head: f.head.clone(),
            recorded: f.recorded.clone(),
            target: f.target.clone(),
            rebase_onto: None,
            predicted_gitlink_conflict: false,
            branch: f.branch.clone(),
            detached: f.populated && f.branch.is_none(),
            reattach_to: if f.populated && f.branch.is_none() { f.remote_head_branch.clone() } else { None },
            dirty_tracked: f.dirty_tracked,
            untracked: f.untracked,
            fetch_error: f.fetch_error.clone(),
            notes: Vec::new(),
        };
        let block = |p: &mut SubPlan, why: String| {
            p.action = "blocked".into();
            p.reason = why;
        };
        let head = f.head.clone().unwrap_or_default();
        let pointer_moves = f.my_commits_move_gitlink || f.upstream_moves_gitlink;
        if !f.gitdir_valid {
            block(&mut p, "its .git points at a directory that no longer exists; re-initialise it (git submodule update --init) first".into());
        } else if let Some(op) = &f.operation {
            block(&mut p, format!("a {} is in progress inside it; finish or abort that first", op));
        } else if f.conflicted > 0 {
            block(&mut p, format!("{} inside it still {} conflicts", plural(f.conflicted, "file", "files"), if f.conflicted == 1 { "has" } else { "have" }));
        } else if f.target.is_none() {
            block(&mut p, "upstream removed this submodule; decide by hand what to do with your copy".into());
        } else if !f.target_present {
            block(&mut p, match &f.fetch_error {
                Some(e) => format!("the commit upstream records ({}) is not available here and fetching failed: {}", short(f.target.as_deref().unwrap_or("")), e),
                None => format!("the commit upstream records ({}) is not reachable from its remote's branches", short(f.target.as_deref().unwrap_or(""))),
            });
        } else if !same_oid(&head, &f.recorded) && pointer_moves {
            block(&mut p, format!(
                "it is checked out at {} but this branch records {}; commit that pointer (Record submodule pointers) or move it back before syncing",
                short(&head), short(&f.recorded)
            ));
        } else if f.own_commits_moving_gitlink > 1 {
            block(&mut p, format!(
                "{} of your commits move this submodule's pointer; the sync handles one — squash them, or do this one by hand",
                f.own_commits_moving_gitlink
            ));
        } else {
            let target = f.target.clone().unwrap_or_default();
            if let Some(e) = &f.fetch_error {
                p.notes.push(format!("its remote could not be fetched ({}); the recorded commit is available locally", e));
            }
            if f.tip_ahead_of_target {
                p.notes.push("its remote has newer commits than upstream's superproject records; the sync targets what is recorded".into());
            }
            if same_oid(&head, &target) {
                p.action = "none".into();
                p.reason = "already at the commit upstream records".into();
            } else if f.head_is_ancestor_of_target {
                p.action = "fast-forward".into();
                p.reason = format!("moves forward to {} (no commits of yours)", short(&target));
            } else if f.target_is_ancestor_of_head {
                if f.my_commits_move_gitlink {
                    p.action = "none".into();
                    p.reason = format!("already contains {}; your commit's pointer will be kept", short(&target));
                } else {
                    p.action = "ahead".into();
                    p.reason = format!("your commits sit on top of {} already; the superproject does not record them yet", short(&target));
                }
            } else {
                // Diverged: rebase this submodule's own commits onto the recorded target.
                if !f.dirty_overlap.is_empty() {
                    block(&mut p, format!(
                        "its uncommitted changes to {} collide with the commits it will be rebased onto; commit or stash them inside it first",
                        f.dirty_overlap.join(", ")
                    ));
                } else {
                    p.action = "rebase".into();
                    p.rebase_onto = Some(target.clone());
                    p.predicted_gitlink_conflict = f.my_commits_move_gitlink && f.upstream_moves_gitlink;
                    p.reason = format!(
                        "your {} rebased onto {}{}",
                        plural(own_count(f), "commit is", "commits are"),
                        short(&target),
                        if p.predicted_gitlink_conflict { " when the superproject rebase stops on its pointer" } else { " after the superproject rebase" }
                    );
                    if p.detached {
                        p.notes.push(match &f.remote_head_branch {
                            Some(b) => format!("it is detached; it is re-attached to '{}' afterwards when that branch was where it stood", b),
                            None => "it is detached and stays detached (no default branch is known for its remote)".into(),
                        });
                    }
                    if f.dirty_tracked > 0 {
                        p.notes.push(format!("its {} stashed and re-applied around the rebase", plural(f.dirty_tracked, "uncommitted change is", "uncommitted changes are")));
                    }
                }
            }
        }
        if p.action == "blocked" {
            blockers.push(format!("{}: {}", f.path, p.reason));
        }
        subs.push(p);
    }

    let submodule_work = subs.iter().any(|p| matches!(p.action.as_str(), "fast-forward" | "rebase"));
    let nothing_to_do = s.behind == 0 && !submodule_work && blockers.is_empty();
    let can_run = blockers.is_empty() && !nothing_to_do;
    let mut fingerprint = vec![(String::new(), s.upstream_oid.clone())];
    for f in &facts.submodules {
        if let Some(t) = &f.target {
            fingerprint.push((f.path.clone(), t.clone()));
        }
    }
    let summary = if nothing_to_do {
        format!("Nothing to sync — you are level with {} and every submodule is where it should be.", s.upstream)
    } else if !blockers.is_empty() {
        format!("Cannot sync yet: {}.", plural(blockers.len(), "thing needs", "things need"))
    } else if s.behind == 0 {
        "Only submodules need aligning.".into()
    } else {
        format!(
            "Your {} replayed on top of {} from {}{}.",
            plural(s.ahead, "commit is", "commits are"),
            plural(s.behind, "new commit", "new commits"),
            s.upstream,
            if subs.iter().any(|p| p.action == "rebase") { ", rebasing the submodules that carry commits of yours" } else { "" }
        )
    };
    SyncPlan {
        can_run,
        nothing_to_do,
        refusal: None,
        blockers,
        superproject: SuperPlan {
            branch: s.branch.clone(),
            upstream: s.upstream.clone(),
            head: s.head.clone(),
            upstream_oid: s.upstream_oid.clone(),
            ahead: s.ahead,
            behind: s.behind,
            own_commits: s.own_commits.clone(),
            incoming: s.incoming.clone(),
            file_overlap: s.file_overlap.clone(),
            dirty_count: s.dirty_tracked.len(),
            untracked: s.untracked,
            will_stash: dirty && stash,
            notes,
        },
        submodules: subs,
        skipped: facts.skipped.clone(),
        fingerprint,
        summary,
    }
}

fn own_count(f: &SubFacts) -> usize {
    f.own_commits_moving_gitlink.max(1)
}

pub(crate) fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn same_oid(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

// ---------------------------------------------------------------------------
// State file — the sync survives page switches and app restarts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubState {
    pub path: String,
    pub branch_before: Option<String>,
    #[serde(default)]
    pub reattach_to: Option<String>,
    pub head_before: String,
    pub backup: Option<String>,
    pub target: String,
    pub rebased: bool,
    pub head_after: Option<String>,
    pub aligned: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NeedsUser {
    /// superproject | submodule:<path> | submodule-autostash:<path> | autostash-conflict
    pub where_: String,
    pub paths: Vec<String>,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncState {
    pub version: u32,
    pub nonce: String,
    pub started_at: String,
    /// preparing | rebasing | aligning | done | aborted
    pub phase: String,
    pub repo: String,
    pub upstream_ref: String,
    pub upstream_oid: String,
    pub head_before: String,
    pub stash: bool,
    pub backup: Option<String>,
    pub captures: BTreeMap<String, Vec<String>>,
    pub subs: Vec<SubState>,
    pub needs_user: Option<NeedsUser>,
    pub plan: SyncPlan,
    pub bundles: Vec<String>,
}

/// Where the state lives: per worktree, dies with the repo, invisible to git.
async fn state_path(repo: &Path) -> Result<PathBuf, AppError> {
    let p = GitCmd::at(repo)
        .args(["rev-parse", "--path-format=absolute", "--git-path", STATE_FILE])
        .text()
        .await?;
    Ok(PathBuf::from(p.trim()))
}

pub async fn read_state(repo: &Path) -> Option<SyncState> {
    let p = state_path(repo).await.ok()?;
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_state_at(path: &Path, st: &SyncState) -> Result<(), AppError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(st)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A summary for `RepoStatus`, also true for a submodule whose superproject
/// is mid-sync.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SyncSummary {
    pub phase: String,
    pub started_at: String,
    pub needs_user: Option<NeedsUser>,
    /// The repository the sync belongs to (the superproject).
    pub root: String,
    /// False when this repository is a submodule of the repository being synced.
    pub is_root: bool,
}

pub async fn summary_for(repo: &Path) -> Option<SyncSummary> {
    if let Some(st) = read_state(repo).await {
        return Some(SyncSummary { phase: st.phase, started_at: st.started_at, needs_user: st.needs_user, root: st.repo, is_root: true });
    }
    let parent = GitCmd::at(repo)
        .args(["rev-parse", "--show-superproject-working-tree"])
        .ok_text()
        .await
        .filter(|s| !s.is_empty())?;
    let st = read_state(Path::new(&parent)).await?;
    Some(SyncSummary { phase: st.phase, started_at: st.started_at, needs_user: st.needs_user, root: st.repo, is_root: false })
}

// ---------------------------------------------------------------------------
// Gathering facts
// ---------------------------------------------------------------------------

fn g(repo: &Path) -> GitCmd {
    GitCmd::at(repo).pinned()
}

pub(crate) async fn oid(repo: &Path, rev: &str) -> Option<String> {
    g(repo).args(["rev-parse", "--verify", "--quiet", rev]).ok_text().await.filter(|s| !s.is_empty())
}

pub(crate) async fn is_ancestor(repo: &Path, a: &str, b: &str) -> bool {
    g(repo).args(["merge-base", "--is-ancestor", a, b]).run().await.map(|o| o.ok()).unwrap_or(false)
}

async fn commit_list(repo: &Path, range: &str) -> Vec<CommitRef> {
    g(repo)
        .args(["log", &format!("-n{}", MAX_LISTED), "--format=%H%x1f%h%x1f%s", range])
        .ok_text()
        .await
        .map(|out| {
            out.lines()
                .filter_map(|l| {
                    let mut it = l.split('\x1f');
                    Some(CommitRef { oid: it.next()?.to_string(), short: it.next()?.to_string(), subject: it.next().unwrap_or("").to_string(), touches: Vec::new() })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn name_only(repo: &Path, args: &[&str]) -> BTreeSet<String> {
    g(repo)
        .args(args.iter().copied())
        .ok_text()
        .await
        .map(|out| out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// Tracked paths with uncommitted changes, and untracked paths.
async fn dirty_sets(repo: &Path) -> (Vec<String>, Vec<String>) {
    let out = GitCmd::at(repo)
        .top_flag("--no-optional-locks")
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=none"])
        .run()
        .await;
    let Ok(out) = out else { return (Vec::new(), Vec::new()) };
    let parsed = git_status::parse_porcelain_v2(&out.stdout);
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for e in parsed.entries {
        if e.kind == "untracked" {
            untracked.push(e.path);
        } else if !e.is_submodule {
            tracked.push(e.path);
        }
    }
    (tracked, untracked)
}

async fn fetch_remote(repo: &Path, remote: &str, extra: &[&str]) -> Result<(), String> {
    let mut args = vec!["fetch", "--quiet", "--no-recurse-submodules", "--", remote];
    args.extend_from_slice(extra);
    let out = g(repo).args(args).timeout(NET_TIMEOUT).run().await.map_err(|e| e.to_string())?;
    if out.ok() {
        Ok(())
    } else {
        Err(out.stderr.trim().lines().last().unwrap_or("fetch failed").to_string())
    }
}

/// Everything the planner needs. `fetch` is the only network step; the run
/// gathers again without it and compares oids with the plan.
pub async fn gather_facts(repo_path: &str, fetch: bool) -> Result<SyncFacts, AppError> {
    let repo = PathBuf::from(repo_path);
    let branch = g(&repo).args(["symbolic-ref", "-q", "--short", "HEAD"]).ok_text().await.unwrap_or_default();
    let upstream_ref = g(&repo)
        .args(["rev-parse", "--symbolic-full-name", "@{u}"])
        .ok_text()
        .await
        .filter(|s| s.starts_with("refs/remotes/"))
        .ok_or_else(|| AppError::Command("This branch has no upstream to sync with.".into()))?;
    let upstream = upstream_ref.trim_start_matches("refs/remotes/").to_string();
    let remote = upstream.split('/').next().unwrap_or("origin").to_string();
    crate::git_exec::validate_remote_name(&remote)?;

    // Every fetch runs concurrently: a slow submodule remote should not
    // serialise six timeouts.
    let subs_listed = crate::submodules::list_submodules(repo_path).await?;
    let mut fetch_errors: BTreeMap<String, String> = BTreeMap::new();
    if fetch {
        let mut tasks = Vec::new();
        {
            let r = repo.clone();
            let rem = remote.clone();
            tasks.push(tokio::spawn(async move { (String::new(), fetch_remote(&r, &rem, &[]).await) }));
        }
        for s in subs_listed.iter().filter(|s| s.listed && s.initialised && s.gitdir_valid) {
            let sub = repo.join(&s.path);
            let path = s.path.clone();
            let rem = s.upstream_remote.clone().unwrap_or_else(|| "origin".into());
            tasks.push(tokio::spawn(async move { (path, fetch_remote(&sub, &rem, &[]).await) }));
        }
        for t in tasks {
            if let Ok((path, Err(e))) = t.await {
                if path.is_empty() {
                    return Err(AppError::Command(format!("Fetching {} failed: {}", remote, e)));
                }
                fetch_errors.insert(path, e);
            }
        }
    }

    let head = oid(&repo, "HEAD").await.ok_or_else(|| AppError::Command("No commits yet".into()))?;
    let upstream_oid = oid(&repo, &upstream_ref).await.ok_or_else(|| AppError::Command(format!("{} does not exist locally", upstream)))?;
    let base_oid = g(&repo).args(["merge-base", "HEAD", &upstream_ref]).ok_text().await.unwrap_or_default();
    let counts = g(&repo).args(["rev-list", "--left-right", "--count", &format!("HEAD...{}", upstream_ref)]).ok_text().await.unwrap_or_default();
    let mut it = counts.split_whitespace();
    let ahead: usize = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let behind: usize = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut own_commits = commit_list(&repo, &format!("{}..HEAD", upstream_ref)).await;
    let incoming = commit_list(&repo, &format!("HEAD..{}", upstream_ref)).await;
    let upstream_changed = name_only(&repo, &["diff", "--name-only", "--ignore-submodules=none", &format!("HEAD...{}", upstream_ref)]).await;
    let upstream_added = name_only(&repo, &["diff", "--name-only", "--diff-filter=A", "--ignore-submodules=none", &format!("HEAD...{}", upstream_ref)]).await;
    let mine_changed = name_only(&repo, &["log", "--name-only", "--format=", "--ignore-submodules=none", &format!("{}..HEAD", upstream_ref)]).await;
    let (dirty_tracked, untracked) = dirty_sets(&repo).await;
    let file_overlap: Vec<String> = mine_changed.intersection(&upstream_changed).cloned().collect();
    let dirty_overlap: Vec<String> = dirty_tracked.iter().filter(|p| upstream_changed.contains(*p)).cloned().collect();
    let untracked_overlap: Vec<String> = untracked.iter().filter(|p| upstream_added.contains(*p)).cloned().collect();
    let is_submodule_itself = g(&repo).args(["rev-parse", "--show-superproject-working-tree"]).ok_text().await.is_some_and(|s| !s.is_empty());

    let mut submodules = Vec::new();
    let mut skipped = Vec::new();
    for s in &subs_listed {
        if !s.listed {
            skipped.push(Skipped { path: s.path.clone(), why: "unmapped".into() });
            continue;
        }
        if !s.initialised {
            skipped.push(Skipped { path: s.path.clone(), why: "not-initialised".into() });
            continue;
        }
        let sub = repo.join(&s.path);
        let recorded = s.recorded.clone();
        let target = oid(&repo, &format!("{}:{}", upstream_ref, s.path)).await;
        let base_link = if base_oid.is_empty() { None } else { oid(&repo, &format!("{}:{}", base_oid, s.path)).await };
        let moving: Vec<String> = g(&repo)
            .args(["log", "--format=%H", &format!("{}..HEAD", upstream_ref), "--", &s.path])
            .ok_text()
            .await
            .map(|o| o.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();
        for c in own_commits.iter_mut() {
            if moving.iter().any(|m| m == &c.oid) {
                c.touches.push(s.path.clone());
            }
        }
        let mut f = SubFacts {
            path: s.path.clone(),
            name: s.name.clone(),
            mapped: true,
            populated: true,
            gitdir_valid: s.gitdir_valid,
            head: s.actual.clone(),
            recorded: recorded.clone(),
            target: target.clone(),
            base_link: base_link.clone(),
            target_present: false,
            head_is_ancestor_of_target: false,
            target_is_ancestor_of_head: false,
            branch: s.own_branch.clone(),
            upstream: s.own_upstream.clone(),
            remote: s.upstream_remote.clone().or_else(|| Some("origin".into())),
            remote_head_branch: None,
            tip_ahead_of_target: false,
            own_commits_moving_gitlink: moving.len(),
            my_commits_move_gitlink: base_link.as_deref().is_some_and(|b| !same_oid(b, &recorded)),
            upstream_moves_gitlink: match (&base_link, &target) {
                (Some(b), Some(t)) => !same_oid(b, t),
                (None, Some(_)) | (Some(_), None) => true,
                _ => false,
            },
            dirty_tracked: s.dirty_tracked,
            untracked: s.dirty_untracked,
            dirty_overlap: Vec::new(),
            conflicted: s.conflicted,
            operation: s.operation.clone(),
            fetch_error: fetch_errors.get(&s.path).cloned(),
            has_nested: sub.join(".gitmodules").exists(),
        };
        if !s.gitdir_valid {
            submodules.push(f);
            continue;
        }
        if let Some(t) = &target {
            let rem = f.remote.clone().unwrap_or_else(|| "origin".into());
            let mut present = g(&sub).args(["cat-file", "-e", &format!("{}^{{commit}}", t)]).run().await.map(|o| o.ok()).unwrap_or(false);
            if !present && fetch && f.fetch_error.is_none() {
                // What `git submodule update` does: fetch the exact commit.
                if fetch_remote(&sub, &rem, &[t]).await.is_ok() {
                    present = g(&sub).args(["cat-file", "-e", &format!("{}^{{commit}}", t)]).run().await.map(|o| o.ok()).unwrap_or(false);
                }
            }
            f.target_present = present;
            if present {
                if let Some(h) = &f.head {
                    f.head_is_ancestor_of_target = is_ancestor(&sub, h, t).await;
                    f.target_is_ancestor_of_head = is_ancestor(&sub, t, h).await;
                    let (dirty, _) = dirty_sets(&sub).await;
                    let changed = name_only(&sub, &["diff", "--name-only", &format!("{}...{}", h, t)]).await;
                    f.dirty_overlap = dirty.into_iter().filter(|p| changed.contains(p)).collect();
                }
                let remote_head = g(&sub).args(["symbolic-ref", "-q", "--short", &format!("refs/remotes/{}/HEAD", rem)]).ok_text().await;
                f.remote_head_branch = remote_head.as_deref().and_then(|r| r.split_once('/')).map(|(_, b)| b.to_string());
                let tip = match &f.upstream {
                    Some(u) => oid(&sub, &format!("refs/remotes/{}", u)).await,
                    None => match &remote_head {
                        Some(r) => oid(&sub, &format!("refs/remotes/{}", r)).await,
                        None => None,
                    },
                };
                if let Some(tip) = tip {
                    f.tip_ahead_of_target = !same_oid(&tip, t) && is_ancestor(&sub, t, &tip).await;
                }
            }
        }
        submodules.push(f);
    }

    Ok(SyncFacts {
        superproject: SuperFacts {
            branch,
            upstream,
            upstream_ref,
            remote,
            head,
            upstream_oid,
            base_oid,
            ahead,
            behind,
            own_commits,
            incoming,
            file_overlap,
            dirty_tracked,
            untracked: untracked.len(),
            dirty_overlap,
            untracked_overlap,
            is_submodule_itself,
        },
        submodules,
        skipped,
    })
}

// ---------------------------------------------------------------------------
// Assess
// ---------------------------------------------------------------------------

async fn facts_for_check(repo_path: &str) -> Result<(RepoStatus, StatusFacts), AppError> {
    let status = git_status::repo_status(repo_path).await?;
    let facts = StatusFacts::from(&status);
    Ok((status, facts))
}

/// Fetch, measure, decide — nothing is written.
/// `fetch` is false only when re-planning with a different `stash` choice
/// right after an assessment: the answer is then local and instant.
pub async fn sync_plan(repo_path: &str, stash: bool, fetch: bool) -> Result<SyncPlan, AppError> {
    let (_status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::Sync, &f) {
        return Ok(SyncPlan { refusal: Some(r.clone()), summary: r.message, ..Default::default() });
    }
    let facts = gather_facts(repo_path, fetch).await?;
    Ok(plan(&facts, stash))
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubOutcome {
    pub path: String,
    pub action: String,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub recorded_after: Option<String>,
    pub ok: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncOutcome {
    /// done | needs-user | aborted | failed
    pub phase: String,
    pub needs_user: Option<NeedsUser>,
    pub backups: Vec<BackupRef>,
    pub bundles: Vec<String>,
    pub head_before: String,
    pub head_after: String,
    pub incoming: usize,
    pub own_kept: Vec<CommitRef>,
    pub own_dropped: Vec<CommitRef>,
    pub stashed: bool,
    pub stash_left: bool,
    pub submodules: Vec<SubOutcome>,
    pub verified: Verified,
    pub unrecorded_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BackupRef {
    pub repo: String,
    pub branch: String,
    pub oid: String,
    pub recovery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Verified {
    pub behind_zero: bool,
    pub own_on_top: bool,
    pub dirty_paths_unchanged: bool,
    pub untracked_unchanged: bool,
    pub submodules_aligned: bool,
    pub no_conflicts_left: bool,
}

pub struct RunOptions {
    pub stash: bool,
    pub bundles: bool,
    /// From the plan; empty skips the "upstream moved since you assessed" check (tests only).
    pub fingerprint: Vec<(String, String)>,
}

pub(crate) fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S").to_string()
}

pub(crate) async fn make_backup(repo: &Path, name: &str) -> Result<String, AppError> {
    let out = g(repo).args(["branch", "--", name]).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!(
            "Could not create the backup branch {} in {}: {}. Nothing was changed.",
            name,
            repo.display(),
            out.stderr.trim()
        )));
    }
    oid(repo, "HEAD").await.ok_or_else(|| AppError::Command("no HEAD".into()))
}

async fn rebase_in_progress(repo: &Path) -> bool {
    match GitCmd::at(repo).args(["rev-parse", "--path-format=absolute", "--git-path", "rebase-merge"]).ok_text().await {
        Some(p) => Path::new(p.trim()).is_dir(),
        None => false,
    }
}

pub(crate) async fn unmerged(repo: &Path) -> Vec<git_status::ChangeEntry> {
    let Ok(out) = GitCmd::at(repo)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=no", "--ignore-submodules=none"])
        .run()
        .await
    else {
        return Vec::new();
    };
    git_status::parse_porcelain_v2(&out.stdout)
        .entries
        .into_iter()
        .filter(|e| e.kind == "conflicted")
        .collect()
}

/// The reflog subject a rebase writes when it cannot re-apply its autostash
/// (`git stash store -m autostash`): exactly this word, no branch prefix. A
/// hand-made stash whose message mentions the word is "On main: …autostash…"
/// and is not one.
pub(crate) const AUTOSTASH_SUBJECT: &str = "autostash";

pub(crate) fn is_autostash_subject(subject: &str) -> bool {
    subject.trim() == AUTOSTASH_SUBJECT
}

/// Any entry in the stash list that a rebase's failed autostash pop stored.
pub(crate) async fn has_autostash_entry(repo: &Path) -> bool {
    g(repo)
        .args(["stash", "list", "--format=%gs"])
        .ok_text()
        .await
        .is_some_and(|s| s.lines().any(is_autostash_subject))
}

/// The newest stash entry is one a rebase's failed autostash pop stored — the
/// entry conflicts left behind with no operation in progress came from.
pub(crate) async fn autostash_on_top(repo: &Path) -> bool {
    g(repo)
        .args(["stash", "list", "-n1", "--format=%gs"])
        .ok_text()
        .await
        .is_some_and(|s| is_autostash_subject(&s))
}

/// The state machine's single step: called after the superproject rebase
/// started or resumed, and again by `sync_continue`. Returns when the sync is
/// done, needs the user, or failed.
async fn drive(repo: &Path, st: &mut SyncState, state_file: &Path) -> Result<(), AppError> {
    loop {
        if !rebase_in_progress(repo).await {
            break;
        }
        let conflicts = unmerged(repo).await;
        if conflicts.is_empty() {
            // Stopped with nothing unmerged: an untracked file in the way, a
            // hook, an LFS smudge failure. Hand over with git's words.
            st.needs_user = Some(NeedsUser {
                where_: "superproject".into(),
                paths: Vec::new(),
                hint: "The rebase stopped without a conflict to resolve. Read what git said on the Changes page, fix it, then Continue sync — or Abort sync.".into(),
            });
            st.phase = "rebasing".into();
            write_state_at(state_file, st)?;
            return Ok(());
        }
        let files: Vec<String> = conflicts.iter().filter(|c| !c.is_submodule).map(|c| c.path.clone()).collect();
        if !files.is_empty() {
            st.needs_user = Some(NeedsUser {
                where_: "superproject".into(),
                paths: files.clone(),
                hint: format!("Resolve {} on the Changes page (edit, then stage), then Continue sync.", files.join(", ")),
            });
            st.phase = "rebasing".into();
            write_state_at(state_file, st)?;
            return Ok(());
        }
        for c in conflicts.iter().filter(|c| c.is_submodule) {
            resolve_gitlink_stop(repo, st, state_file, c).await?;
            if st.needs_user.is_some() {
                return Ok(());
            }
        }
        let out = g(repo).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
        if !out.ok() && !rebase_in_progress(repo).await {
            return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
        }
    }
    // The rebase finished. A failed autostash pop leaves unmerged paths and
    // an "autostash" stash entry with no operation marker.
    let leftovers = unmerged(repo).await;
    if !leftovers.is_empty() || has_autostash_entry(repo).await {
        st.needs_user = Some(NeedsUser {
            where_: "autostash-conflict".into(),
            paths: leftovers.iter().map(|c| c.path.clone()).collect(),
            hint: "The rebase finished, but your stashed changes did not re-apply cleanly. Resolve and stage them, then Continue sync — or run `git reset --hard` (your changes are still in stash@{0}) and `git stash pop` by hand.".into(),
        });
        st.phase = "aligning".into();
        write_state_at(state_file, st)?;
        return Ok(());
    }
    st.phase = "aligning".into();
    write_state_at(state_file, st)?;
    align_submodules(repo, st, state_file).await
}

/// A submodule-pointer stop: rebase that submodule's own commits onto the
/// commit upstream recorded, then record the result. Neither `--ours` nor
/// `--theirs` is ever right here.
async fn resolve_gitlink_stop(repo: &Path, st: &mut SyncState, state_file: &Path, c: &git_status::ChangeEntry) -> Result<(), AppError> {
    let path = c.path.clone();
    let hand_over = |st: &mut SyncState, hint: String| {
        st.needs_user = Some(NeedsUser { where_: format!("submodule:{}", path), paths: vec![path.clone()], hint });
    };
    if c.stage_oids.len() != 3 || c.stage_oids.iter().any(|o| o.is_empty() || o == "0000000000000000000000000000000000000000") || c.stage_modes.get(1) != Some(&"160000".to_string()) || c.stage_modes.get(2) != Some(&"160000".to_string()) {
        hand_over(st, format!("The submodule {} was added or removed on one side; decide by hand, stage it, then Continue sync.", path));
        return write_state_at(state_file, st);
    }
    let ours = c.stage_oids[1].clone(); // upstream's recorded commit (during a rebase, "ours" is the onto side)
    let theirs = c.stage_oids[2].clone(); // my commit's recorded commit
    let sub = repo.join(&path);
    let head = oid(&sub, "HEAD").await.unwrap_or_default();
    let ss = st.subs.iter_mut().find(|s| s.path == path);
    let Some(ss) = ss else {
        hand_over(st, format!("The submodule {} was not part of the plan; stage it by hand, then Continue sync.", path));
        return write_state_at(state_file, st);
    };
    let already_rebased = ss.rebased && ss.head_after.as_deref() == Some(head.as_str());
    if !same_oid(&head, &theirs) && !already_rebased {
        hand_over(st, format!(
            "{} is checked out at {} but your commit records {}. Put it where you want it (with your commits on top of {}), stage it, then Continue sync.",
            path, short(&head), short(&theirs), short(&ours)
        ));
        return write_state_at(state_file, st);
    }
    if !already_rebased {
        // Detached: rebase detached, re-attach afterwards when the branch was where it stood.
        let out = g(&sub).args(["rebase", "--merge", "--autostash", &ours]).timeout(NET_TIMEOUT).run().await?;
        if !out.ok() {
            ss.rebased = true;
            ss.note = "rebase stopped on a conflict inside it".into();
            hand_over(st, format!(
                "Rebasing {} onto {} stopped on a conflict inside it. Open it as its own repository on the Changes page, resolve and stage, then Continue sync.",
                path, short(&ours)
            ));
            return write_state_at(state_file, st);
        }
        if !unmerged(&sub).await.is_empty() || has_autostash_entry(&sub).await {
            ss.rebased = true;
            hand_over(st, format!(
                "{} was rebased, but its uncommitted changes did not re-apply cleanly. Resolve and stage them inside it, then Continue sync.",
                path
            ));
            st.needs_user.as_mut().unwrap().where_ = format!("submodule-autostash:{}", path);
            return write_state_at(state_file, st);
        }
        let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
        reattach(&sub, ss).await;
        ss.rebased = true;
        ss.head_after = Some(new_head.clone());
        ss.note = format!("rebased onto {}", short(&ours));
    }
    let out = g(repo).args(["add", "--", &path]).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!("Could not record {}: {}", path, out.stderr.trim())));
    }
    write_state_at(state_file, st)
}

/// After a detached rebase: the branch the submodule was on before (or, when
/// it was detached, the branch its remote's HEAD names) is moved to the new
/// HEAD and checked out — but only when that branch stood exactly where the
/// checkout was, or is already an ancestor of the new HEAD. A branch pointing
/// anywhere else is somebody's work and is left alone.
async fn reattach(sub: &Path, ss: &mut SubState) {
    let on_branch = g(sub).args(["symbolic-ref", "-q", "HEAD"]).ok_text().await.is_some();
    if on_branch {
        return;
    }
    let candidates: Vec<String> = ss.branch_before.iter().chain(ss.reattach_to.iter()).cloned().collect();
    for b in candidates {
        let Some(at) = oid(sub, &format!("refs/heads/{}", b)).await else { continue };
        if same_oid(&at, &ss.head_before) || is_ancestor(sub, &b, "HEAD").await {
            let _ = g(sub).args(["branch", "-f", &b, "HEAD"]).run().await;
            let _ = g(sub).args(["switch", "--quiet", &b]).run().await;
            ss.note = format!("{} (re-attached to '{}')", ss.note, b);
            return;
        }
    }
}

/// Step 5: bring every mapped, populated submodule to the commit the
/// superproject now records.
async fn align_submodules(repo: &Path, st: &mut SyncState, state_file: &Path) -> Result<(), AppError> {
    for i in 0..st.subs.len() {
        let path = st.subs[i].path.clone();
        let sub = repo.join(&path);
        let Some(recorded) = oid(repo, &format!("HEAD:{}", path)).await else { continue };
        let Some(head) = oid(&sub, "HEAD").await else { continue };
        st.subs[i].target = recorded.clone();
        if same_oid(&head, &recorded) {
            st.subs[i].aligned = true;
            if st.subs[i].note.is_empty() {
                st.subs[i].note = "already at the recorded commit".into();
            }
            continue;
        }
        if is_ancestor(&sub, &head, &recorded).await {
            let on_branch = g(&sub).args(["symbolic-ref", "-q", "HEAD"]).ok_text().await.is_some();
            let out = if on_branch {
                g(&sub).args(["merge", "--ff-only", "--no-edit", "--no-stat", "--", &recorded]).timeout(LOCAL_TIMEOUT).run().await?
            } else {
                g(&sub).args(["checkout", "--quiet", "--detach", &recorded]).timeout(LOCAL_TIMEOUT).run().await?
            };
            if out.ok() {
                st.subs[i].aligned = true;
                st.subs[i].head_after = Some(recorded.clone());
                st.subs[i].note = format!("fast-forwarded to {}{}", short(&recorded), if on_branch { "" } else { " (detached)" });
            } else {
                st.subs[i].note = format!("could not fast-forward: {}", out.stderr.trim());
            }
            continue;
        }
        if is_ancestor(&sub, &recorded, &head).await {
            st.subs[i].aligned = true;
            st.subs[i].head_after = Some(head.clone());
            st.subs[i].note = format!("your commits sit on top of {}; the superproject does not record them yet", short(&recorded));
            continue;
        }
        // Diverged and untouched by my commits: rebase its own commits onto the recorded target.
        let out = g(&sub).args(["rebase", "--merge", "--autostash", &recorded]).timeout(NET_TIMEOUT).run().await?;
        st.subs[i].rebased = true;
        if !out.ok() {
            st.needs_user = Some(NeedsUser {
                where_: format!("submodule:{}", path),
                paths: vec![path.clone()],
                hint: format!("Rebasing {} onto {} stopped on a conflict inside it. Open it as its own repository, resolve and stage, then Continue sync.", path, short(&recorded)),
            });
            return write_state_at(state_file, st);
        }
        if !unmerged(&sub).await.is_empty() || has_autostash_entry(&sub).await {
            st.needs_user = Some(NeedsUser {
                where_: format!("submodule-autostash:{}", path),
                paths: vec![path.clone()],
                hint: format!("{} was rebased, but its uncommitted changes did not re-apply cleanly. Resolve and stage them inside it, then Continue sync.", path),
            });
            return write_state_at(state_file, st);
        }
        let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
        let mut ss = st.subs[i].clone();
        reattach(&sub, &mut ss).await;
        st.subs[i] = ss;
        st.subs[i].aligned = true;
        st.subs[i].head_after = Some(new_head);
        st.subs[i].note = format!("your commits rebased onto {}; the superproject does not record them yet", short(&recorded));
    }
    st.phase = "done".into();
    write_state_at(state_file, st)
}

/// Where a sub's rebase is mid-flight.
async fn sub_rebase_in_progress(repo: &Path, st: &SyncState) -> Option<String> {
    for s in &st.subs {
        if rebase_in_progress(&repo.join(&s.path)).await {
            return Some(s.path.clone());
        }
    }
    None
}

fn recovery(repo: &Path, branch: &str) -> String {
    format!("git -C {} reset --hard {}", shell_quote(&repo.to_string_lossy()), branch)
}

pub(crate) fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':')) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

async fn finish(repo_path: &str, st: &SyncState, state_file: &Path) -> Result<OpResult, AppError> {
    let repo = PathBuf::from(repo_path);
    let outcome = build_outcome(&repo, st).await;
    let status = git_status::repo_status(repo_path).await?;
    let mut op = if st.phase == "done" {
        std::fs::remove_file(state_file).ok();
        let status = git_status::repo_status(repo_path).await?;
        let verified = outcome.verified.behind_zero && outcome.verified.own_on_top && outcome.verified.no_conflicts_left;
        let headline = if !verified {
            "Sync finished, but a check did not pass — read the details.".to_string()
        } else if outcome.own_kept.is_empty() {
            format!(
                "Synced: fast-forwarded to {} from {}.",
                plural(outcome.incoming, "new commit", "new commits"),
                st.plan.superproject.upstream
            )
        } else {
            format!(
                "Synced: {} on top of {} from {}.",
                plural(outcome.own_kept.len(), "commit of yours", "commits of yours"),
                plural(outcome.incoming, "new commit", "new commits"),
                st.plan.superproject.upstream
            )
        };

        let mut detail = String::new();
        if !outcome.own_dropped.is_empty() {
            detail.push_str(&format!(
                " {} of yours {} dropped because upstream already contained {}: {}.",
                plural(outcome.own_dropped.len(), "commit", "commits"),
                if outcome.own_dropped.len() == 1 { "was" } else { "were" },
                if outcome.own_dropped.len() == 1 { "it" } else { "them" },
                outcome.own_dropped.iter().map(|c| c.subject.clone()).collect::<Vec<_>>().join("; ")
            ));
        }
        if !outcome.unrecorded_pointers.is_empty() {
            detail.push_str(&format!(" Submodule pointers not yet recorded by this branch: {}. Use \"Record submodule pointers\" when you want a commit for them.", outcome.unrecorded_pointers.join(", ")));
        }
        if outcome.stashed {
            detail.push_str(" Your uncommitted changes were stashed and re-applied (staged changes come back unstaged).");
        }
        if !outcome.verified.dirty_paths_unchanged {
            detail.push_str(" The set of files with uncommitted changes differs from before the sync — check the file list.");
        }
        let mut r = OpResult::done(headline, detail.trim().to_string(), status);
        r.ok = verified;
        r
    } else if st.phase == "aborted" {
        std::fs::remove_file(state_file).ok();
        OpResult::done("Sync aborted.".to_string(), "Everything is back where it was, except what is named below.".to_string(), status)
    } else {
        let nu = st.needs_user.clone().unwrap_or(NeedsUser { where_: "superproject".into(), paths: vec![], hint: String::new() });
        let mut r = OpResult::done(
            match nu.where_.as_str() {
                "autostash-conflict" => "Sync paused: your stashed changes did not re-apply cleanly.".to_string(),
                w if w.starts_with("submodule") => format!("Sync paused inside {}.", w.split(':').nth(1).unwrap_or("a submodule")),
                _ => "Sync paused: conflicts to resolve.".to_string(),
            },
            nu.hint.clone(),
            status,
        );
        r.ok = false;
        r
    };
    op.sync = Some(outcome);
    Ok(op)
}

async fn build_outcome(repo: &Path, st: &SyncState) -> SyncOutcome {
    let head_after = oid(repo, "HEAD").await.unwrap_or_default();
    let own_after = commit_list(repo, &format!("{}..HEAD", st.upstream_ref)).await;
    let kept_subjects: BTreeSet<String> = own_after.iter().map(|c| c.subject.clone()).collect();
    let own_dropped: Vec<CommitRef> = st.plan.superproject.own_commits.iter().filter(|c| !kept_subjects.contains(&c.subject)).cloned().collect();
    let counts = g(repo).args(["rev-list", "--left-right", "--count", &format!("HEAD...{}", st.upstream_ref)]).ok_text().await.unwrap_or_default();
    let behind: usize = counts.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
    let (dirty_now, untracked_now) = dirty_sets(repo).await;
    let dirty_before = st.captures.get("").cloned().unwrap_or_default();
    let dirty_set_now: BTreeSet<String> = dirty_now.iter().cloned().collect();
    let dirty_set_before: BTreeSet<String> = dirty_before.iter().cloned().collect();
    let untracked_before = st.captures.get("untracked").map(|v| v.len()).unwrap_or(untracked_now.len());
    let mut backups = Vec::new();
    if let Some(b) = &st.backup {
        backups.push(BackupRef { repo: repo.to_string_lossy().to_string(), branch: b.clone(), oid: st.head_before.clone(), recovery: recovery(repo, b) });
    }
    let mut subs = Vec::new();
    let mut aligned_all = true;
    let mut unrecorded = Vec::new();
    for s in &st.subs {
        let sub = repo.join(&s.path);
        if let Some(b) = &s.backup {
            backups.push(BackupRef { repo: sub.to_string_lossy().to_string(), branch: b.clone(), oid: s.head_before.clone(), recovery: recovery(&sub, b) });
        }
        let recorded = oid(repo, &format!("HEAD:{}", s.path)).await;
        let head_now = oid(&sub, "HEAD").await;
        let ok = match (&recorded, &head_now) {
            (Some(r), Some(h)) => same_oid(r, h) || is_ancestor(&sub, r, h).await,
            _ => false,
        };
        if !ok {
            aligned_all = false;
        }
        if let (Some(r), Some(h)) = (&recorded, &head_now) {
            if !same_oid(r, h) && is_ancestor(&sub, r, h).await {
                unrecorded.push(s.path.clone());
            }
        }
        subs.push(SubOutcome {
            path: s.path.clone(),
            action: st.plan.submodules.iter().find(|p| p.path == s.path).map(|p| p.action.clone()).unwrap_or_default(),
            head_before: Some(s.head_before.clone()),
            head_after: head_now,
            recorded_after: recorded,
            ok,
            note: s.note.clone(),
        });
    }
    SyncOutcome {
        phase: match st.phase.as_str() {
            "done" => "done".into(),
            "aborted" => "aborted".into(),
            _ => "needs-user".into(),
        },
        needs_user: st.needs_user.clone(),
        backups,
        bundles: st.bundles.clone(),
        head_before: st.head_before.clone(),
        head_after,
        incoming: st.plan.superproject.behind,
        own_kept: own_after,
        own_dropped,
        stashed: st.stash && !dirty_before.is_empty(),
        stash_left: has_autostash_entry(repo).await,
        submodules: subs,
        verified: Verified {
            behind_zero: behind == 0,
            own_on_top: true,
            dirty_paths_unchanged: dirty_set_now == dirty_set_before,
            untracked_unchanged: untracked_now.len() == untracked_before,
            submodules_aligned: aligned_all,
            no_conflicts_left: unmerged(repo).await.is_empty() && !rebase_in_progress(repo).await,
        },
        unrecorded_pointers: unrecorded,
    }
}

async fn write_bundle(repo: &Path, dir: &Path, name: &str) -> Result<String, AppError> {
    std::fs::create_dir_all(dir)?;
    let final_path = dir.join(format!("{}.bundle", name));
    let tmp = dir.join(format!("{}.bundle.tmp", name));
    let out = g(repo).args(["bundle", "create", &tmp.to_string_lossy(), "--all"]).timeout(NET_TIMEOUT).run().await?;
    if !out.ok() {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::Command(format!("bundle failed: {}", out.stderr.trim())));
    }
    std::fs::rename(&tmp, &final_path)?;
    Ok(final_path.to_string_lossy().to_string())
}

/// The whole operation, from a plan the user approved.
pub async fn sync_run(repo_path: &str, opts: RunOptions) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::Sync, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    // No silent re-fetch: measure again and compare with what was approved.
    let facts = gather_facts(repo_path, false).await?;
    if !opts.fingerprint.is_empty() {
        let mut now = vec![(String::new(), facts.superproject.upstream_oid.clone())];
        for s in &facts.submodules {
            if let Some(t) = &s.target {
                now.push((s.path.clone(), t.clone()));
            }
        }
        if now != opts.fingerprint {
            return Ok(OpResult::refused(
                Refusal { code: "assess-again".into(), message: "Upstream moved since you assessed. Assess again, then sync.".into() },
                status,
            ));
        }
    }
    let plan = plan(&facts, opts.stash);
    if !plan.can_run {
        let msg = if plan.nothing_to_do { plan.summary.clone() } else { plan.blockers.join(" ") };
        return Ok(OpResult::refused(Refusal { code: if plan.nothing_to_do { "nothing-to-sync".into() } else { "sync-blocked".into() }, message: msg }, status));
    }

    let state_file = state_path(&repo).await?;
    let id = stamp();
    let head_before = facts.superproject.head.clone();
    let (dirty, untracked) = dirty_sets(&repo).await;
    let mut captures = BTreeMap::new();
    captures.insert(String::new(), dirty.clone());
    captures.insert("untracked".into(), untracked);
    let mut st = SyncState {
        version: STATE_VERSION,
        nonce: uuid::Uuid::new_v4().to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        phase: "preparing".into(),
        repo: repo_path.to_string(),
        upstream_ref: facts.superproject.upstream_ref.clone(),
        upstream_oid: facts.superproject.upstream_oid.clone(),
        head_before: head_before.clone(),
        stash: opts.stash && !dirty.is_empty(),
        backup: None,
        captures,
        subs: plan
            .submodules
            .iter()
            .filter(|p| p.action != "blocked")
            .map(|p| SubState {
                path: p.path.clone(),
                branch_before: p.branch.clone(),
                reattach_to: p.reattach_to.clone(),
                head_before: p.head.clone().unwrap_or_default(),
                backup: None,
                target: p.target.clone().unwrap_or_default(),
                rebased: false,
                head_after: None,
                aligned: false,
                note: String::new(),
            })
            .collect(),
        needs_user: None,
        plan: plan.clone(),
        bundles: Vec::new(),
    };
    write_state_at(&state_file, &st)?;

    // Backups first; a name collision fails closed.
    let backup = format!("{}{}", BACKUP_PREFIX, id);
    if let Err(e) = make_backup(&repo, &backup).await {
        let _ = std::fs::remove_file(&state_file);
        return Err(e);
    }
    st.backup = Some(backup.clone());
    for i in 0..st.subs.len() {
        let action = plan.submodules.iter().find(|p| p.path == st.subs[i].path).map(|p| p.action.clone()).unwrap_or_default();
        if action == "rebase" {
            let sub = repo.join(&st.subs[i].path);
            match make_backup(&sub, &backup).await {
                Ok(_) => st.subs[i].backup = Some(backup.clone()),
                Err(e) => {
                    let _ = g(&repo).args(["branch", "-D", &backup]).run().await;
                    let _ = std::fs::remove_file(&state_file);
                    return Err(e);
                }
            }
        }
    }
    if opts.bundles {
        if let Some(dir) = dirs::data_dir().map(|d| d.join("com.gitswitch.app").join("sync-backups").join(format!("{}-{}", crate::paths::base_name(repo_path), id))) {
            if let Ok(p) = write_bundle(&repo, &dir, "superproject").await {
                st.bundles.push(p);
            }
            for s in &st.subs {
                if s.backup.is_some() {
                    if let Ok(p) = write_bundle(&repo.join(&s.path), &dir, &s.path.replace('/', "__")).await {
                        st.bundles.push(p);
                    }
                }
            }
        }
    }
    st.phase = "rebasing".into();
    write_state_at(&state_file, &st)?;

    let mut args = vec!["rebase", "--merge"];
    args.push(if st.stash { "--autostash" } else { "--no-autostash" });
    args.push(&st.upstream_ref);
    let out = g(&repo).args(args).timeout(NET_TIMEOUT).run().await?;
    if !out.ok() && !rebase_in_progress(&repo).await {
        // Refused before starting (a hook, an untracked file): nothing to undo but the backup branches.
        st.phase = "aborted".into();
        write_state_at(&state_file, &st)?;
        let after = git_status::repo_status(repo_path).await?;
        let a = crate::git_advice::explain(crate::git_advice::GitOp::Rebase, &format!("{}\n{}", out.text(), out.stderr), &crate::git_ops::advice_ctx(&after));
        let _ = std::fs::remove_file(&state_file);
        let mut r = OpResult::failed(a, after);
        r.sync = Some(build_outcome(&repo, &st).await);
        return Ok(r);
    }
    drive(&repo, &mut st, &state_file).await?;
    finish(repo_path, &st, &state_file).await
}

/// After the user resolved and staged: continue the innermost rebase, then
/// resume the state machine.
pub async fn sync_continue(repo_path: &str) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::SyncContinue, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    let state_file = state_path(&repo).await?;
    let Some(mut st) = read_state(&repo).await else {
        return Ok(OpResult::refused(Refusal { code: "no-sync".into(), message: "No sync is paused here.".into() }, status));
    };
    if let Some(path) = sub_rebase_in_progress(&repo, &st).await {
        let sub = repo.join(&path);
        if !unmerged(&sub).await.is_empty() {
            let status = git_status::repo_status(repo_path).await?;
            return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: format!("{} still has conflicted files. Resolve and stage them inside it first.", path) }, status));
        }
        let out = g(&sub).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
        if !out.ok() && rebase_in_progress(&sub).await {
            let status = git_status::repo_status(repo_path).await?;
            return Ok(OpResult::refused(Refusal { code: "sub-rebase-stopped".into(), message: format!("Rebasing {} stopped again: {}", path, out.stderr.trim()) }, status));
        }
        if let Some(ss) = st.subs.iter_mut().find(|s| s.path == path) {
            let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
            reattach(&sub, ss).await;
            ss.head_after = Some(new_head);
            ss.rebased = true;
            if ss.note.is_empty() || ss.note.contains("stopped") {
                ss.note = "rebased (conflicts resolved by you)".into();
            }
        }
        st.needs_user = None;
        if rebase_in_progress(&repo).await {
            // Back in the superproject's stop: record the submodule and carry on.
            let out = g(&repo).args(["add", "--", &path]).run().await?;
            if !out.ok() {
                return Err(AppError::Command(format!("Could not record {}: {}", path, out.stderr.trim())));
            }
            let out = g(&repo).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
            if !out.ok() && !rebase_in_progress(&repo).await {
                return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
            }
            drive(&repo, &mut st, &state_file).await?;
        } else {
            align_submodules(&repo, &mut st, &state_file).await?;
        }
        return finish(repo_path, &st, &state_file).await;
    }
    if st.needs_user.as_ref().map(|n| n.where_.as_str()) == Some("autostash-conflict") {
        if !unmerged(&repo).await.is_empty() {
            let status = git_status::repo_status(repo_path).await?;
            return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: "Resolve and stage the conflicted files first.".into() }, status));
        }
        // The user staged the resolution of the stash. The leftover stash entry is theirs to drop; carry on.
        st.needs_user = None;
        align_submodules(&repo, &mut st, &state_file).await?;
        return finish(repo_path, &st, &state_file).await;
    }
    if rebase_in_progress(&repo).await {
        if !unmerged(&repo).await.iter().any(|c| !c.is_submodule) {
            let out = g(&repo).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
            if !out.ok() && !rebase_in_progress(&repo).await {
                return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
            }
            st.needs_user = None;
            drive(&repo, &mut st, &state_file).await?;
            return finish(repo_path, &st, &state_file).await;
        }
        let status = git_status::repo_status(repo_path).await?;
        return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: "Resolve and stage every conflicted file first.".into() }, status));
    }
    // Finished or aborted outside the app.
    st.needs_user = None;
    if oid(&repo, "HEAD").await.as_deref() == Some(st.head_before.as_str()) {
        st.phase = "aborted".into();
        return finish(repo_path, &st, &state_file).await;
    }
    align_submodules(&repo, &mut st, &state_file).await?;
    finish(repo_path, &st, &state_file).await
}

/// Undo: inner rebase first, then the superproject's (git restores the
/// autostash), then every submodule the sync rebased goes back to its
/// pre-sync tip — only when clean, never at the cost of uncommitted work.
pub async fn sync_abort(repo_path: &str) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::SyncAbort, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    let state_file = state_path(&repo).await?;
    let Some(mut st) = read_state(&repo).await else {
        return Ok(OpResult::refused(Refusal { code: "no-sync".into(), message: "No sync is paused here.".into() }, status));
    };
    let mut notes: Vec<String> = Vec::new();
    for s in st.subs.iter_mut() {
        let sub = repo.join(&s.path);
        if rebase_in_progress(&sub).await {
            let _ = g(&sub).args(["rebase", "--abort"]).timeout(LOCAL_TIMEOUT).run().await;
        }
        if s.rebased {
            let (dirty, _) = dirty_sets(&sub).await;
            if dirty.is_empty() && unmerged(&sub).await.is_empty() {
                let target = s.head_before.clone();
                let ok = match &s.branch_before {
                    Some(b) => g(&sub).args(["switch", "--quiet", "-C", b, &target]).run().await.map(|o| o.ok()).unwrap_or(false),
                    None => g(&sub).args(["checkout", "--quiet", "--detach", &target]).run().await.map(|o| o.ok()).unwrap_or(false),
                };
                s.note = if ok { format!("put back at {}", short(&target)) } else { format!("could not be put back; its pre-sync tip is branch {}", s.backup.clone().unwrap_or_default()) };
            } else {
                s.note = format!("left as rebased because it has uncommitted changes; its pre-sync tip is branch {}", s.backup.clone().unwrap_or_default());
            }
            notes.push(format!("{}: {}", s.path, s.note));
        }
    }
    if rebase_in_progress(&repo).await {
        let out = g(&repo).args(["rebase", "--abort"]).timeout(LOCAL_TIMEOUT).run().await?;
        if !out.ok() {
            notes.push(format!("the superproject rebase could not be aborted: {}", out.stderr.trim()));
        }
    } else if oid(&repo, "HEAD").await.as_deref() != Some(st.head_before.as_str()) {
        let (dirty, _) = dirty_sets(&repo).await;
        if dirty.is_empty() && unmerged(&repo).await.is_empty() && !has_autostash_entry(&repo).await {
            let _ = g(&repo).args(["reset", "--hard", &st.head_before]).timeout(LOCAL_TIMEOUT).run().await;
            notes.push(format!("the superproject was put back at {}", short(&st.head_before)));
        } else {
            notes.push(format!("the superproject was left as rebased because it has uncommitted changes; its pre-sync tip is branch {}", st.backup.clone().unwrap_or_default()));
        }
    }
    st.phase = "aborted".into();
    st.needs_user = None;
    let mut r = finish(repo_path, &st, &state_file).await?;
    if !notes.is_empty() {
        r.detail = notes.join(" ");
    }
    Ok(r)
}

/// One commit recording every submodule whose checkout sits ahead of what
/// the branch records — offered, never automatic.
pub async fn record_pointers(repo_path: &str) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    // Nothing is staged yet, so the commit pre-check is run as if the pointers
    // it is about to stage already were: every other rule (paused sync,
    // conflicts, identity, an operation in progress) applies unchanged.
    let repo = PathBuf::from(repo_path);
    let subs = crate::submodules::list_submodules(repo_path).await?;
    let mut paths = Vec::new();
    for s in subs.iter().filter(|s| s.initialised && s.listed) {
        if let Some(a) = &s.actual {
            if !same_oid(a, &s.recorded) && is_ancestor(&repo.join(&s.path), &s.recorded, a).await {
                paths.push(s.path.clone());
            }
        }
    }
    let mut as_if_staged = f.clone();
    as_if_staged.staged += paths.len().max(1); // permission first, content second
    if let Some(r) = check(&Intent::Commit { amend: false, message_empty: false }, &as_if_staged) {
        return Ok(OpResult::refused(r, status));
    }
    if paths.is_empty() {
        return Ok(OpResult::refused(Refusal { code: "nothing-to-record".into(), message: "Every submodule is at the commit this branch records.".into() }, status));
    }
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    g(&repo).args(args).timeout(LOCAL_TIMEOUT).text().await?;
    let msg = format!("Record submodule pointers: {}", paths.join(", "));
    g(&repo).args(["commit", "--quiet", "--only", "-m", &msg, "--"]).args(paths.iter().map(String::as_str)).timeout(LOCAL_TIMEOUT).text().await?;
    let after = git_status::repo_status(repo_path).await?;
    Ok(OpResult::done(format!("Recorded {} submodule pointer(s).", paths.len()), msg, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sup() -> SuperFacts {
        SuperFacts {
            branch: "main".into(),
            upstream: "origin/main".into(),
            upstream_ref: "refs/remotes/origin/main".into(),
            remote: "origin".into(),
            head: "h".repeat(40),
            upstream_oid: "u".repeat(40),
            base_oid: "b".repeat(40),
            ahead: 2,
            behind: 3,
            own_commits: vec![CommitRef { oid: "1".repeat(40), short: "1111111".into(), subject: "seal work".into(), touches: vec!["A".into()] }],
            incoming: vec![],
            ..Default::default()
        }
    }
    fn sub(path: &str) -> SubFacts {
        SubFacts {
            path: path.into(),
            mapped: true,
            populated: true,
            gitdir_valid: true,
            head: Some("r".repeat(40)),
            recorded: "r".repeat(40),
            target: Some("t".repeat(40)),
            base_link: Some("o".repeat(40)),
            target_present: true,
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            remote: Some("origin".into()),
            remote_head_branch: Some("main".into()),
            my_commits_move_gitlink: true,
            upstream_moves_gitlink: true,
            own_commits_moving_gitlink: 1,
            ..Default::default()
        }
    }
    fn facts(subs: Vec<SubFacts>) -> SyncFacts {
        SyncFacts { superproject: sup(), submodules: subs, skipped: vec![] }
    }

    #[test]
    fn a_diverged_submodule_is_rebased_onto_the_recorded_target_at_the_stop() {
        let p = plan(&facts(vec![sub("A")]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        let a = &p.submodules[0];
        assert_eq!(a.action, "rebase");
        assert_eq!(a.rebase_onto.as_deref(), Some("t".repeat(40).as_str()));
        assert!(a.predicted_gitlink_conflict);
        assert!(p.summary.contains("rebasing the submodules"));
        assert_eq!(p.fingerprint[0].1, "u".repeat(40));
        assert_eq!(p.fingerprint[1], ("A".to_string(), "t".repeat(40)));
    }

    #[test]
    fn every_ambiguous_state_is_a_named_blocker() {
        // Checked out at an unrecorded commit while the pointer moves.
        let mut a = sub("A");
        a.head = Some("x".repeat(40));
        let p = plan(&facts(vec![a]), true);
        assert!(!p.can_run);
        assert!(p.blockers[0].contains("checked out at xxxxxxx but this branch records"));
        // Two own commits moving the pointer.
        let mut a = sub("A");
        a.own_commits_moving_gitlink = 2;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("2 of your commits move"));
        // Upstream removed it.
        let mut a = sub("A");
        a.target = None;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("upstream removed"));
        // Target unreachable after a failed fetch.
        let mut a = sub("A");
        a.target_present = false;
        a.fetch_error = Some("could not read".into());
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("fetching failed"));
        // An operation inside it.
        let mut a = sub("A");
        a.operation = Some("rebase".into());
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("in progress inside"));
        // Dirty files inside it that the rebase would touch.
        let mut a = sub("A");
        a.dirty_overlap = vec!["A.txt".into()];
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("collide"));
        // A dead gitfile.
        let mut a = sub("A");
        a.gitdir_valid = false;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("no longer exists"));
    }

    #[test]
    fn stashing_is_the_users_choice_and_overlaps_block_it() {
        let mut s = sup();
        s.dirty_tracked = vec!["README.md".into()];
        let f = SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] };
        let p = plan(&f, false);
        assert!(p.blockers[0].contains("Turn on stashing"));
        let p = plan(&f, true);
        assert!(p.can_run);
        assert!(p.superproject.will_stash);
        s.dirty_overlap = vec!["README.md".into()];
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] }, true);
        assert!(p.blockers[0].contains("collide with incoming"));
        s.dirty_overlap.clear();
        s.untracked_overlap = vec!["new.txt".into()];
        let p = plan(&SyncFacts { superproject: s, submodules: vec![], skipped: vec![] }, true);
        assert!(p.blockers[0].contains("Untracked file would be overwritten"));
    }

    #[test]
    fn fast_forward_ahead_and_none_are_told_apart() {
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        let p = plan(&facts(vec![b.clone()]), true);
        assert_eq!(p.submodules[0].action, "fast-forward");
        let mut c = b.clone();
        c.head_is_ancestor_of_target = false;
        c.target_is_ancestor_of_head = true;
        let p = plan(&facts(vec![c]), true);
        assert_eq!(p.submodules[0].action, "ahead");
        assert!(p.submodules[0].reason.contains("does not record them yet"));
        let mut d = b.clone();
        d.head = d.target.clone();
        d.recorded = d.target.clone().unwrap();
        d.head_is_ancestor_of_target = false;
        let p = plan(&facts(vec![d]), true);
        assert_eq!(p.submodules[0].action, "none");
    }

    #[test]
    fn nothing_to_do_is_said_plainly_and_the_remote_tip_note_is_informational() {
        let mut s = sup();
        s.behind = 0;
        s.ahead = 0;
        let mut a = sub("A");
        a.head = a.target.clone();
        a.recorded = a.target.clone().unwrap();
        a.my_commits_move_gitlink = false;
        a.upstream_moves_gitlink = false;
        a.own_commits_moving_gitlink = 0;
        a.tip_ahead_of_target = true;
        let p = plan(&SyncFacts { superproject: s, submodules: vec![a], skipped: vec![Skipped { path: "forge/x".into(), why: "unmapped".into() }] }, true);
        assert!(p.nothing_to_do);
        assert!(!p.can_run);
        assert!(p.summary.starts_with("Nothing to sync"));
        assert!(p.submodules[0].notes.iter().any(|n| n.contains("newer commits than upstream's superproject records")));
        assert_eq!(p.skipped[0].why, "unmapped");
    }

    #[test]
    fn state_round_trips() {
        let st = SyncState {
            version: STATE_VERSION,
            nonce: "n".into(),
            started_at: "t".into(),
            phase: "rebasing".into(),
            repo: "/r".into(),
            upstream_ref: "refs/remotes/origin/main".into(),
            upstream_oid: "u".into(),
            head_before: "h".into(),
            stash: true,
            backup: Some("gitswitch-before-sync-1".into()),
            captures: BTreeMap::new(),
            subs: vec![],
            needs_user: Some(NeedsUser { where_: "submodule:A".into(), paths: vec!["A".into()], hint: "x".into() }),
            plan: SyncPlan::default(),
            bundles: vec![],
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: SyncState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
        assert_eq!(shell_quote("/Users/me/with space"), "'/Users/me/with space'");
        assert_eq!(shell_quote("/Users/me/repo"), "/Users/me/repo");
    }
}
