//! Turning git's terse failures into a sentence that says what to do next.
//!
//! This generalises `sparse::explain_clone_error` along one extra dimension:
//! the same stderr means different things for a clone and for a push. Transport
//! failures (SSH keys, SAML SSO, host keys, certificates) are *delegated* to the
//! clone explainer rather than reimplemented, so that hard-won wording stays in
//! one place and keeps its existing tests.
//!
//! Every message ends with git's own words. Nothing is hidden, and an
//! unrecognised failure is passed through verbatim rather than guessed at.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GitOp {
    Fetch,
    Pull,
    Push,
    Commit,
    Checkout,
    Merge,
    Rebase,
    Lfs,
    CherryPick,
    Revert,
    Branch,
    Reset,
    Stash,
}

impl GitOp {
    fn noun(&self) -> &'static str {
        match self {
            GitOp::Fetch => "fetch",
            GitOp::Pull => "pull",
            GitOp::Push => "push",
            GitOp::Commit => "commit",
            GitOp::Checkout => "branch switch",
            GitOp::Merge => "merge",
            GitOp::Rebase => "rebase",
            GitOp::Lfs => "LFS download",
            GitOp::CherryPick => "cherry-pick",
            GitOp::Revert => "revert",
            GitOp::Branch => "branch change",
            GitOp::Reset => "reset",
            GitOp::Stash => "stash",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct AdviceCtx<'a> {
    pub key_path: Option<&'a str>,
    pub url: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub remote: Option<&'a str>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Advice {
    pub headline: String,
    pub guidance: String,
    /// Stable id the UI maps to a button. `None` means there is nothing to offer.
    pub action: Option<String>,
    pub git_said: String,
}

fn advice(headline: &str, guidance: &str, action: Option<&str>, stderr: &str) -> Advice {
    Advice {
        headline: headline.to_string(),
        guidance: guidance.to_string(),
        action: action.map(|s| s.to_string()),
        git_said: stderr.trim().to_string(),
    }
}

/// Files git names in "your local changes would be overwritten" — pulled out so
/// the UI can list exactly what is in the way.
pub fn blocking_files(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut collecting = false;
    for line in stderr.lines() {
        let t = line.trim();
        if t.contains("would be overwritten by") || t.contains("would be lost by") {
            collecting = true;
            continue;
        }
        if collecting {
            if t.is_empty()
                || t.starts_with("Please")
                || t.starts_with("Aborting")
                || t.starts_with("error:")
                || t.starts_with("hint:")
                || t.starts_with("fatal:")
            {
                break;
            }
            out.push(t.to_string());
        }
    }
    out
}

pub fn explain(op: GitOp, stderr: &str, ctx: &AdviceCtx) -> Advice {
    let s = stderr;
    let has = |needle: &str| s.contains(needle);

    // --- GitSwitch's own guards, first: their text is ours, so it is exact ---
    if has("remote helper 'gitswitch-push-blocked'")
        || has("gitswitch-push-blocked")
        || has("GitSwitch: push blocked")
    {
        return advice(
            "Pushing is turned off for this repository.",
            "GitSwitch is blocking it on purpose. Turn push access back on for this repo (or its profile) in the Push access panel, then try again.",
            Some("unblock-push"),
            s,
        );
    }
    if has("GitSwitch: commit blocked") {
        // Deliberately does not repeat the hook's own --no-verify suggestion:
        // the app fixes the identity instead of bypassing the check.
        return advice(
            "This folder's identity guard refused the commit.",
            "The email git is configured with doesn't match the profile that owns this folder. Fix the identity in the Profiles page (or remove the repo-local override), then commit again.",
            Some("fix-identity"),
            s,
        );
    }

    // --- Operation-specific ---
    match op {
        GitOp::Push => {
            if has("non-fast-forward")
                || has("fetch first")
                || has("Updates were rejected because the remote contains work")
                || has("tip of your current branch is behind")
            {
                return advice(
                    "The remote has commits you don't have yet.",
                    "Pull first, then push. GitSwitch never force-pushes, because that would delete whatever is on the remote.",
                    Some("pull-then-push"),
                    s,
                );
            }
            if has("has no upstream branch") || has("set-upstream") {
                return advice(
                    "This branch isn't on the remote yet.",
                    "Use Publish branch — it pushes and sets the upstream, so later pushes just work.",
                    Some("push-set-upstream"),
                    s,
                );
            }
            if has("GH006")
                || has("protected branch")
                || has("pre-receive hook declined")
                || has("push declined due to")
            {
                let branch = ctx.branch.unwrap_or("this branch");
                return advice(
                    &format!("GitHub protects {}.", branch),
                    "Direct pushes are refused by a branch protection rule. Push to a new branch and open a pull request instead.",
                    Some("open-pr"),
                    s,
                );
            }
        }
        GitOp::Pull | GitOp::Merge | GitOp::Rebase | GitOp::CherryPick | GitOp::Revert | GitOp::Reset => {
            if has("is a merge but no -m option was given") {
                return advice(
                    "That's a merge commit — choose which side to keep.",
                    "Reverting or picking a merge needs a mainline parent. Parent 1 keeps your branch's side and undoes what the merge brought in.",
                    Some("choose-mainline"),
                    s,
                );
            }
            if has("bad revision") || has("unknown revision") || has("Not a valid object name") {
                return advice(
                    "That isn't a commit in this repository.",
                    "Check the id — it may belong to another repository, or the commit was never fetched here.",
                    None,
                    s,
                );
            }
            if has("needs merge") || has("you need to resolve your current index first") || has("You have unmerged files") {
                return advice(
                    "There are still conflicts to resolve.",
                    "Resolve and stage every conflicted file first, then try again.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("Not possible to fast-forward") || has("divergent branches") {
                return advice(
                    "Fast-forward isn't possible — the branches have diverged.",
                    "You have local commits the remote doesn't, and it has commits you don't. Choose Merge (keeps both histories, adds a merge commit) or Rebase (replays your commits on top; they get new hashes).",
                    Some("choose-pull-mode"),
                    s,
                );
            }
            if has("would be overwritten by")
                || has("Please commit your changes or stash them")
                || has("cannot pull with rebase")
                || has("You have unstaged changes")
            {
                let files = blocking_files(s);
                let list = if files.is_empty() {
                    String::new()
                } else {
                    format!(" In the way: {}.", files.join(", "))
                };
                return advice(
                    "Your uncommitted changes are in the way.",
                    &format!(
                        "Commit them or stash them first, then {} again.{}",
                        op.noun(),
                        list
                    ),
                    Some("stash-first"),
                    s,
                );
            }
            if has("CONFLICT (")
                || has("Automatic merge failed")
                || has("could not apply")
                || has("Merge conflict in")
            {
                return advice(
                    "There are conflicts to resolve.",
                    "Nothing was resolved automatically — your work is intact. Open the conflicted files, fix the marked sections, stage them, then finish. You can also abort and go back to where you were.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("The previous cherry-pick is now empty") || has("is now empty, possibly due to conflict resolution") {
                return advice(
                    "This commit has nothing left to apply.",
                    "The resolution made it identical to what's already there. Continue again: GitSwitch skips the empty commit and moves on.",
                    Some("skip-empty"),
                    s,
                );
            }
            if has("Applying autostash resulted in conflicts") {
                return advice(
                    "Your stashed changes conflict with the new commits.",
                    "The rebase itself finished. Your uncommitted work is safe in the stash: resolve the conflicted files and stage them, or run `git reset --hard` (nothing of yours is lost — it is still in stash@{0}) and `git stash pop` to try again by hand.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("Failed to merge submodule") || has("commits not present") || has("not checked out") {
                return advice(
                    "A submodule pointer conflicts.",
                    "Both sides moved the same submodule. Bring that submodule to the commit you want (with your commits on top), stage its path, then continue — or abort.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("pre-rebase hook refused") {
                return advice(
                    "A pre-rebase hook refused to let this rebase start.",
                    "The repository's own hook said no. Read its message below; nothing was changed.",
                    None,
                    s,
                );
            }
            if has("refusing to merge unrelated histories") {
                return advice(
                    "These two histories have nothing in common.",
                    "That usually means the remote isn't the repository you think it is, or the branch was recreated. Check the remote URL before going further — GitSwitch won't force unrelated histories together.",
                    None,
                    s,
                );
            }
            if has("no tracking information") || has("no upstream configured") {
                return advice(
                    "This branch isn't tracking a remote branch.",
                    "Publish it first (push with upstream), or set its upstream, then pull.",
                    Some("push-set-upstream"),
                    s,
                );
            }
        }
        GitOp::Commit => {
            if has("nothing to commit") || has("no changes added to commit") {
                return advice(
                    "Nothing is staged.",
                    "Stage the files you want in this commit first.",
                    None,
                    s,
                );
            }
            if has("You have unmerged files") || has("fix conflicts and run") {
                return advice(
                    "There are still conflicts.",
                    "Resolve every conflicted file and stage it, then commit to finish the merge.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("Please tell me who you are") || has("unable to auto-detect email") {
                return advice(
                    "git doesn't know who you are in this folder.",
                    "No name or email is configured here. Map this folder to a profile in GitSwitch, then commit again.",
                    Some("fix-identity"),
                    s,
                );
            }
            if has("gpg failed to sign") || has("failed to write commit object") {
                return advice(
                    "Signing failed, so the commit wasn't made.",
                    "This profile signs commits with an SSH key. Check that the signing key still exists and is readable in the Signing panel, or turn signing off for this profile.",
                    Some("fix-signing"),
                    s,
                );
            }
        }
        GitOp::Checkout | GitOp::Branch => {
            if has("invalid reference") || has("did not match any file(s) known to git") {
                return advice(
                    "That branch doesn't exist here.",
                    "Pick one from the list. A branch that only exists on the remote has to be checked out as a local branch first.",
                    None,
                    s,
                );
            }
            if has("is already checked out at") || has("is already used by worktree") {
                return advice(
                    "That branch is checked out in another worktree.",
                    "git keeps one checkout per branch. Switch there, or create a new branch from it here.",
                    None,
                    s,
                );
            }
            if has("Cannot delete branch") && has("checked out") {
                return advice(
                    "That's the branch you're on.",
                    "Switch to another branch first, then delete this one.",
                    None,
                    s,
                );
            }
            if has("is not fully merged") {
                return advice(
                    "This branch has commits nothing else holds.",
                    "Deleting it would drop them. Confirm the deletion to do it anyway — the undo command is shown afterwards.",
                    None,
                    s,
                );
            }
            if has("would be overwritten by checkout") || has("Please commit your changes") || has("would be overwritten by") {
                let files = blocking_files(s);
                let list = if files.is_empty() {
                    String::new()
                } else {
                    format!(" In the way: {}.", files.join(", "))
                };
                return advice(
                    "Your uncommitted changes are in the way.",
                    &format!("Commit or stash them before switching branches.{}", list),
                    Some("stash-first"),
                    s,
                );
            }
            if has("already exists") {
                return advice(
                    "A branch with that name already exists.",
                    "Pick a different name, or switch to the existing branch.",
                    None,
                    s,
                );
            }
        }
        GitOp::Stash => {
            if has("No local changes to save") {
                return advice("Nothing to stash.", "The working tree is clean.", None, s);
            }
            if has("conflicts in index. Try without --index") {
                return advice(
                    "The stash's staged/unstaged split couldn't be restored.",
                    "Apply it without restoring the index: the changes come back unstaged.",
                    Some("retry-without-index"),
                    s,
                );
            }
            if has("would be overwritten by") {
                let files = blocking_files(s);
                let list = if files.is_empty() { String::new() } else { format!(" In the way: {}.", files.join(", ")) };
                return advice(
                    "Your uncommitted changes are in the way.",
                    &format!("Commit or stash them first, then apply this stash.{}", list),
                    Some("stash-first"),
                    s,
                );
            }
            if has("CONFLICT (") || has("The stash entry is kept") || has("Merge conflict in") {
                return advice(
                    "The stash conflicts with what's here now.",
                    "Resolve the conflicted files (Keep mine / Take theirs works), stage them, and the change is applied. The stash entry is still in the list until you drop it.",
                    Some("resolve-conflicts"),
                    s,
                );
            }
            if has("is not a valid reference") || has("No stash entries found") || has("is not a stash-like commit") {
                return advice(
                    "That stash isn't there any more.",
                    "The list was out of date — refresh it.",
                    None,
                    s,
                );
            }
            if has("Could not restore untracked files from stash") || has("already exists, no checkout") {
                return advice(
                    "The stash's new files couldn't be written.",
                    "A file with the same name already exists in the tree. Move or delete it, then apply the stash again.",
                    None,
                    s,
                );
            }
        }
        GitOp::Lfs => {
            if has("'lfs' is not a git command") || has("git-lfs: command not found") {
                return advice(
                    "git-lfs isn't installed.",
                    "This repository stores large files with Git LFS, but the git-lfs tool isn't on this Mac. Install it (brew install git-lfs), then pull the files again.",
                    None,
                    s,
                );
            }
            // An HTTP LFS server says "Object does not exist on the server";
            // a path remote (git-lfs 3.x) says "remote missing object".
            if has("does not exist on the server")
                || has("Object does not exist")
                || has("remote missing object")
                || has("missing object")
            {
                return advice(
                    "Some large files aren't on the server.",
                    "The repository points at LFS objects the server doesn't have — usually because whoever committed them never pushed the objects, or they were pruned. Ask the person who added those files to run `git lfs push --all`.",
                    None,
                    s,
                );
            }
            if has("smudge filter lfs failed") || has("batch response") {
                return advice(
                    "The LFS server refused the download.",
                    "Git could reach the repository but the LFS batch request failed. Check the details below — it is usually an authentication or permissions problem for the LFS endpoint rather than for git itself.",
                    None,
                    s,
                );
            }
        }
        _ => {}
    }

    // --- Shared, operation-independent ---
    if let Some(msg) = crate::lfs::missing_lfs_message(s) {
        return advice(
            "git-lfs isn't installed, and this repository requires it.",
            &msg,
            None,
            s,
        );
    }
    if has("index.lock") {
        return advice(
            "Another git process is working in this folder.",
            "Wait for it to finish (a terminal command, an editor, or a background tool), then try again. If nothing is running, a stale .git/index.lock can be deleted by hand.",
            Some("retry"),
            s,
        );
    }
    if has("detached HEAD") || has("not currently on any branch") {
        return advice(
            "You're not on a branch (detached HEAD).",
            "Switch to a branch first — there's no branch for this to act on.",
            None,
            s,
        );
    }

    // --- Transport: reuse the clone explainer rather than restate it ---
    let delegated = crate::sparse::explain_clone_error(s, ctx.key_path, ctx.url.unwrap_or(""));
    if delegated.trim() != s.trim() {
        // It recognised the failure; it already ends with "git said: …".
        let hint = delegated
            .split("\n\ngit said:")
            .next()
            .unwrap_or(&delegated)
            .to_string();
        return Advice {
            headline: format!("The {} couldn't reach GitHub.", op.noun()),
            guidance: hint,
            action: None,
            git_said: s.trim().to_string(),
        };
    }

    Advice {
        headline: format!("The {} failed.", op.noun()),
        guidance: String::new(),
        action: None,
        git_said: s.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything the user would see, so a test can assert on all of it.
    fn all_text(a: &Advice) -> String {
        format!("{}\n{}\n{}", a.headline, a.guidance, a.git_said)
    }

    fn ctx() -> AdviceCtx<'static> {
        AdviceCtx::default()
    }

    #[test]
    fn a_blocked_push_names_the_block_and_never_suggests_bypassing_it() {
        let a = explain(
            GitOp::Push,
            "git: 'remote-gitswitch-push-blocked' is not a git command.\nfatal: remote helper 'gitswitch-push-blocked' aborted session",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("unblock-push"));
        assert!(a.headline.contains("turned off"));
        assert!(!all_text(&a).contains("--no-verify"));
    }

    #[test]
    fn a_blocked_commit_points_at_the_identity_not_at_no_verify() {
        // The hook's own text suggests --no-verify as a manual escape hatch.
        // The app must never repeat that suggestion.
        let a = explain(
            GitOp::Commit,
            "GitSwitch: commit blocked.\n  this folder should commit as: a@b.com\n    git -c user.email=\"x\" commit --no-verify ...",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("fix-identity"));
        assert!(!a.headline.contains("--no-verify"));
        assert!(!a.guidance.contains("--no-verify"));
    }

    #[test]
    fn non_fast_forward_sends_you_to_pull_not_to_force() {
        for stderr in [
            "! [rejected] main -> main (non-fast-forward)",
            "hint: Updates were rejected because the remote contains work that you do not have locally.",
            "! [rejected] main -> main (fetch first)",
        ] {
            let a = explain(GitOp::Push, stderr, &ctx());
            assert_eq!(a.action.as_deref(), Some("pull-then-push"), "{}", stderr);
            let msg = all_text(&a);
            assert!(!msg.contains("--force"));
            // git's own words are always carried through.
            assert!(!a.git_said.is_empty());
        }
    }

    #[test]
    fn a_missing_upstream_offers_publish() {
        let a = explain(
            GitOp::Push,
            "fatal: The current branch feat has no upstream branch.",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("push-set-upstream"));
    }

    #[test]
    fn a_protected_branch_names_the_branch_and_offers_a_pr() {
        let c = AdviceCtx {
            branch: Some("main"),
            ..Default::default()
        };
        let a = explain(
            GitOp::Push,
            "remote: error: GH006: Protected branch update failed for refs/heads/main.",
            &c,
        );
        assert_eq!(a.action.as_deref(), Some("open-pr"));
        assert!(a.headline.contains("main"));
    }

    #[test]
    fn a_diverged_branch_explains_both_remaining_choices() {
        let a = explain(
            GitOp::Pull,
            "fatal: Not possible to fast-forward, aborting.",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("choose-pull-mode"));
        let g = a.guidance.to_lowercase();
        assert!(g.contains("merge") && g.contains("rebase"));
    }

    #[test]
    fn local_changes_in_the_way_are_listed_by_name() {
        let stderr = "error: Your local changes to the following files would be overwritten by merge:\n\tsrc/app.ts\n\tREADME.md\nPlease commit your changes or stash them before you merge.\nAborting";
        let a = explain(GitOp::Pull, stderr, &ctx());
        assert_eq!(a.action.as_deref(), Some("stash-first"));
        assert!(a.guidance.contains("src/app.ts"));
        assert!(a.guidance.contains("README.md"));
        assert_eq!(blocking_files(stderr), vec!["src/app.ts", "README.md"]);
    }

    #[test]
    fn conflicts_are_reported_as_unresolved_and_recoverable() {
        let a = explain(
            GitOp::Merge,
            "CONFLICT (content): Merge conflict in src/app.ts\nAutomatic merge failed; fix conflicts and then commit the result.",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("resolve-conflicts"));
        // It must not claim anything was fixed.
        assert!(a.guidance.contains("Nothing was resolved automatically"));
    }

    #[test]
    fn unrelated_histories_are_explained_but_never_forced() {
        let a = explain(
            GitOp::Pull,
            "fatal: refusing to merge unrelated histories",
            &ctx(),
        );
        assert!(a.action.is_none());
        assert!(!all_text(&a).contains("--allow-unrelated-histories"));
    }

    #[test]
    fn a_stale_lock_tells_you_what_to_look_for() {
        let a = explain(
            GitOp::Commit,
            "fatal: Unable to create '/repo/.git/index.lock': File exists.",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("retry"));
    }

    #[test]
    fn signing_failures_point_at_the_signing_panel() {
        let a = explain(
            GitOp::Commit,
            "error: gpg failed to sign the data\nfatal: failed to write commit object",
            &ctx(),
        );
        assert_eq!(a.action.as_deref(), Some("fix-signing"));
    }

    #[test]
    fn transport_failures_reuse_the_clone_wording() {
        let c = AdviceCtx {
            key_path: Some("/Users/me/.ssh/id_ed25519_work"),
            url: Some("git@github.com:Org/repo.git"),
            ..Default::default()
        };
        let a = explain(
            GitOp::Push,
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            &c,
        );
        // Same advice the Clone page gives, named for the key.
        assert!(a.guidance.contains("id_ed25519_work"));
        assert!(a.git_said.contains("Permission denied"));
    }

    #[test]
    fn saml_sso_still_explains_authorising_the_key() {
        let c = AdviceCtx {
            key_path: Some("/Users/me/.ssh/id_company"),
            url: Some("git@github.com:Org/repo.git"),
            ..Default::default()
        };
        let a = explain(GitOp::Push, "ERROR: The `Org' organization has enabled or enforced SAML SSO.", &c);
        assert!(a.guidance.contains("single sign-on"));
    }

    #[test]
    fn an_unknown_failure_is_passed_through_untouched() {
        let a = explain(GitOp::Push, "fatal: something nobody has seen before", &ctx());
        assert!(a.action.is_none());
        assert!(a.guidance.is_empty());
        assert_eq!(a.git_said, "fatal: something nobody has seen before");
        assert!(all_text(&a).contains("something nobody has seen before"));
    }

    #[test]
    fn reverting_a_merge_asks_for_a_mainline() {
        let a = explain(GitOp::Revert, "error: commit abc is a merge but no -m option was given.\nfatal: revert failed", &AdviceCtx::default());
        assert_eq!(a.action.as_deref(), Some("choose-mainline"));
        assert!(a.guidance.contains("Parent 1"));
    }

    #[test]
    fn an_unknown_commit_is_named_as_such() {
        let a = explain(GitOp::Reset, "fatal: ambiguous argument 'zzz': unknown revision or path not in the working tree.", &AdviceCtx::default());
        assert!(a.headline.contains("isn't a commit"));
        assert!(a.action.is_none());
    }

    #[test]
    fn a_missing_branch_and_a_branch_in_another_worktree_are_explained() {
        let a = explain(GitOp::Branch, "fatal: invalid reference: nope", &AdviceCtx::default());
        assert!(a.headline.contains("doesn't exist"));
        let b = explain(GitOp::Branch, "fatal: 'feature' is already checked out at '/tmp/wt'", &AdviceCtx::default());
        assert!(b.headline.contains("another worktree"));
        let c = explain(GitOp::Branch, "error: the branch 'feature' is not fully merged", &AdviceCtx::default());
        assert!(c.guidance.contains("undo command"));
    }

    #[test]
    fn stash_failures_say_what_to_do_and_never_drop_anything() {
        let a = explain(GitOp::Stash, "No local changes to save", &AdviceCtx::default());
        assert_eq!(a.headline, "Nothing to stash.");
        let b = explain(GitOp::Stash, "Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\nThe stash entry is kept in case you need it again.", &AdviceCtx::default());
        assert_eq!(b.action.as_deref(), Some("resolve-conflicts"));
        assert!(b.guidance.contains("still in the list"));
        let c = explain(GitOp::Stash, "error: could not restore untracked files from stash\nCould not restore untracked files from stash", &AdviceCtx::default());
        assert!(c.guidance.contains("same name already exists"));
        let d = explain(GitOp::Stash, "fatal: log for 'stash' only has 1 entries\nstash@{3} is not a valid reference", &AdviceCtx::default());
        assert!(d.headline.contains("isn't there any more"));
        let e = explain(GitOp::Stash, "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt\nPlease commit your changes or stash them before you merge.", &AdviceCtx::default());
        assert_eq!(e.action.as_deref(), Some("stash-first"));
        assert!(e.guidance.contains("a.txt"));
        let f = explain(GitOp::Stash, "error: conflicts in index. Try without --index.", &AdviceCtx::default());
        assert_eq!(f.action.as_deref(), Some("retry-without-index"));
    }
}
