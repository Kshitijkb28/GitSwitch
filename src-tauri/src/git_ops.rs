//! The everyday git loop: stage, unstage, discard, commit, pull, push,
//! submodules, abort.
//!
//! Every operation follows the same four phases:
//!
//! 1. **Snapshot** the repo (`git_status::repo_status`).
//! 2. **Check** preconditions against that snapshot — a pure function, so every
//!    refusal is unit-testable and nothing runs when we already know it fails.
//! 3. **Mutate** with one explicit git command. Never `--force`, never
//!    `--no-verify`, never a pathless `clean`, never an implicit pull strategy.
//! 4. **Verify** by re-reading the repo and reporting what *actually* changed —
//!    the same "proof, not a promise" idea as `git_history::fetch_repo`.

use crate::error::AppError;
use crate::git_advice::{self, Advice, AdviceCtx, GitOp};
use crate::git_exec::{GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT};
use crate::git_status::{self, RepoStatus};
use crate::sparse::SubmoduleReport;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A refusal carries a stable `code` (for the UI and for tests) and a sentence
/// the user can act on.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

pub(crate) fn refuse(code: &str, message: impl Into<String>) -> Refusal {
    Refusal {
        code: code.to_string(),
        message: message.into(),
    }
}

/// What the caller is asking for — enough for `check` to decide, and nothing more.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Stage,
    Unstage { conflicted_selected: bool },
    Discard { has_paths: bool, submodule_selected: bool, conflicted_selected: bool },
    Commit { amend: bool, message_empty: bool },
    Push,
    /// `autostash` lets a rebase pull run on a dirty tree (git stashes and re-applies).
    Pull { mode: PullMode, autostash: bool },
    Submodule,
    Abort,
    /// Finish the operation in progress once its conflicts are staged.
    Continue,
    /// Assess or run a sync (sync.rs): fetch, rebase with my commits on top.
    Sync,
    SyncContinue,
    SyncAbort,
    /// Branches, undo, reset, detach, revert, cherry-pick, conflict sides,
    /// discard-all (tree.rs). The variant carries the facts its check needs.
    Tree(crate::tree::TreeIntent),
    /// Stash push / apply / drop / restore one file (stash.rs).
    Stash(crate::stash::StashIntent),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullMode {
    FfOnly,
    Merge,
    Rebase,
}

impl PullMode {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "ff-only" => Ok(PullMode::FfOnly),
            "merge" => Ok(PullMode::Merge),
            "rebase" => Ok(PullMode::Rebase),
            other => Err(AppError::Command(format!(
                "'{}' is not a pull mode — expected ff-only, merge or rebase",
                other
            ))),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            PullMode::FfOnly => "fast-forward only",
            PullMode::Merge => "merge",
            PullMode::Rebase => "rebase",
        }
    }
}

/// The snapshot `check` is allowed to see. Flat and owned so the preconditions
/// can be tested without a repository.
#[derive(Debug, Clone, Default)]
pub struct StatusFacts {
    pub staged: usize,
    pub unstaged: usize,
    pub conflicted: usize,
    pub operation: Option<String>,
    pub abort_command: Option<String>,
    pub continue_command: Option<String>,
    /// A sync is paused here (or in this repository's superproject).
    pub sync_in_progress: bool,
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
    pub push_blocked: bool,
    pub push_locked: bool,
    pub push_reason: String,
    pub identity_matches: bool,
    pub identity_email: String,
    pub profile_email: Option<String>,
    pub email_scope: String,
    pub can_amend: bool,
    pub untracked: usize,
}

impl From<&RepoStatus> for StatusFacts {
    fn from(s: &RepoStatus) -> Self {
        StatusFacts {
            staged: s.staged_count,
            unstaged: s.unstaged_count,
            conflicted: s.conflicted_count,
            operation: s.operation.as_ref().map(|o| o.kind.clone()),
            abort_command: s.operation.as_ref().map(|o| o.abort_command.clone()),
            continue_command: s.operation.as_ref().and_then(|o| o.continue_command.clone()),
            sync_in_progress: s.sync.is_some(),
            detached: s.detached,
            unborn: s.unborn,
            upstream: s.upstream.clone(),
            push_blocked: s.push.blocked,
            push_locked: s.push.lock.locked,
            push_reason: s.push.reason.clone(),
            identity_matches: s.identity.matches_profile,
            identity_email: s.identity.email.clone(),
            profile_email: s.identity.profile_email.clone(),
            email_scope: s.identity.email_scope.clone(),
            can_amend: s.can_amend,
            untracked: s.untracked_count,
        }
    }
}

/// "There's a rebase in progress…" — the refusal every operation that would
/// fight an in-progress merge/rebase/cherry-pick shares.
pub(crate) fn busy(f: &StatusFacts, verb: &str) -> Option<Refusal> {
    f.operation.as_ref().map(|kind| {
        refuse(
            "operation-in-progress",
            format!(
                "There's a {} in progress. Finish or abort it before you {} ({}).",
                kind,
                verb,
                f.abort_command.clone().unwrap_or_default()
            ),
        )
    })
}

/// Everything that must be true before git runs. Pure.
pub fn check(intent: &Intent, f: &StatusFacts) -> Option<Refusal> {
    let busy = |verb: &str| -> Option<Refusal> { busy(f, verb) };

    // While a sync is paused, only the sync's own buttons and the resolution
    // work (stage, unstage, diff) make sense; everything else would fight it.
    let allowed_during_sync = match intent {
        Intent::Stage | Intent::Unstage { .. } | Intent::Discard { .. } | Intent::SyncContinue | Intent::SyncAbort => true,
        Intent::Tree(t) => t.allowed_during_sync(),
        Intent::Stash(s) => s.allowed_during_sync(),
        _ => false,
    };
    if f.sync_in_progress && !allowed_during_sync {
        return Some(refuse(
            "sync-in-progress",
            "A sync is paused here. Continue or abort it first (the Changes page shows both).",
        ));
    }

    match intent {
        Intent::Tree(t) => crate::tree::check_tree(t, f),
        Intent::Stash(s) => crate::stash::check_stash(s, f),

        Intent::Stage => None,

        Intent::Unstage { conflicted_selected } => {
            if *conflicted_selected {
                // `git reset HEAD -- <conflicted>` throws away the stage 1/2/3
                // entries and leaves the file looking resolved-to-HEAD, quietly
                // destroying the merge information.
                return Some(refuse(
                    "unmerged-paths",
                    "That file is part of a conflict. Unstaging it would throw away the merge information — resolve it instead.",
                ));
            }
            None
        }

        Intent::Discard {
            has_paths,
            submodule_selected,
            conflicted_selected,
        } => {
            if !has_paths {
                return Some(refuse(
                    "no-paths",
                    "Nothing selected. Discard only ever acts on files you pick.",
                ));
            }
            if *submodule_selected {
                return Some(refuse(
                    "submodule-selected",
                    "Those changes live inside a submodule. Open it as its own repository to discard them.",
                ));
            }
            // Inside a merge/rebase the stage-1/2/3 entries are the conflict
            // itself; outside one (a stash that failed to re-apply) restoring
            // the file to HEAD is exactly the way out, and the stash still
            // holds the change.
            if *conflicted_selected && f.operation.is_some() {
                return Some(refuse(
                    "unmerged-paths",
                    "Conflicted files can't be discarded here. Resolve the conflict, or abort the merge to undo all of it.",
                ));
            }
            None
        }

        Intent::Commit {
            amend,
            message_empty,
        } => {
            if f.conflicted > 0 {
                return Some(refuse(
                    "unmerged-paths",
                    format!(
                        "{} file(s) still have conflicts. Resolve and stage them first.",
                        f.conflicted
                    ),
                ));
            }
            if *amend {
                if !f.can_amend {
                    return Some(refuse(
                        "already-pushed",
                        "That commit is already on the remote. Amending it would need a force-push, which GitSwitch never does — make a new commit instead.",
                    ));
                }
                if let Some(r) = busy("amend") {
                    return Some(r);
                }
            } else if f.staged == 0 && f.operation.as_deref() != Some("merge") {
                // A merge is finished by exactly one commit, and that commit is
                // worth making even when nothing is staged: it records the second
                // parent. (Resolving every conflict in favour of our side leaves
                // the index identical to HEAD, which is exactly this case.)
                return Some(refuse(
                    "nothing-staged",
                    "Nothing is staged. Stage the files you want in this commit first.",
                ));
            }
            // Checked after "nothing staged" so the more fundamental blocker is
            // the one reported on an untouched repo.
            if *message_empty {
                return Some(refuse("empty-message", "A commit needs a message."));
            }
            if !f.identity_matches {
                let expected = f.profile_email.clone().unwrap_or_default();
                let where_from = if f.email_scope == "local" {
                    " This repo's own .git/config overrides the profile."
                } else {
                    ""
                };
                return Some(refuse(
                    "identity-mismatch",
                    format!(
                        "This folder should commit as {} but git is set to {}.{} Fix the identity before committing.",
                        expected, f.identity_email, where_from
                    ),
                ));
            }
            None
        }

        Intent::Push => {
            if f.push_locked {
                return Some(refuse("push-locked", f.push_reason.clone()));
            }
            if f.push_blocked {
                return Some(refuse("push-blocked", f.push_reason.clone()));
            }
            if f.unborn {
                return Some(refuse(
                    "unborn-head",
                    "There are no commits yet — nothing to push.",
                ));
            }
            if f.detached {
                return Some(refuse(
                    "detached-head",
                    "You're not on a branch, so there's no branch to push. Switch to one first.",
                ));
            }
            busy("push")
        }

        Intent::Pull { mode, autostash } => {
            if f.unborn {
                return Some(refuse(
                    "unborn-head",
                    "There are no commits yet. Pull needs a branch with history.",
                ));
            }
            if f.detached {
                return Some(refuse(
                    "detached-head",
                    "You're not on a branch. Switch to one before pulling.",
                ));
            }
            if f.upstream.is_none() {
                return Some(refuse(
                    "no-upstream",
                    "This branch isn't tracking a remote branch yet, so there's nothing to pull from.",
                ));
            }
            if let Some(r) = busy("pull") {
                return Some(r);
            }
            if *mode == PullMode::Rebase && !*autostash && (f.staged > 0 || f.unstaged > 0) {
                // --no-autostash is the default on purpose: a silent
                // stash/unstash cycle can fail halfway and leave work in a
                // stash nobody asked for. Autostash is an explicit choice, and
                // a failed re-apply is then reported, never hidden.
                return Some(refuse(
                    "dirty-tree",
                    "Rebase needs a clean working tree. Commit or stash your changes first, turn on autostash, or pull with merge instead.",
                ));
            }
            None
        }

        Intent::Submodule => busy("update submodules"),

        Intent::Abort => {
            if f.operation.is_none() {
                Some(refuse("nothing-to-abort", "There's no operation in progress."))
            } else {
                None
            }
        }

        Intent::Sync => {
            if f.unborn {
                return Some(refuse("unborn-head", "There are no commits yet — nothing to sync."));
            }
            if f.detached {
                return Some(refuse("detached-head", "You're not on a branch. Check one out first."));
            }
            if f.upstream.is_none() {
                return Some(refuse("no-upstream", "This branch isn't tracking a remote branch, so there is nothing to sync with."));
            }
            busy("sync")
        }

        Intent::SyncContinue | Intent::SyncAbort => {
            if f.sync_in_progress {
                None
            } else {
                Some(refuse("no-sync", "No sync is paused here."))
            }
        }

        Intent::Continue => {
            if f.operation.is_none() {
                return Some(refuse("nothing-to-continue", "There's no operation in progress."));
            }
            if f.continue_command.is_none() {
                return Some(refuse(
                    "cannot-continue",
                    format!("A {} can't be continued from here; finish it in a terminal or abort it.", f.operation.clone().unwrap_or_default()),
                ));
            }
            if f.conflicted > 0 {
                return Some(refuse(
                    "unmerged-paths",
                    format!(
                        "{} file{} still {} conflicts. Resolve and stage {} first.",
                        f.conflicted,
                        if f.conflicted == 1 { "" } else { "s" },
                        if f.conflicted == 1 { "has" } else { "have" },
                        if f.conflicted == 1 { "it" } else { "them" }
                    ),
                ));
            }
            None
        }
    }
}

/// NUL-delimited pathspecs for `--pathspec-from-file=-`, each marked
/// `:(literal)` so nothing in a filename is read as an option, a glob or
/// pathspec magic.
pub fn pathspec_stdin(paths: &[String]) -> Result<Vec<u8>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Command(
            "No files selected — this only ever acts on files you pick.".into(),
        ));
    }
    let mut out = Vec::new();
    for p in paths {
        validate_repo_path(p)?;
        out.extend_from_slice(b":(literal)");
        out.extend_from_slice(p.as_bytes());
        out.push(0);
    }
    Ok(out)
}

/// A path must stay inside the worktree. Paths come from our own status output,
/// so this is a guard against a malformed request, not a user typo.
pub fn validate_repo_path(p: &str) -> Result<(), AppError> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.starts_with('\\')
        || p.starts_with("~")
        || p.split(['/', '\\']).any(|seg| seg == "..")
        || p.contains('\0')
        // A Windows drive-absolute path.
        || (p.len() > 1 && p.as_bytes()[1] == b':');
    if bad {
        return Err(AppError::Command(format!(
            "'{}' isn't a path inside this repository",
            p
        )));
    }
    Ok(())
}

/// The argv for a push. Pure, so a test can prove what it can never contain.
pub fn build_push_args(remote: &str, local_branch: &str, remote_ref: &str, set_upstream: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["push".into(), "--porcelain".into()];
    if set_upstream {
        a.push("--set-upstream".into());
    }
    // `--` ends option parsing; both sides of the refspec are fully qualified so
    // nothing depends on push.default, the source can never be empty (which
    // would be a deletion), and there is no leading '+' (which would be a force).
    a.push("--".into());
    a.push(remote.to_string());
    a.push(format!("refs/heads/{}:{}", local_branch, remote_ref));
    a
}

/// The argv that integrates fetched work. One command per mode, named by the
/// same word the button uses.
pub fn build_integrate_args(mode: PullMode, upstream_ref: &str, autostash: bool) -> Vec<String> {
    match mode {
        PullMode::FfOnly => vec![
            "merge".into(),
            "--ff-only".into(),
            "--no-edit".into(),
            "--no-stat".into(),
            upstream_ref.to_string(),
        ],
        PullMode::Merge => vec![
            "merge".into(),
            "--no-edit".into(),
            "--no-stat".into(),
            upstream_ref.to_string(),
        ],
        PullMode::Rebase => vec![
            "rebase".into(),
            if autostash { "--autostash".into() } else { "--no-autostash".into() },
            upstream_ref.to_string(),
        ],
    }
}

fn signature_label(code: &str) -> &'static str {
    match code {
        "G" => "good",
        "U" => "good, untrusted",
        "X" => "expired",
        "Y" => "expired key",
        "R" => "revoked key",
        "B" => "bad",
        "E" => "unverifiable",
        _ => "unsigned",
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub signature: String,
    pub signed: bool,
    pub amended: bool,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct PushedRef {
    pub flag: String,
    pub summary: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PushOutcome {
    pub refs: Vec<PushedRef>,
    pub up_to_date: bool,
    pub new_upstream: Option<String>,
    pub remote: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PullOutcome {
    pub mode: String,
    pub head_before: String,
    pub head_after: String,
    pub fast_forward: bool,
    pub commits_pulled: usize,
    pub files_changed: usize,
    pub conflicts: Vec<String>,
    /// The literal command that puts the branch back where it was.
    pub recovery: Option<String>,
    /// The rebase finished but git could not re-apply the autostash cleanly:
    /// the conflicted files are in the tree and the stash entry is kept.
    pub autostash_conflict: bool,
    pub autostash_left: Option<String>,
}

/// What every operation returns: whether it worked, what to tell the user, and
/// the freshly re-read repository state.
#[derive(Debug, Serialize, Clone)]
pub struct OpResult {
    pub ok: bool,
    pub headline: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<Advice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<RepoStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<PushOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull: Option<PullOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodules: Option<SubmoduleReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfs: Option<crate::lfs::LfsStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<crate::sync::SyncOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<crate::tree::TreeOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stash: Option<crate::stash::StashOutcome>,
}

impl OpResult {
    pub(crate) fn blank() -> Self {
        OpResult {
            ok: true,
            headline: String::new(),
            detail: String::new(),
            advice: None,
            refusal: None,
            status: None,
            commit: None,
            push: None,
            pull: None,
            submodules: None,
            lfs: None,
            sync: None,
            tree: None,
            stash: None,
        }
    }

    /// A refusal decided before the repo could be read at all.
    pub(crate) fn refused_alone(r: Refusal) -> Self {
        OpResult {
            ok: false,
            headline: r.message.clone(),
            refusal: Some(r),
            ..OpResult::blank()
        }
    }

    pub(crate) fn refused(r: Refusal, status: RepoStatus) -> Self {
        OpResult {
            ok: false,
            headline: r.message.clone(),
            refusal: Some(r),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    pub(crate) fn done(headline: impl Into<String>, detail: impl Into<String>, status: RepoStatus) -> Self {
        OpResult {
            ok: true,
            headline: headline.into(),
            detail: detail.into(),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    pub(crate) fn failed(advice: Advice, status: RepoStatus) -> Self {
        OpResult {
            ok: false,
            headline: advice.headline.clone(),
            detail: advice.guidance.clone(),
            advice: Some(advice),
            status: Some(status),
            ..OpResult::blank()
        }
    }
}

pub(crate) async fn snapshot(repo_path: &str) -> Result<RepoStatus, AppError> {
    git_status::repo_status(repo_path).await
}

pub(crate) fn advice_ctx<'a>(status: &'a RepoStatus) -> AdviceCtx<'a> {
    AdviceCtx {
        key_path: None,
        url: status.push.remotes.first().map(|r| r.fetch_url.as_str()),
        branch: status.branch.as_deref(),
        remote: status.push.remotes.first().map(|r| r.name.as_str()),
    }
}

// --- Staging -------------------------------------------------------------

pub async fn stage(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Stage, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let conflicted_before: Vec<String> = before
        .entries
        .iter()
        .filter(|e| e.kind == "conflicted" && paths.contains(&e.path))
        .map(|e| e.path.clone())
        .collect();

    let stdin = pathspec_stdin(&paths)?;
    let out = GitCmd::at(repo_path)
        .args(["add", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin_bytes(stdin)
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    let staged_now = paths
        .iter()
        .filter(|p| {
            after
                .entries
                .iter()
                .any(|e| &&e.path == p && e.is_staged())
        })
        .count();
    let mut detail = format!("{} of {} selected file(s) are now staged.", staged_now, paths.len());
    if !conflicted_before.is_empty() {
        detail.push_str(&format!(
            " {} conflict(s) marked resolved.",
            conflicted_before.len()
        ));
    }
    Ok(OpResult::done("Staged.", detail, after))
}

/// Explicitly separate from `stage`, so "stage everything" can never happen by
/// an empty path list falling through.
pub async fn stage_all(repo_path: &str) -> Result<OpResult, AppError> {
    let out = GitCmd::at(repo_path)
        .args(["add", "--all", "--"])
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    Ok(OpResult::done(
        "Staged everything.",
        format!("{} file(s) staged.", after.staged_count),
        after,
    ))
}

pub async fn unstage(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let conflicted_selected = before
        .entries
        .iter()
        .any(|e| e.kind == "conflicted" && paths.contains(&e.path));
    if let Some(r) = check(
        &Intent::Unstage {
            conflicted_selected,
        },
        &StatusFacts::from(&before),
    ) {
        return Ok(OpResult::refused(r, before));
    }

    let stdin = pathspec_stdin(&paths)?;
    // With no commits yet there is no HEAD to reset against.
    let out = if before.unborn {
        GitCmd::at(repo_path)
            .args([
                "rm",
                "--cached",
                "--quiet",
                "-r",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .stdin_bytes(stdin)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?
    } else {
        GitCmd::at(repo_path)
            .args([
                "reset",
                "--quiet",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
                "HEAD",
            ])
            .stdin_bytes(stdin)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?
    };

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    let still_staged = paths
        .iter()
        .filter(|p| after.entries.iter().any(|e| &&e.path == p && e.is_staged()))
        .count();
    Ok(OpResult::done(
        "Unstaged.",
        format!(
            "{} file(s) moved out of the staging area.",
            paths.len() - still_staged
        ),
        after,
    ))
}

// --- Discard (destructive) ----------------------------------------------

/// Throw away changes to the named files. Tracked files go back to HEAD;
/// untracked files are deleted. Never pathless, never `-x` (which would take
/// .env and node_modules with it).
pub async fn discard(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let selected: Vec<&git_status::ChangeEntry> = before
        .entries
        .iter()
        .filter(|e| paths.contains(&e.path))
        .collect();

    let intent = Intent::Discard {
        has_paths: !paths.is_empty(),
        submodule_selected: selected.iter().any(|e| e.is_submodule),
        conflicted_selected: selected.iter().any(|e| e.kind == "conflicted"),
    };
    if let Some(r) = check(&intent, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }

    // A conflicted entry with no operation in progress (a stash that failed
    // to re-apply) is discarded like any tracked file: back to HEAD, and the
    // change is still in the stash. `check` refused it when an operation is
    // in progress, so reaching here means it is safe.
    let tracked: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "tracked" || e.kind == "conflicted")
        .map(|e| e.path.clone())
        .collect();
    let untracked: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "untracked")
        .map(|e| e.path.clone())
        .collect();
    // A staged-added file has no HEAD version: restoring it deletes it. The UI
    // needs to word that differently, so it is counted separately.
    let will_be_deleted: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "untracked" || e.staged == "A")
        .map(|e| e.path.clone())
        .collect();

    if tracked.is_empty() && untracked.is_empty() {
        return Ok(OpResult::refused(
            refuse(
                "no-paths",
                "Those files have no changes to discard — the list may be out of date. Refresh and try again.",
            ),
            before,
        ));
    }

    let mut errors: Vec<String> = Vec::new();

    if !tracked.is_empty() {
        let out = GitCmd::at(repo_path)
            .args([
                "restore",
                "--staged",
                "--worktree",
                "--source=HEAD",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .stdin_bytes(pathspec_stdin(&tracked)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }

    if !untracked.is_empty() {
        // `git clean` has no --pathspec-from-file, so these go in argv — still
        // `:(literal)`-marked, and still never with -x.
        let mut cmd = GitCmd::at(repo_path).args(["clean", "--force", "-d", "--"]);
        for p in &untracked {
            validate_repo_path(p)?;
            cmd = cmd.arg(format!(":(literal){}", p));
        }
        let out = cmd.timeout(LOCAL_TIMEOUT).run().await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }

    let after = snapshot(repo_path).await?;
    // clean exits 0 even when it matched nothing, so verify by re-reading.
    let not_discarded: Vec<String> = paths
        .iter()
        .filter(|p| after.entries.iter().any(|e| &&e.path == p))
        .cloned()
        .collect();

    if !errors.is_empty() {
        let a = git_advice::explain(GitOp::Checkout, &errors.join("\n"), &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    let mut detail = format!(
        "{} file(s) discarded{}.",
        paths.len() - not_discarded.len(),
        if will_be_deleted.is_empty() {
            String::new()
        } else {
            format!(", {} of them deleted", will_be_deleted.len())
        }
    );
    if !not_discarded.is_empty() {
        detail.push_str(&format!(
            " {} still show(s) changes: {}.",
            not_discarded.len(),
            not_discarded.join(", ")
        ));
    }
    Ok(OpResult::done("Discarded.", detail, after))
}

// --- Commit --------------------------------------------------------------

pub async fn commit(repo_path: &str, message: &str, amend: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let trimmed = message.trim();
    let intent = Intent::Commit {
        amend,
        message_empty: trimmed.is_empty(),
    };
    if let Some(r) = check(&intent, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }

    let mut cmd = GitCmd::at(repo_path).args(["commit", "--cleanup=whitespace"]);
    if amend {
        cmd = cmd.arg("--amend");
    }
    // The message goes on stdin: it can start with '-', span any length, and
    // never becomes an argument. No --no-verify: the identity guard must run.
    // No -S/--no-gpg-sign: signing is the profile's decision, already in config.
    let out = cmd
        .arg("--file=-")
        .stdin_bytes(trimmed.as_bytes().to_vec())
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let stderr = if out.stderr.is_empty() {
            out.text()
        } else {
            out.stderr.clone()
        };
        let a = git_advice::explain(GitOp::Commit, &stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    // Report the identity and signature that actually landed.
    let line = GitCmd::at(repo_path)
        .args(["log", "-1", "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%G?%x1f%s"])
        .ok_text()
        .await
        .unwrap_or_default();
    let f: Vec<&str> = line.split('\x1f').collect();
    let shortstat = GitCmd::at(repo_path)
        .args(["show", "--shortstat", "--format=", "HEAD"])
        .ok_text()
        .await
        .unwrap_or_default();
    let (files_changed, insertions, deletions) = parse_shortstat(&shortstat);
    let sig_code = f.get(4).copied().unwrap_or("N");

    let info = CommitInfo {
        hash: f.first().copied().unwrap_or_default().to_string(),
        short: f.get(1).copied().unwrap_or_default().to_string(),
        author_name: f.get(2).copied().unwrap_or_default().to_string(),
        author_email: f.get(3).copied().unwrap_or_default().to_string(),
        signature: signature_label(sig_code).to_string(),
        signed: sig_code != "N",
        subject: f.get(5).copied().unwrap_or_default().to_string(),
        amended: amend,
        files_changed,
        insertions,
        deletions,
    };

    let headline = if amend {
        format!("Amended {}.", info.short)
    } else {
        format!("Committed {}.", info.short)
    };
    let detail = format!(
        "as {} <{}> · {} file(s), +{} −{}{}",
        info.author_name,
        info.author_email,
        info.files_changed,
        info.insertions,
        info.deletions,
        if info.signed {
            format!(" · signature {}", info.signature)
        } else {
            String::new()
        }
    );

    Ok(OpResult {
        commit: Some(info),
        ..OpResult::done(headline, detail, after)
    })
}

pub fn parse_shortstat(s: &str) -> (usize, usize, usize) {
    let num = |needle: &str| -> usize {
        s.split(',')
            .find(|p| p.contains(needle))
            .and_then(|p| p.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (num("file"), num("insertion"), num("deletion"))
}

// --- Push ----------------------------------------------------------------

pub async fn push(repo_path: &str, set_upstream: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Push, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let Some(branch) = before.branch.clone() else {
        return Ok(OpResult::refused(
            refuse("detached-head", "You're not on a branch."),
            before,
        ));
    };

    // Which remote, and which ref on it.
    let (remote, remote_ref) = match &before.upstream {
        Some(up) => {
            let remote = up.split('/').next().unwrap_or("origin").to_string();
            let merge_ref = GitCmd::at(repo_path)
                .args(["config", "--get", &format!("branch.{}.merge", branch)])
                .ok_text()
                .await
                .filter(|s| s.starts_with("refs/heads/"))
                .unwrap_or_else(|| format!("refs/heads/{}", branch));
            (remote, merge_ref)
        }
        None => (
            before
                .push
                .remotes
                .first()
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "origin".into()),
            format!("refs/heads/{}", branch),
        ),
    };
    crate::git_exec::validate_remote_name(&remote)?;
    crate::git_exec::validate_branch_name(&branch)?;

    let needs_upstream = before.upstream.is_none() || set_upstream;
    let args = build_push_args(&remote, &branch, &remote_ref, needs_upstream);
    let out = GitCmd::at(repo_path)
        .args(args)
        .timeout(NET_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let mut ctx = advice_ctx(&after);
        ctx.remote = Some(&remote);
        let a = git_advice::explain(GitOp::Push, &out.stderr, &ctx);
        return Ok(OpResult::failed(a, after));
    }

    let refs = parse_push_porcelain(&out.text());
    let up_to_date = refs.iter().all(|r| r.flag == "=");
    let outcome = PushOutcome {
        up_to_date,
        new_upstream: if needs_upstream {
            after.upstream.clone()
        } else {
            None
        },
        remote: remote.clone(),
        refs,
    };

    let headline = if up_to_date {
        "Already up to date.".to_string()
    } else {
        format!("Pushed to {}.", remote)
    };
    let detail = outcome
        .refs
        .iter()
        .map(|r| format!("{} {}", r.to, r.summary))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(OpResult {
        push: Some(outcome),
        ..OpResult::done(headline, detail, after)
    })
}

/// `--porcelain` output: `<flag>\t<from>:<to>\t<summary> (<reason>)`.
pub fn parse_push_porcelain(out: &str) -> Vec<PushedRef> {
    let mut refs = Vec::new();
    for line in out.lines() {
        if line.starts_with("To ") || line.starts_with("Done") || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let (from, to) = parts[1].split_once(':').unwrap_or(("", parts[1]));
        refs.push(PushedRef {
            flag: parts[0].to_string(),
            from: from.to_string(),
            to: to.to_string(),
            summary: parts[2].to_string(),
        });
    }
    refs
}

// --- Pull ----------------------------------------------------------------

/// `git pull` is never used: its behaviour depends on `pull.rebase`, `pull.ff`
/// and `branch.<n>.rebase`, so the same button could merge for one user and
/// rebase for another. Fetch and integrate are separate, and the mode the user
/// picked is the command that runs.
pub async fn pull(repo_path: &str, mode: PullMode, with_lfs: bool, autostash: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Pull { mode, autostash }, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let upstream = before.upstream.clone().unwrap_or_default();
    let remote = upstream.split('/').next().unwrap_or("origin").to_string();
    crate::git_exec::validate_remote_name(&remote)?;
    let head_before = before.head_oid.clone().unwrap_or_default();

    // Fetch only the remote's own refs: no refspec, no --prune, no --tags, so
    // nothing local can move in this step.
    let fetched = GitCmd::at(repo_path)
        .args(["fetch", "--quiet", "--", &remote])
        .timeout(NET_TIMEOUT)
        .run()
        .await?;
    if !fetched.ok() {
        let after = snapshot(repo_path).await?;
        let a = git_advice::explain(GitOp::Fetch, &fetched.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    // Fully qualified, so a local branch of the same name can't win.
    let upstream_ref = format!("refs/remotes/{}", upstream);
    let out = GitCmd::at(repo_path)
        .args(build_integrate_args(mode, &upstream_ref, autostash))
        .timeout(NET_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    // An autostash that git could not re-apply leaves conflicted files with
    // no rebase in progress, and stores the entry in the stash list (a clean
    // re-apply stores nothing). Git may still exit 0 here, so this is measured
    // — against `before`, so an old entry named "autostash" from an earlier
    // pull cannot make a clean pull look failed.
    let autostash_conflict = mode == PullMode::Rebase
        && autostash
        && after.operation.is_none()
        && (after.conflicted_count > 0 || after.stash_count > before.stash_count);
    let head_after = after.head_oid.clone().unwrap_or_default();
    let conflicts: Vec<String> = after
        .entries
        .iter()
        .filter(|e| e.kind == "conflicted")
        .map(|e| e.path.clone())
        .collect();

    let fast_forward = !head_before.is_empty()
        && GitCmd::at(repo_path)
            .args(["merge-base", "--is-ancestor", &head_before, &head_after])
            .run()
            .await
            .map(|o| o.ok())
            .unwrap_or(false);
    // The upstream commits that came in — not `head_before..head_after`, which
    // would also count the local commits a rebase replayed or the merge commit.
    let commits_pulled = if head_before == head_after {
        0
    } else {
        GitCmd::at(repo_path)
            .args([
                "rev-list",
                "--count",
                &format!("{}..{}", head_before, upstream_ref),
            ])
            .ok_text()
            .await
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    let files_changed = if head_before == head_after {
        0
    } else {
        GitCmd::at(repo_path)
            .args([
                "diff",
                "--name-only",
                &format!("{}..{}", head_before, head_after),
            ])
            .ok_text()
            .await
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    };

    let outcome = PullOutcome {
        mode: mode.label().to_string(),
        head_before: head_before.clone(),
        head_after: head_after.clone(),
        fast_forward,
        commits_pulled,
        files_changed,
        conflicts: conflicts.clone(),
        recovery: if head_before.is_empty() || head_before == head_after {
            None
        } else {
            Some(format!("git reset --hard {}", head_before))
        },
        autostash_conflict,
        autostash_left: if autostash_conflict { Some("stash@{0}".to_string()) } else { None },
    };

    if autostash_conflict {
        let a = git_advice::explain(
            GitOp::Rebase,
            &format!("Applying autostash resulted in conflicts\n{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            pull: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    if !out.ok() {
        // Conflicts are left exactly as they are — resolving them is the user's
        // work, and aborting for them would throw it away.
        let a = git_advice::explain(
            match mode {
                PullMode::Rebase => GitOp::Rebase,
                PullMode::Merge => GitOp::Merge,
                // The user pressed Pull, so that is the word the message uses.
                PullMode::FfOnly => GitOp::Pull,
            },
            &format!("{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            pull: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    let mut headline = if commits_pulled == 0 {
        "Already up to date.".to_string()
    } else {
        format!(
            "Pulled {} commit(s) with {}.",
            commits_pulled,
            mode.label()
        )
    };
    let mut detail = if commits_pulled == 0 {
        String::new()
    } else {
        format!(
            "{} file(s) changed. {}",
            files_changed,
            if fast_forward {
                "Fast-forwarded."
            } else {
                "History was combined."
            }
        )
    };

    // A plain pull only downloads LFS content for files it touches, and only
    // when the repository's LFS filters are configured. Without them every
    // pulled large file lands as a pointer stub and nothing says so — which is
    // why this step is offered, and why it is named in the result either way.
    let lfs_state;
    if with_lfs {
        let done = fetch_lfs_content(repo_path).await?;
        lfs_state = Some(done.after.clone());
        if let Some(a) = done.error {
            detail.push_str(&format!(" The pull worked, but downloading the LFS files didn't: {}", a.headline));
        } else if done.fetched > 0 {
            headline.push_str(&format!(
                " Downloaded {} LFS file{}.",
                done.fetched,
                if done.fetched == 1 { "" } else { "s" }
            ));
            if done.configured_now {
                detail.push_str(" This repository's LFS filters weren't set up, so that was done first.");
            }
        }
    } else {
        // Not asked for, but say so rather than leaving silent stubs behind.
        let state = crate::lfs::lfs_status(repo_path).await?;
        if state.uses_lfs && state.pointers > 0 {
            detail.push_str(&format!(
                " {} LFS file(s) here are still pointer stubs — use Pull LFS files to download them.",
                state.pointers
            ));
        }
        lfs_state = Some(state);
    }

    Ok(OpResult {
        pull: Some(outcome),
        lfs: lfs_state,
        ..OpResult::done(headline, detail, after)
    })
}

// --- Submodules ----------------------------------------------------------

pub async fn submodule_update(repo_path: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Submodule, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    if !before.has_submodules {
        return Ok(OpResult::refused(
            refuse("no-submodules", "This repository has no submodules."),
            before,
        ));
    }
    let url = before
        .push
        .remotes
        .first()
        .map(|r| r.fetch_url.clone())
        .unwrap_or_default();

    // Reuse the clone path's per-submodule loop: a batch `submodule update`
    // aborts on the first gitlink missing from .gitmodules and leaves the rest
    // cloned-but-empty.
    let report = crate::sparse::update_submodules(&PathBuf::from(repo_path), None, None, &url).await;
    let after = snapshot(repo_path).await?;

    let headline = format!("{} of {} submodule(s) up to date.", report.downloaded, report.listed);
    let mut detail = String::new();
    if report.unlisted > 0 {
        detail.push_str(&format!(
            "{} submodule entry(s) have no address in .gitmodules, so git can't fetch them — the same as any clone. ",
            report.unlisted
        ));
    }
    if let Some(err) = &report.error {
        detail.push_str(err);
    }

    Ok(OpResult {
        submodules: Some(report),
        ..OpResult::done(headline, detail, after)
    })
}

// --- Git LFS -------------------------------------------------------------

/// Download the real content for every LFS pointer stub in the checkout.
/// `git lfs pull` is fetch + checkout for the current ref only, so it never
/// touches history, the index, or files that aren't LFS pointers.
/// What a download of LFS content actually did. Every number is measured by
/// counting pointer stubs before and after, never inferred from an exit code.
pub struct LfsFetch {
    pub after: crate::lfs::LfsStatus,
    pub fetched: usize,
    /// The repository had no LFS filters, so `git lfs install --local` was run
    /// first — without it `git lfs pull` exits 0 and downloads nothing.
    pub configured_now: bool,
    /// git-lfs failed. The caller decides how loudly to say so.
    pub error: Option<Advice>,
}

impl LfsFetch {
    fn nothing_to_do(status: crate::lfs::LfsStatus) -> Self {
        LfsFetch {
            after: status,
            fetched: 0,
            configured_now: false,
            error: None,
        }
    }
}

/// Download the content behind every pointer stub. Safe to call on any repo:
/// one that doesn't use LFS, has no stubs, or has no git-lfs installed simply
/// reports that nothing was done.
pub(crate) async fn fetch_lfs_content(repo_path: &str) -> Result<LfsFetch, AppError> {
    let before = crate::lfs::lfs_status(repo_path).await?;
    if !before.uses_lfs || !before.installed || before.pointers == 0 {
        return Ok(LfsFetch::nothing_to_do(before));
    }

    // A clone made before git-lfs was installed has no lfs filters in its
    // config. In that state `git lfs pull` prints "Skipping object checkout,
    // Git LFS is not installed for this repository" and exits 0 having done
    // nothing. `git lfs install --local` writes exactly those filter entries,
    // touches nothing else, and is what the LFS docs tell you to run first.
    let mut configured_now = false;
    if !before.filters_configured {
        let setup = GitCmd::at(repo_path)
            .args(["lfs", "install", "--local"])
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !setup.ok() {
            let advice = git_advice::explain(GitOp::Lfs, &setup.stderr, &AdviceCtx::default());
            return Ok(LfsFetch {
                after: before,
                fetched: 0,
                configured_now: false,
                error: Some(advice),
            });
        }
        configured_now = true;
    }

    let out = GitCmd::at(repo_path)
        .args(["lfs", "pull"])
        .timeout(NET_TIMEOUT)
        .run()
        .await?;

    // Measure, don't assume: count the stubs again.
    let after = crate::lfs::lfs_status(repo_path).await?;
    let fetched = before.pointers.saturating_sub(after.pointers);
    let error = if out.ok() {
        None
    } else {
        let stderr = if out.stderr.is_empty() { out.text() } else { out.stderr.clone() };
        Some(git_advice::explain(GitOp::Lfs, &stderr, &AdviceCtx::default()))
    };

    Ok(LfsFetch { after, fetched, configured_now, error })
}

pub async fn lfs_pull(repo_path: &str) -> Result<OpResult, AppError> {
    // LFS state is read *before* the repo snapshot on purpose: with
    // filter.lfs.required set and git-lfs missing, `git status` itself fails,
    // and the person needs the install hint — not the snapshot's error.
    let lfs_before = crate::lfs::lfs_status(repo_path).await?;
    if !lfs_before.uses_lfs {
        return Ok(OpResult::refused_alone(refuse(
            "no-lfs",
            "This repository doesn't use Git LFS — there is nothing to pull.",
        )));
    }
    if !lfs_before.installed {
        return Ok(OpResult::refused_alone(refuse(
            "lfs-not-installed",
            "git-lfs isn't installed on this Mac, so the large files can't be downloaded. Install it (brew install git-lfs) and try again.",
        )));
    }
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Submodule, &StatusFacts::from(&before)) {
        // Same rule as submodule update: not in the middle of a merge/rebase,
        // because lfs pull rewrites files in the worktree.
        return Ok(OpResult::refused(
            Refusal {
                code: r.code,
                message: r.message.replace("update submodules", "pull LFS files"),
            },
            before,
        ));
    }
    if lfs_before.pointers == 0 {
        return Ok(OpResult {
            lfs: Some(lfs_before),
            ..OpResult::done(
                "All LFS files are already here.",
                "Nothing needed downloading.",
                before,
            )
        });
    }

    let done = fetch_lfs_content(repo_path).await?;
    let lfs_after = done.after.clone();
    let fetched = done.fetched;
    let configured_now = done.configured_now;
    let after = snapshot(repo_path).await?;

    if let Some(a) = done.error {
        return Ok(OpResult {
            lfs: Some(lfs_after),
            detail: if fetched > 0 {
                format!("{} file(s) did download before it failed.", fetched)
            } else {
                a.guidance.clone()
            },
            ..OpResult::failed(a, after)
        });
    }

    let headline = if lfs_after.pointers == 0 {
        format!("Downloaded {} LFS file{}.", fetched, if fetched == 1 { "" } else { "s" })
    } else {
        format!(
            "Downloaded {} of {} LFS files — {} still missing.",
            fetched, lfs_before.pointers, lfs_after.pointers
        )
    };
    let mut detail = if lfs_after.pointers == 0 {
        "Every pointer stub now holds its real content.".to_string()
    } else if !lfs_after.filters_configured {
        "git lfs pull skipped the checkout because this repository's LFS filters aren't configured — even after setting them up. Run `git lfs install --local` in the folder and try again.".to_string()
    } else {
        "git lfs pull finished without an error but some files are still pointer stubs; their objects may not exist on the server.".to_string()
    };
    if configured_now {
        detail.push_str(" (LFS filters were not set up for this repository, so `git lfs install --local` was run first.)");
    }
    Ok(OpResult {
        lfs: Some(lfs_after),
        ..OpResult::done(headline, detail, after)
    })
}

// --- Abort ---------------------------------------------------------------

pub async fn abort(repo_path: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Abort, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let op = before.operation.clone().unwrap();
    let args: Vec<&str> = op.abort_command.split_whitespace().skip(1).collect();
    let out = GitCmd::at(repo_path)
        .args(args)
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(op_kind_to_gitop(&op.kind), &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    Ok(OpResult::done(
        format!("Aborted the {}.", op.kind),
        "The branch is back where it was before it started.".to_string(),
        after,
    ))
}

fn op_kind_to_gitop(kind: &str) -> GitOp {
    if kind.contains("rebase") || kind == "am" {
        GitOp::Rebase
    } else {
        GitOp::Merge
    }
}

/// Is the pick that stopped now empty — every change resolved to what HEAD
/// already has? Then `--continue` refuses and `--skip` is the only way on.
async fn staged_is_empty(repo_path: &str) -> bool {
    GitCmd::at(repo_path)
        .args(["diff", "--cached", "--quiet", "HEAD"])
        .run()
        .await
        .map(|o| o.ok())
        .unwrap_or(false)
}

// --- Continue ------------------------------------------------------------

/// Finish the operation in progress: `rebase --continue`, `cherry-pick
/// --continue`, `revert --continue`, `am --continue`, or the commit that
/// concludes a merge. A pick that became empty after the resolution is
/// skipped (git would refuse to continue it) and the result says so.
pub async fn continue_op(repo_path: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Continue, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let op = before.operation.clone().unwrap();
    let cmd = op.continue_command.clone().unwrap_or_default();
    let mut args: Vec<String> = cmd.split_whitespace().skip(1).map(|s| s.to_string()).collect();
    let mut skipped = false;
    let picks = matches!(op.kind.as_str(), "rebase" | "rebase-interactive" | "cherry-pick" | "revert");
    if picks && staged_is_empty(repo_path).await {
        args = vec![args[0].clone(), "--skip".into()];
        skipped = true;
    }
    let out = GitCmd::at(repo_path)
        .pinned()
        .args(args.iter().map(String::as_str))
        .timeout(NET_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(
            op_kind_to_gitop(&op.kind),
            &format!("{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult::failed(a, after));
    }
    // A rebase started with --autostash pops the stash when it finishes; a pop
    // that conflicts leaves conflicted files behind with no rebase in progress
    // and stores the stash as a new list entry (a clean pop stores nothing).
    if op.kind.contains("rebase") && after.operation.is_none() && (after.conflicted_count > 0 || after.stash_count > before.stash_count) {
        let a = git_advice::explain(
            GitOp::Rebase,
            &format!("Applying autostash resulted in conflicts\n{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult::failed(a, after));
    }
    let (headline, detail) = if after.operation.is_some() {
        (
            format!("Continued the {} — it stopped again.", op.kind),
            if after.conflicted_count > 0 {
                format!("{} file(s) conflict this time. Resolve and stage them, then continue again.", after.conflicted_count)
            } else {
                after.operation.as_ref().map(|o| o.detail.clone()).unwrap_or_default()
            },
        )
    } else {
        (
            format!("Finished the {}.", op.kind),
            if skipped {
                "The stopped commit had nothing left to apply after the resolution, so it was skipped.".to_string()
            } else {
                String::new()
            },
        )
    };
    Ok(OpResult::done(headline, detail, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> StatusFacts {
        StatusFacts {
            identity_matches: true,
            upstream: Some("origin/main".into()),
            can_amend: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_push_is_structurally_incapable_of_forcing_or_deleting() {
        // Every shape the builder can produce, including hostile-looking input.
        for (remote, branch) in [
            ("origin", "main"),
            ("upstream", "feature/x"),
            ("origin", "--force"),
        ] {
            for set_upstream in [true, false] {
                let args = build_push_args(remote, branch, &format!("refs/heads/{}", branch), set_upstream);
                let joined = args.join(" ");
                for forbidden in [
                    "--force",
                    "--force-with-lease",
                    "--delete",
                    "--mirror",
                    "--all",
                    "--tags",
                    "--prune",
                ] {
                    assert!(
                        !args.iter().any(|a| a == forbidden),
                        "{} must never appear: {}",
                        forbidden,
                        joined
                    );
                }
                // A leading '+' in the refspec would make it a force-push.
                let refspec = args.last().unwrap();
                assert!(!refspec.starts_with('+'), "{}", refspec);
                // A refspec with an empty source deletes the remote branch.
                assert!(refspec.starts_with("refs/heads/"), "{}", refspec);
                assert!(refspec.contains(':') && !refspec.ends_with(':'));
                // Anything that looks like an option sits after `--`.
                let dashdash = args.iter().position(|a| a == "--").unwrap();
                assert!(args.iter().position(|a| a == remote).unwrap() > dashdash);
            }
        }
    }

    #[test]
    fn push_sets_upstream_only_when_asked() {
        let with = build_push_args("origin", "main", "refs/heads/main", true);
        assert!(with.iter().any(|a| a == "--set-upstream"));
        let without = build_push_args("origin", "main", "refs/heads/main", false);
        assert!(!without.iter().any(|a| a == "--set-upstream"));
    }

    #[test]
    fn push_honours_a_differently_named_upstream_branch() {
        let args = build_push_args("origin", "local-name", "refs/heads/remote-name", false);
        assert_eq!(args.last().unwrap(), "refs/heads/local-name:refs/heads/remote-name");
    }

    #[test]
    fn each_pull_mode_runs_the_command_its_name_promises() {
        let ff = build_integrate_args(PullMode::FfOnly, "refs/remotes/origin/main", false);
        assert_eq!(ff[0], "merge");
        assert!(ff.iter().any(|a| a == "--ff-only"));
        assert!(!ff.iter().any(|a| a == "--rebase"));

        let merge = build_integrate_args(PullMode::Merge, "refs/remotes/origin/main", false);
        assert_eq!(merge[0], "merge");
        assert!(merge.iter().any(|a| a == "--no-edit"));
        assert!(!merge.iter().any(|a| a == "--ff-only"));

        let rebase = build_integrate_args(PullMode::Rebase, "refs/remotes/origin/main", false);
        assert_eq!(rebase[0], "rebase");
        // Autostash would silently move the user's work and can fail on the way back.
        assert!(rebase.iter().any(|a| a == "--no-autostash"));
        // …unless the user asked for it, in which case the failure is reported, never hidden.
        let auto = build_integrate_args(PullMode::Rebase, "refs/remotes/origin/main", true);
        assert!(auto.iter().any(|a| a == "--autostash"));
        assert!(!auto.iter().any(|a| a == "--no-autostash"));

        // All three point at the fully-qualified remote ref.
        for args in [ff, merge, rebase] {
            assert_eq!(args.last().unwrap(), "refs/remotes/origin/main");
        }
    }

    #[test]
    fn pull_modes_parse_from_exactly_three_words() {
        assert_eq!(PullMode::parse("ff-only").unwrap(), PullMode::FfOnly);
        assert_eq!(PullMode::parse("merge").unwrap(), PullMode::Merge);
        assert_eq!(PullMode::parse("rebase").unwrap(), PullMode::Rebase);
        assert!(PullMode::parse("").is_err());
        assert!(PullMode::parse("--hard").is_err());
    }

    #[test]
    fn pathspecs_are_literal_nul_terminated_and_never_empty() {
        let out = pathspec_stdin(&["src/a b.ts".into(), "-weird.txt".into()]).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert_eq!(text, ":(literal)src/a b.ts\0:(literal)-weird.txt\0");
        // A glob must stay a literal filename.
        let star = pathspec_stdin(&["*.md".into()]).unwrap();
        assert_eq!(String::from_utf8(star).unwrap(), ":(literal)*.md\0");
        // Empty selection is an error, never "everything".
        assert!(pathspec_stdin(&[]).is_err());
    }

    #[test]
    fn a_filename_with_a_newline_survives_the_pathspec_encoding() {
        let out = pathspec_stdin(&["weird\nname.txt".into()]).unwrap();
        assert_eq!(out.iter().filter(|b| **b == 0).count(), 1);
        assert!(String::from_utf8(out).unwrap().contains("weird\nname.txt"));
    }

    #[test]
    fn paths_outside_the_worktree_are_rejected() {
        for bad in [
            "/etc/passwd",
            "../outside.txt",
            "a/../../b",
            "~/secret",
            "C:/Windows/x",
            "",
        ] {
            assert!(validate_repo_path(bad).is_err(), "{} should be rejected", bad);
        }
        for good in ["src/app.ts", "a b/c.md", "deep/nested/path.rs", "..dotfile"] {
            assert!(validate_repo_path(good).is_ok(), "{} should be allowed", good);
        }
    }

    #[test]
    fn commit_refuses_with_nothing_staged_and_with_an_empty_message() {
        let f = clean();
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "nothing-staged");

        let staged = StatusFacts { staged: 2, ..clean() };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: true,
            },
            &staged,
        )
        .unwrap();
        assert_eq!(r.code, "empty-message");
    }

    #[test]
    fn a_merge_can_still_be_concluded_by_a_commit() {
        // The case a naive "refuse if any operation is in progress" rule would
        // make impossible.
        let f = StatusFacts {
            staged: 3,
            conflicted: 0,
            operation: Some("merge".into()),
            abort_command: Some("git merge --abort".into()),
            ..clean()
        };
        assert!(check(
            &Intent::Commit {
                amend: false,
                message_empty: false
            },
            &f
        )
        .is_none());

        // And with nothing staged: resolving every conflict in favour of our
        // side leaves the index identical to HEAD, but the merge commit still
        // has to be made — it is what records the second parent.
        let resolved_to_ours = StatusFacts { staged: 0, ..f };
        assert!(check(
            &Intent::Commit {
                amend: false,
                message_empty: false
            },
            &resolved_to_ours
        )
        .is_none());

        // Outside a merge, nothing staged is still a refusal.
        let idle = StatusFacts {
            staged: 0,
            operation: None,
            abort_command: None,
            ..clean()
        };
        assert_eq!(
            check(
                &Intent::Commit {
                    amend: false,
                    message_empty: false
                },
                &idle
            )
            .unwrap()
            .code,
            "nothing-staged"
        );
    }

    #[test]
    fn commit_refuses_while_conflicts_remain() {
        let f = StatusFacts {
            staged: 1,
            conflicted: 2,
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "unmerged-paths");
    }

    #[test]
    fn commit_refuses_when_the_identity_is_wrong_and_says_where_it_came_from() {
        let f = StatusFacts {
            staged: 1,
            identity_matches: false,
            identity_email: "personal@me.com".into(),
            profile_email: Some("work@corp.com".into()),
            email_scope: "local".into(),
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "identity-mismatch");
        assert!(r.message.contains("work@corp.com"));
        assert!(r.message.contains("personal@me.com"));
        assert!(r.message.contains(".git/config"));
    }

    #[test]
    fn amending_a_published_commit_is_refused() {
        let f = StatusFacts {
            staged: 1,
            can_amend: false,
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: true,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "already-pushed");
        assert!(r.message.contains("force-push"));
        // Amend with nothing staged is fine — that's how you fix a message.
        let f2 = StatusFacts { staged: 0, ..clean() };
        assert!(check(
            &Intent::Commit {
                amend: true,
                message_empty: false
            },
            &f2
        )
        .is_none());
    }

    #[test]
    fn a_locked_push_is_refused_by_its_own_code() {
        let r = check(
            &Intent::Push,
            &StatusFacts {
                push_blocked: true,
                push_locked: true,
                push_reason: "Locked for this repository. Unlocking needs the administrator password.".into(),
                ..Default::default()
            },
        )
        .expect("refused");
        assert_eq!(r.code, "push-locked");
        assert!(r.message.contains("administrator"));
    }

    #[test]
    fn a_blocked_push_never_reaches_git() {
        let f = StatusFacts {
            push_blocked: true,
            push_reason: "Blocked by profile 'Work'.".into(),
            ..clean()
        };
        let r = check(&Intent::Push, &f).unwrap();
        assert_eq!(r.code, "push-blocked");
        assert!(r.message.contains("Work"));
    }

    #[test]
    fn push_refuses_on_a_detached_or_unborn_head() {
        let d = StatusFacts { detached: true, ..clean() };
        assert_eq!(check(&Intent::Push, &d).unwrap().code, "detached-head");
        let u = StatusFacts { unborn: true, ..clean() };
        assert_eq!(check(&Intent::Push, &u).unwrap().code, "unborn-head");
    }

    #[test]
    fn rebase_needs_a_clean_tree_but_fast_forward_does_not() {
        let dirty = StatusFacts { unstaged: 2, ..clean() };
        assert_eq!(
            check(&Intent::Pull { mode: PullMode::Rebase, autostash: false }, &dirty).unwrap().code,
            "dirty-tree"
        );
        // Let git decide for the other two — it refuses only if files collide.
        assert!(check(&Intent::Pull { mode: PullMode::FfOnly, autostash: false }, &dirty).is_none());
        assert!(check(&Intent::Pull { mode: PullMode::Merge, autostash: false }, &dirty).is_none());
    }

    #[test]
    fn pull_refuses_without_an_upstream() {
        let f = StatusFacts { upstream: None, ..clean() };
        assert_eq!(
            check(&Intent::Pull { mode: PullMode::Merge, autostash: false }, &f).unwrap().code,
            "no-upstream"
        );
    }

    #[test]
    fn every_operation_stops_while_a_rebase_is_in_progress() {
        let f = StatusFacts {
            operation: Some("rebase".into()),
            abort_command: Some("git rebase --abort".into()),
            staged: 1,
            ..clean()
        };
        for intent in [
            Intent::Push,
            Intent::Pull { mode: PullMode::Merge, autostash: false },
            Intent::Submodule,
            Intent::Sync,
            Intent::Tree(crate::tree::TreeIntent::UndoCommit { head_is_root: false, head_is_merge: false }),
            Intent::Tree(crate::tree::TreeIntent::Switch { detached_unique: 0 }),
            Intent::Tree(crate::tree::TreeIntent::DiscardAll { include_untracked: false, stash_first: false }),
            Intent::Stash(crate::stash::StashIntent::Push { include_untracked: false, has_paths: false }),
            Intent::Stash(crate::stash::StashIntent::Apply { exists: true, pop: true }),
        ] {
            let r = check(&intent, &f).unwrap();
            assert_eq!(r.code, "operation-in-progress", "{:?}", intent);
            assert!(r.message.contains("git rebase --abort"));
        }
        // Resolving a conflict is exactly the work an operation in progress needs.
        assert!(check(&Intent::Tree(crate::tree::TreeIntent::ResolveSide { has_paths: true, submodule_selected: false, not_conflicted_selected: false }), &f).is_none());
    }

    #[test]
    fn autostash_lets_a_dirty_tree_rebase_and_nothing_else_changes() {
        let dirty = StatusFacts { unstaged: 1, ..clean() };
        assert_eq!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: false }, &dirty).unwrap().code, "dirty-tree");
        assert!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: true }, &dirty).is_none());
        assert_eq!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: true }, &StatusFacts { upstream: None, ..dirty.clone() }).unwrap().code, "no-upstream");
    }

    #[test]
    fn a_paused_sync_blocks_everything_but_resolution_and_its_own_buttons() {
        let f = StatusFacts { sync_in_progress: true, operation: Some("rebase".into()), abort_command: Some("git rebase --abort".into()), continue_command: Some("git rebase --continue".into()), ..clean() };
        for intent in [
            Intent::Push,
            Intent::Pull { mode: PullMode::Merge, autostash: false },
            Intent::Commit { amend: false, message_empty: false },
            Intent::Abort,
            Intent::Continue,
            Intent::Sync,
            Intent::Submodule,
            Intent::Tree(crate::tree::TreeIntent::UndoCommit { head_is_root: false, head_is_merge: false }),
            Intent::Tree(crate::tree::TreeIntent::Reset { mode: crate::tree::ResetMode::Hard, target_valid: true, target_is_head: false, drops_pushed: false, stash_first: false }),
            Intent::Tree(crate::tree::TreeIntent::DiscardAll { include_untracked: false, stash_first: false }),
            Intent::Stash(crate::stash::StashIntent::Push { include_untracked: false, has_paths: false }),
            Intent::Stash(crate::stash::StashIntent::Drop { exists: true }),
        ] {
            assert_eq!(check(&intent, &f).unwrap().code, "sync-in-progress", "{:?}", intent);
        }
        assert!(check(&Intent::Stage, &f).is_none());
        // Resolution work is allowed: taking a side, and recovering a file from the autostash.
        assert!(check(&Intent::Tree(crate::tree::TreeIntent::ResolveSide { has_paths: true, submodule_selected: false, not_conflicted_selected: false }), &f).is_none());
        assert!(check(&Intent::Stash(crate::stash::StashIntent::RestoreFile { exists: true, in_stash: true }), &f).is_none());
        assert!(check(&Intent::SyncContinue, &f).is_none());
        assert!(check(&Intent::SyncAbort, &f).is_none());
        assert_eq!(check(&Intent::SyncContinue, &clean()).unwrap().code, "no-sync");
        // Sync itself needs a branch with an upstream and no operation, but not a clean tree.
        assert!(check(&Intent::Sync, &StatusFacts { staged: 3, unstaged: 2, ..clean() }).is_none());
        assert_eq!(check(&Intent::Sync, &StatusFacts { upstream: None, ..clean() }).unwrap().code, "no-upstream");
        assert_eq!(check(&Intent::Sync, &StatusFacts { detached: true, ..clean() }).unwrap().code, "detached-head");
    }

    #[test]
    fn continue_needs_an_operation_a_continue_command_and_no_conflicts() {
        assert_eq!(check(&Intent::Continue, &clean()).unwrap().code, "nothing-to-continue");
        let bisect = StatusFacts {
            operation: Some("bisect".into()),
            abort_command: Some("git bisect reset".into()),
            continue_command: None,
            ..clean()
        };
        assert_eq!(check(&Intent::Continue, &bisect).unwrap().code, "cannot-continue");
        let conflicted = StatusFacts {
            operation: Some("rebase".into()),
            abort_command: Some("git rebase --abort".into()),
            continue_command: Some("git rebase --continue".into()),
            conflicted: 2,
            ..clean()
        };
        let r = check(&Intent::Continue, &conflicted).unwrap();
        assert_eq!(r.code, "unmerged-paths");
        assert!(r.message.contains("2 files"));
        let ready = StatusFacts { conflicted: 0, ..conflicted };
        assert!(check(&Intent::Continue, &ready).is_none(), "a rebase with everything staged may continue");
    }

    #[test]
    fn unstaging_a_conflicted_file_is_refused() {
        let f = StatusFacts { conflicted: 1, ..clean() };
        let r = check(
            &Intent::Unstage {
                conflicted_selected: true,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "unmerged-paths");
        assert!(check(
            &Intent::Unstage {
                conflicted_selected: false
            },
            &f
        )
        .is_none());
    }

    #[test]
    fn discard_refuses_without_paths_and_inside_submodules() {
        let f = clean();
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: false,
                    submodule_selected: false,
                    conflicted_selected: false
                },
                &f
            )
            .unwrap()
            .code,
            "no-paths"
        );
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: true,
                    submodule_selected: true,
                    conflicted_selected: false
                },
                &f
            )
            .unwrap()
            .code,
            "submodule-selected"
        );
        // Inside a merge the stage entries are the conflict itself…
        let merging = StatusFacts { operation: Some("merge".into()), abort_command: Some("git merge --abort".into()), ..f.clone() };
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: true,
                    submodule_selected: false,
                    conflicted_selected: true
                },
                &merging
            )
            .unwrap()
            .code,
            "unmerged-paths"
        );
        // …but a stash that failed to re-apply leaves conflicts with no
        // operation, and restoring the file to HEAD is the way out.
        assert!(check(
            &Intent::Discard { has_paths: true, submodule_selected: false, conflicted_selected: true },
            &f
        )
        .is_none());
    }

    #[test]
    fn abort_is_refused_when_nothing_is_in_progress() {
        assert_eq!(
            check(&Intent::Abort, &clean()).unwrap().code,
            "nothing-to-abort"
        );
        let f = StatusFacts {
            operation: Some("merge".into()),
            abort_command: Some("git merge --abort".into()),
            ..clean()
        };
        assert!(check(&Intent::Abort, &f).is_none());
    }

    #[test]
    fn push_porcelain_is_parsed_into_refs() {
        let out = "To github.com:o/r.git\n \trefs/heads/main:refs/heads/main\t1a2b3c..4d5e6f\nDone";
        let refs = parse_push_porcelain(out);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].flag, " ");
        assert_eq!(refs[0].to, "refs/heads/main");
        assert_eq!(refs[0].summary, "1a2b3c..4d5e6f");

        let new_branch = "To github.com:o/r.git\n*\trefs/heads/feat:refs/heads/feat\t[new branch]\nDone";
        assert_eq!(parse_push_porcelain(new_branch)[0].flag, "*");

        let up_to_date = "To github.com:o/r.git\n=\trefs/heads/main:refs/heads/main\t[up to date]\nDone";
        let r = parse_push_porcelain(up_to_date);
        assert!(r.iter().all(|x| x.flag == "="));
    }

    #[test]
    fn shortstat_numbers_are_read_back() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 45 insertions(+), 2 deletions(-)"),
            (3, 45, 2)
        );
        assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)"), (1, 1, 0));
        assert_eq!(parse_shortstat(""), (0, 0, 0));
    }

    #[test]
    fn signature_codes_map_to_words() {
        assert_eq!(signature_label("G"), "good");
        assert_eq!(signature_label("B"), "bad");
        assert_eq!(signature_label("N"), "unsigned");
        assert_eq!(signature_label(""), "unsigned");
    }
}
