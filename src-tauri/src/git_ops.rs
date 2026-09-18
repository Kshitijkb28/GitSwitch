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
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

fn refuse(code: &str, message: impl Into<String>) -> Refusal {
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
    Pull(PullMode),
    Submodule,
    Abort,
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
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
    pub push_blocked: bool,
    pub push_reason: String,
    pub identity_matches: bool,
    pub identity_email: String,
    pub profile_email: Option<String>,
    pub email_scope: String,
    pub can_amend: bool,
}

impl From<&RepoStatus> for StatusFacts {
    fn from(s: &RepoStatus) -> Self {
        StatusFacts {
            staged: s.staged_count,
            unstaged: s.unstaged_count,
            conflicted: s.conflicted_count,
            operation: s.operation.as_ref().map(|o| o.kind.clone()),
            abort_command: s.operation.as_ref().map(|o| o.abort_command.clone()),
            detached: s.detached,
            unborn: s.unborn,
            upstream: s.upstream.clone(),
            push_blocked: s.push.blocked,
            push_reason: s.push.reason.clone(),
            identity_matches: s.identity.matches_profile,
            identity_email: s.identity.email.clone(),
            profile_email: s.identity.profile_email.clone(),
            email_scope: s.identity.email_scope.clone(),
            can_amend: s.can_amend,
        }
    }
}

/// Everything that must be true before git runs. Pure.
pub fn check(intent: &Intent, f: &StatusFacts) -> Option<Refusal> {
    let busy = |verb: &str| -> Option<Refusal> {
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
    };

    match intent {
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
            if *conflicted_selected {
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

        Intent::Pull(mode) => {
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
            if *mode == PullMode::Rebase && (f.staged > 0 || f.unstaged > 0) {
                // --no-autostash is deliberate: a silent stash/unstash cycle can
                // fail halfway and leave work in a stash nobody asked for.
                return Some(refuse(
                    "dirty-tree",
                    "Rebase needs a clean working tree. Commit or stash your changes first, or pull with merge instead.",
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
pub fn build_integrate_args(mode: PullMode, upstream_ref: &str) -> Vec<String> {
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
            "--no-autostash".into(),
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
}

impl OpResult {
    fn blank() -> Self {
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
        }
    }

    fn refused(r: Refusal, status: RepoStatus) -> Self {
        OpResult {
            ok: false,
            headline: r.message.clone(),
            refusal: Some(r),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    fn done(headline: impl Into<String>, detail: impl Into<String>, status: RepoStatus) -> Self {
        OpResult {
            ok: true,
            headline: headline.into(),
            detail: detail.into(),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    fn failed(advice: Advice, status: RepoStatus) -> Self {
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

async fn snapshot(repo_path: &str) -> Result<RepoStatus, AppError> {
    git_status::repo_status(repo_path).await
}

fn advice_ctx<'a>(status: &'a RepoStatus) -> AdviceCtx<'a> {
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

    let tracked: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "tracked")
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
pub async fn pull(repo_path: &str, mode: PullMode) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Pull(mode), &StatusFacts::from(&before)) {
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
        .args(build_integrate_args(mode, &upstream_ref))
        .timeout(NET_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
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
    let commits_pulled = if head_before == head_after {
        0
    } else {
        GitCmd::at(repo_path)
            .args([
                "rev-list",
                "--count",
                &format!("{}..{}", head_before, head_after),
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
    };

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

    let headline = if commits_pulled == 0 {
        "Already up to date.".to_string()
    } else {
        format!(
            "Pulled {} commit(s) with {}.",
            commits_pulled,
            mode.label()
        )
    };
    let detail = if commits_pulled == 0 {
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

    Ok(OpResult {
        pull: Some(outcome),
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
        let a = git_advice::explain(GitOp::Merge, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    Ok(OpResult::done(
        format!("Aborted the {}.", op.kind),
        "The branch is back where it was before it started.".to_string(),
        after,
    ))
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
        let ff = build_integrate_args(PullMode::FfOnly, "refs/remotes/origin/main");
        assert_eq!(ff[0], "merge");
        assert!(ff.iter().any(|a| a == "--ff-only"));
        assert!(!ff.iter().any(|a| a == "--rebase"));

        let merge = build_integrate_args(PullMode::Merge, "refs/remotes/origin/main");
        assert_eq!(merge[0], "merge");
        assert!(merge.iter().any(|a| a == "--no-edit"));
        assert!(!merge.iter().any(|a| a == "--ff-only"));

        let rebase = build_integrate_args(PullMode::Rebase, "refs/remotes/origin/main");
        assert_eq!(rebase[0], "rebase");
        // Autostash would silently move the user's work and can fail on the way back.
        assert!(rebase.iter().any(|a| a == "--no-autostash"));

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
            check(&Intent::Pull(PullMode::Rebase), &dirty).unwrap().code,
            "dirty-tree"
        );
        // Let git decide for the other two — it refuses only if files collide.
        assert!(check(&Intent::Pull(PullMode::FfOnly), &dirty).is_none());
        assert!(check(&Intent::Pull(PullMode::Merge), &dirty).is_none());
    }

    #[test]
    fn pull_refuses_without_an_upstream() {
        let f = StatusFacts { upstream: None, ..clean() };
        assert_eq!(
            check(&Intent::Pull(PullMode::Merge), &f).unwrap().code,
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
            Intent::Pull(PullMode::Merge),
            Intent::Submodule,
        ] {
            let r = check(&intent, &f).unwrap();
            assert_eq!(r.code, "operation-in-progress");
            assert!(r.message.contains("git rebase --abort"));
        }
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
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: true,
                    submodule_selected: false,
                    conflicted_selected: true
                },
                &f
            )
            .unwrap()
            .code,
            "unmerged-paths"
        );
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
