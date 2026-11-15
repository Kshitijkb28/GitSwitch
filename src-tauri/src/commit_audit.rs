use crate::error::AppError;
use crate::profiles;
use crate::repo_scan;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub(crate) const MARKER: &str = "GitSwitch identity guard";
const MAX_COMMITS_PER_REPO: usize = 60;

#[derive(Debug, Serialize, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub subject: String,
    /// Already on the remote — rewriting it would need a force-push.
    pub pushed: bool,
    /// The local branches holding it, when no remote-tracking branch does.
    #[serde(default)]
    pub branches: Vec<String>,
    /// On the checked-out branch: what Fix rewrites.
    pub on_head: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoAudit {
    pub path: String,
    pub repo_name: String,
    pub profile_name: String,
    pub expected_name: String,
    pub expected_email: String,
    /// Commits that are ours but carry the wrong identity.
    pub mismatched: Vec<CommitInfo>,
    pub unpushed_count: usize,
    pub pushed_count: usize,
    pub has_upstream: bool,
    pub dirty: bool,
    /// "none" | "gitswitch" | "foreign"
    pub guard: String,
    /// Wrong commits of yours on other local branches: check one out to fix them.
    pub other_branch_count: usize,
    /// Why Fix cannot run here right now, when it cannot.
    pub fix_blocker: Option<String>,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    // The one runner: its environment, its log, and the signature pin that
    // keeps verification text out of parsed output. The local clock: this
    // walks reflogs and history, and makes the backup branch.
    let out = crate::git_exec::GitCmd::at(repo)
        .args(args.iter().copied())
        .timeout(crate::git_exec::LOCAL_TIMEOUT)
        .run_blocking()
        .map_err(|e| match e {
            AppError::Command(said) => said,
            other => other.to_string(),
        })?;
    if !out.ok() {
        return Err(out.stderr);
    }
    Ok(out.trimmed())
}

fn git_env(repo: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    // Its one use is the rewrite: a rebase running a script on every commit,
    // which may sign each one through the person's own passphrase prompt.
    // A long clock, so only a command that is truly stuck is stopped.
    const REWRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
    let mut cmd = crate::git_exec::GitCmd::at(repo).timeout(REWRITE_TIMEOUT);
    // The leading `-c` pairs go where GitCmd puts its own, so the command
    // line is unchanged and the subcommand it names in a message is the
    // rebase rather than the first setting.
    let mut rest = args;
    while let ["-c", kv, tail @ ..] = rest {
        cmd = cmd.cfg_raw(kv);
        rest = tail;
    }
    cmd = cmd.args(rest.iter().copied());
    for (k, v) in envs {
        cmd = cmd.env(*k, *v);
    }
    let out = cmd.run_blocking().map_err(|e| match e {
        AppError::Command(said) => said,
        other => other.to_string(),
    })?;
    if !out.ok() {
        return Err(out.stderr);
    }
    Ok(out.trimmed())
}

/// One commit's facts, as `log --format=%H%x1f%h%x1f%an%x1f%ae%x1f%ce%x1f%cI%x1f%s` gives them.
struct Facts {
    hash: String,
    short: String,
    author_name: String,
    author_email: String,
    committer_email: String,
    date: String,
    subject: String,
}

const FACTS_FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ce%x1f%cI%x1f%s";

fn parse_facts(out: &str) -> Vec<Facts> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            (f.len() >= 7).then(|| Facts {
                hash: f[0].into(),
                short: f[1].into(),
                author_name: f[2].into(),
                author_email: f[3].into(),
                committer_email: f[4].into(),
                date: f[5].into(),
                subject: f[6..].join("\x1f"),
            })
        })
        .collect()
}

/// The commits no remote-tracking branch has — on HEAD and on each local
/// branch, 500 per ref at most — each with the local branches holding it and
/// whether HEAD does, in HEAD's order first. With no remote-tracking ref at
/// all, nothing has been published: every commit counts.
fn local_only(repo: &Path) -> Vec<(String, Vec<String>, bool)> {
    let mut out: Vec<(String, Vec<String>, bool)> = Vec::new();
    // By full ref name — a short one can also be a file of the same name — and
    // never GitSwitch's own backups: they keep what was there before a Fix, an
    // Update or a reset, which is the point of them, not work to list again.
    let branches: Vec<(String, String)> = git(repo, &["for-each-ref", "--format=%(refname)", "refs/heads"])
        .map(|s| {
            s.lines()
                .filter_map(|full| full.strip_prefix("refs/heads/").map(|name| (full.to_string(), name.to_string())))
                .filter(|(_, name)| !crate::git_history::is_safety_branch(name))
                .collect()
        })
        .unwrap_or_default();
    for (rev, name) in std::iter::once(("HEAD".to_string(), String::new())).chain(branches) {
        let Ok(list) = git(repo, &["rev-list", "-n", "500", &rev, "--not", "--remotes", "--"]) else { continue };
        for h in list.lines().filter(|h| !h.is_empty()) {
            let i = match out.iter().position(|(x, _, _)| x == h) {
                Some(i) => i,
                None => {
                    out.push((h.to_string(), Vec::new(), false));
                    out.len() - 1
                }
            };
            if rev == "HEAD" {
                out[i].2 = true;
            } else if !out[i].1.contains(&name) {
                out[i].1.push(name.clone());
            }
        }
    }
    out
}

fn details(repo: &Path, hashes: &[String]) -> Vec<Facts> {
    let mut out = Vec::new();
    for chunk in hashes.chunks(200) {
        let mut args: Vec<&str> = vec!["log", "--no-walk=unsorted", FACTS_FORMAT];
        args.extend(chunk.iter().map(String::as_str));
        if let Ok(o) = git(repo, &args) {
            out.extend(parse_facts(&o));
        }
    }
    out
}

/// A local commit is yours, made under the wrong identity, when its author is
/// not the profile's address and either the same person committed it (a
/// commit made under a misconfigured identity) or the author is one of your
/// profiles' addresses. A teammate's commit cherry-picked or merged here has
/// the teammate as author and someone else as committer: never yours.
/// `made`: the commits created in this repository (see `made_here`) — the
/// same person as author and committer is yours only if you made it here: a
/// contributor's commits fetched straight into a branch (a pull request
/// checked out) have that shape too.
fn own_wrong(f: &Facts, expected: &str, own_emails: &[String], made: &std::collections::HashSet<String>) -> bool {
    !f.author_email.eq_ignore_ascii_case(expected)
        && ((f.author_email.eq_ignore_ascii_case(&f.committer_email) && made.contains(&f.hash)) || own_emails.iter().any(|e| e.eq_ignore_ascii_case(&f.author_email)))
}

/// The commits created in this repository — by a commit, an amend, a merge, a
/// cherry-pick, a revert, an am, or a rebase's replay — read from its
/// reflogs. A commit that only arrived (fetched, checked out, fast-forwarded,
/// reset to) is not among them.
fn made_here(repo: &Path) -> std::collections::HashSet<String> {
    git(repo, &["log", "-g", "--all", "--format=%H%x1f%gs"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\x1f'))
        .filter(|(_, gs)| {
            gs.starts_with("commit")
                || gs.starts_with("cherry-pick")
                || gs.starts_with("revert")
                || gs.starts_with("am:")
                || ((gs.starts_with("rebase") || gs.starts_with("pull")) && ["(pick)", "(reword)", "(edit)", "(squash)", "(fixup)"].iter().any(|k| gs.contains(k)))
        })
        .map(|(h, _)| h.to_string())
        .collect()
}

/// The profile that owns a repo path — longest matching directory wins, which
/// mirrors how the generated gitconfig resolves overlapping folders.
pub(crate) fn owning_profile<'a>(
    repo: &str,
    profs: &'a [profiles::Profile],
) -> Option<&'a profiles::Profile> {
    let mut best: Option<&profiles::Profile> = None;
    let mut best_len = 0usize;
    for p in profs {
        for d in &p.directories {
            let dir = crate::paths::norm(d);
            if crate::paths::is_within(repo, &dir) && dir.len() >= best_len {
                best_len = dir.len();
                best = Some(p);
            }
        }
    }
    best
}

pub fn guard_state(repo: &Path) -> String {
    match hooks_dir(repo) {
        Ok(h) => guard_state_in(&h.dir),
        Err(_) => guard_state_in(&repo.join(".git").join("hooks")),
    }
}

/// Where this repository's hooks live, from git itself. `.git/hooks` is only
/// right for a plain checkout: in a worktree or a submodule `.git` is a file
/// pointing elsewhere, and `core.hooksPath` moves the directory anywhere —
/// a guard written to `.git/hooks` there is never run.
pub(crate) struct HooksDir {
    pub dir: PathBuf,
    /// `core.hooksPath` is set: the directory is shared or managed by another
    /// tool (husky, lefthook), and not GitSwitch's to write into.
    pub redirected: Option<String>,
}

pub(crate) fn hooks_dir(repo: &Path) -> Result<HooksDir, AppError> {
    let said = git(repo, &["rev-parse", "--git-path", "hooks"]).map_err(AppError::Command)?;
    let raw = PathBuf::from(said.trim());
    let dir = if raw.is_absolute() { raw } else { repo.join(raw) };
    let redirected = git(repo, &["config", "--get", "core.hooksPath"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(HooksDir { dir, redirected })
}

/// Same answer, but for a hooks directory already resolved with
/// `git rev-parse --git-path hooks` — which is the only correct way to find it
/// in a submodule or worktree (`.git` is a file there) or when the repo sets
/// `core.hooksPath`.
pub(crate) fn guard_state_in(hooks: &Path) -> String {
    match std::fs::read_to_string(hooks.join("pre-commit")) {
        Ok(content) if content.contains(MARKER) => "gitswitch".into(),
        Ok(_) => "foreign".into(),
        Err(_) => "none".into(),
    }
}

fn audit_repo(
    repo_path: &str,
    profs: &[profiles::Profile],
    own_emails: &[String],
) -> Option<RepoAudit> {
    let repo = PathBuf::from(repo_path);
    let profile = owning_profile(repo_path, profs)?;
    let expected_email = profile.git_email.trim().to_string();
    let has_upstream = git(&repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok();

    let mut mismatched = Vec::new();
    // Not on any remote: the wrong commits of yours, wherever they are.
    let local = local_only(&repo);
    let hashes: Vec<String> = local.iter().map(|(h, _, _)| h.clone()).collect();
    let made = made_here(&repo);
    for f in details(&repo, &hashes) {
        if !own_wrong(&f, &expected_email, own_emails, &made) {
            continue;
        }
        let (_, branches, on_head) = local.iter().find(|(h, _, _)| *h == f.hash).cloned().unwrap_or_default();
        mismatched.push(CommitInfo {
            hash: f.hash,
            short: f.short,
            author_name: f.author_name,
            author_email: f.author_email,
            date: f.date,
            subject: f.subject,
            pushed: false,
            branches,
            on_head,
        });
    }
    // Already published: only commits authored with one of your profiles'
    // addresses, among the checked-out branch's newest (a teammate's never).
    let recent = git(&repo, &["log", &format!("-n{}", MAX_COMMITS_PER_REPO), FACTS_FORMAT]).unwrap_or_default();
    for f in parse_facts(&recent) {
        if hashes.contains(&f.hash) || f.author_email.eq_ignore_ascii_case(&expected_email) || !own_emails.iter().any(|e| e.eq_ignore_ascii_case(&f.author_email)) {
            continue;
        }
        mismatched.push(CommitInfo {
            hash: f.hash,
            short: f.short,
            author_name: f.author_name,
            author_email: f.author_email,
            date: f.date,
            subject: f.subject,
            pushed: true,
            branches: Vec::new(),
            on_head: true,
        });
    }

    if mismatched.is_empty() {
        return None;
    }

    let dirty = !git(&repo, &["status", "--porcelain"])
        .unwrap_or_default()
        .is_empty();
    let unpushed_count = mismatched.iter().filter(|c| !c.pushed && c.on_head).count();
    let fix_blocker = if unpushed_count > 0 { fix_plan(&repo, profile, own_emails).err() } else { None };

    Some(RepoAudit {
        repo_name: crate::paths::base_name(repo_path),
        path: repo_path.to_string(),
        profile_name: profile.name.clone(),
        expected_name: profile.git_name.clone(),
        expected_email,
        unpushed_count,
        pushed_count: mismatched.iter().filter(|c| c.pushed).count(),
        other_branch_count: mismatched.iter().filter(|c| !c.pushed && !c.on_head).count(),
        mismatched,
        has_upstream,
        dirty,
        guard: guard_state(&repo),
        fix_blocker,
    })
}

/// What a fix would rewrite: from `base` (None = the root) up to HEAD, the
/// commits whose author is one of `wrong` re-authored, the rest replayed as
/// they are.
struct FixPlan {
    base: Option<String>,
    wrong: Vec<String>,
    count: usize,
}

/// Whether the checked-out branch's wrong commits of yours can be rewritten
/// now, and from where. Refused, with the reason in words: something in
/// progress, uncommitted changes, git committing here as someone other than
/// the profile (the fix would write that again), or a merge among them.
fn fix_plan(repo: &Path, profile: &profiles::Profile, own_emails: &[String]) -> Result<FixPlan, String> {
    for (marker, what) in [("rebase-merge", "a rebase"), ("rebase-apply", "a rebase"), ("MERGE_HEAD", "a merge"), ("CHERRY_PICK_HEAD", "a cherry-pick"), ("REVERT_HEAD", "a revert")] {
        if let Ok(p) = git(repo, &["rev-parse", "--git-path", marker]) {
            let p = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { repo.join(&p) };
            if p.exists() {
                return Err(format!("{} is in progress here — finish or abort it first.", what));
            }
        }
    }
    if !git(repo, &["status", "--porcelain"]).map_err(|e| e.to_string())?.is_empty() {
        return Err("This repo has uncommitted changes. Commit or stash them first — rewriting history needs a clean working tree.".into());
    }
    let ident = git(repo, &["var", "GIT_COMMITTER_IDENT"]).unwrap_or_default();
    let committing_as = ident.split_once('<').and_then(|(_, r)| r.split_once('>')).map(|(e, _)| e.trim().to_string()).unwrap_or_default();
    if !committing_as.eq_ignore_ascii_case(profile.git_email.trim()) {
        return Err(format!(
            "git here commits as {}, not {}: re-authoring now would write {} again. Remove this repository's own user.email (Doctor: Remove the local setting) or fix the folder's profile, then try again.",
            if committing_as.is_empty() { "nobody" } else { &committing_as },
            profile.git_email.trim(),
            if committing_as.is_empty() { "nothing" } else { &committing_as }
        ));
    }
    // A branch that tracks one the remote has, with no remote-tracking branch
    // for it here (a single-branch clone, a narrowed fetch): what the remote
    // already has cannot be told, and a commit it has is never rewritten.
    if let Ok(b) = git(repo, &["symbolic-ref", "-q", "--short", "HEAD"]) {
        if git(repo, &["config", "--get", &format!("branch.{}.merge", b.trim())]).is_ok() && git(repo, &["rev-parse", "--verify", "-q", "@{upstream}"]).is_err() {
            return Err("This branch's upstream has no remote-tracking branch here, so GitSwitch can't tell which commits the remote already has — fetch it (or widen remote.<name>.fetch) first.".into());
        }
    }
    let local = local_only(repo);
    let on_head: Vec<String> = local.iter().filter(|(_, _, h)| *h).map(|(h, _, _)| h.clone()).collect();
    let made = made_here(repo);
    let mut wrong_left: Vec<String> = details(repo, &on_head)
        .into_iter()
        .filter(|f| own_wrong(f, profile.git_email.trim(), own_emails, &made))
        .map(|f| f.hash)
        .collect();
    if wrong_left.is_empty() {
        return Err("Nothing to fix: no wrong commit of yours is on this branch.".into());
    }
    let count = wrong_left.len();
    let wrong_emails: Vec<String> = {
        let mut v: Vec<String> = details(repo, &wrong_left).into_iter().map(|f| f.author_email).collect();
        v.sort();
        v.dedup();
        v
    };
    // Newest to oldest along the first-parent line: the base is the parent of
    // the oldest wrong commit; a merge met before then cannot be replayed.
    let chain: Vec<String> = git(repo, &["rev-list", "--first-parent", "HEAD", "--not", "--remotes"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let mut parents: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for chunk in chain.chunks(200) {
        let mut args: Vec<&str> = vec!["log", "--no-walk=unsorted", "--format=%H %P"];
        args.extend(chunk.iter().map(String::as_str));
        for l in git(repo, &args).unwrap_or_default().lines() {
            let mut w = l.split_whitespace();
            if let Some(h) = w.next() {
                parents.insert(h.to_string(), w.map(str::to_string).collect());
            }
        }
    }
    for c in &chain {
        let ps = parents.get(c).cloned().unwrap_or_default();
        if ps.len() > 1 {
            return Err("A merge sits among the commits to rewrite; GitSwitch rewrites a straight line only. Rewrite them by hand, or undo the merge first.".into());
        }
        wrong_left.retain(|w| w != c);
        if wrong_left.is_empty() {
            return Ok(FixPlan { base: ps.first().cloned(), wrong: wrong_emails, count });
        }
    }
    Err("Some of the wrong commits sit on a merged side branch; GitSwitch rewrites a straight line only.".into())
}

/// Find commits across all profile folders that carry the wrong identity.
pub async fn audit_all() -> Result<Vec<RepoAudit>, AppError> {
    let store = profiles::load_profiles()?;
    let profs = store.profiles.clone();
    let own_emails: Vec<String> = profs.iter().map(|p| p.git_email.clone()).collect();

    let mut repo_paths: Vec<String> = Vec::new();
    for p in &profs {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            for r in repo_scan::scan_repos(dir.clone()).await.unwrap_or_default() {
                if !repo_paths.contains(&r.path) {
                    repo_paths.push(r.path);
                }
            }
        }
    }

    tokio::task::spawn_blocking(move || {
        let mut out: Vec<RepoAudit> = repo_paths
            .iter()
            .filter_map(|p| audit_repo(p, &profs, &own_emails))
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.unpushed_count));
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Re-author the checked-out branch's wrong commits of yours with the
/// folder's identity. Nothing a remote has is touched; teammates' commits
/// among them keep their author; author dates are kept; the old history is
/// kept on a backup branch.
pub fn fix_unpushed(repo_path: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let store = profiles::load_profiles()?;
    let own_emails: Vec<String> = store.profiles.iter().map(|p| p.git_email.clone()).collect();
    let Some(profile) = profile_for_repo(&repo, &store.profiles) else {
        return Err(AppError::Config("No profile owns this repository, so there is no identity to re-author with.".into()));
    };
    let plan = fix_plan(&repo, profile, &own_emails).map_err(AppError::Config)?;

    // Safety net: a ref pointing at the pre-rewrite history.
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let backup = format!("gitswitch-backup-{}", stamp);
    git(&repo, &["branch", &backup]).map_err(AppError::Command)?;

    // Only commits by one of the wrong addresses are amended — with
    // --author, which keeps their author dates; the rest are replayed as
    // they are. Nothing from here is put into the script: it reads both
    // from the environment.
    let wrong: String = plan.wrong.iter().map(|e| format!("<{}>", e)).collect();
    let author = format!("{} <{}>", profile.git_name.trim(), profile.git_email.trim());
    let script = r#"ae=$(git -c log.showSignature=false log -1 --format=%ae); case "$GITSWITCH_FIX_WRONG" in *"<$ae>"*) git commit --amend --no-edit --allow-empty --author="$GITSWITCH_FIX_AUTHOR";; esac"#;
    let base = plan.base.clone().unwrap_or_else(|| "--root".into());
    let res = git_env(
        &repo,
        &[
            "-c",
            "core.editor=true",
            // With rebase.updateRefs=true git would move the backup branch
            // along with the rewrite — the one thing it exists not to do.
            "-c",
            "rebase.updateRefs=false",
            "-c",
            "rebase.autoSquash=false",
            "rebase",
            "--no-autosquash",
            "--empty=keep",
            &base,
            "--exec",
            script,
        ],
        &[("GITSWITCH_FIX_WRONG", &wrong), ("GITSWITCH_FIX_AUTHOR", &author)],
    );

    match res {
        Ok(_) => Ok(format!(
            "Re-authored {} commit{}. Previous history kept on branch {}.",
            plan.count,
            if plan.count == 1 { "" } else { "s" },
            backup
        )),
        Err(e) => {
            // Nothing else was in progress (fix_plan refused that): the rebase is ours.
            let _ = git(&repo, &["rebase", "--abort"]);
            Err(AppError::Command(format!(
                "Rewrite failed and was rolled back ({}). Your history is unchanged; branch {} still points at it.",
                e, backup
            )))
        }
    }
}

const HOOK_TEMPLATE: &str = r#"#!/bin/sh
# >>> GitSwitch identity guard >>>
# Blocks commits whose email doesn't match the profile that owns this folder.
# Managed by GitSwitch — switch it off in GitSwitch: Repository > Changes, the guard beside "Committing as".
# gitswitch-expected-email: __EXPECTED_EMAIL_NOTE__
expected=__EXPECTED_EMAIL_SH__
actual="$(git config user.email)"
if [ "$actual" != "$expected" ]; then
  echo "GitSwitch: commit blocked."
  echo "  this folder should commit as: $expected"
  echo "  git is currently configured as: ${actual:-<unset>}"
  echo "  Check the profile for this folder in GitSwitch, or override with:"
  echo "    git -c user.email=\"$actual\" commit --no-verify ..."
  exit 1
fi
# <<< GitSwitch identity guard <<<
__CHAIN__
"#;

/// The guard's text for this email, with the call to the hook that was there
/// before (or none). A profile's email may hold `"`, `$` or a backtick:
/// single-quoted for the shell, where nothing but `'` itself is special.
fn hook_body(expected_email: &str, chain: &str) -> String {
    HOOK_TEMPLATE
        .replace("__EXPECTED_EMAIL_NOTE__", expected_email)
        .replace("__EXPECTED_EMAIL_SH__", &format!("'{}'", expected_email.replace('\'', "'\\''")))
        .replace("__CHAIN__", chain)
}

/// Install a pre-commit hook that refuses commits with the wrong identity.
/// An existing foreign hook is preserved and chained, never discarded.
pub fn install_guard(repo_path: &str, expected_email: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let resolved = hooks_dir(&repo)?;
    if let Some(p) = resolved.redirected {
        return Err(AppError::Config(format!(
            "This repository routes its hooks through core.hooksPath = {} — a directory another tool manages, which GitSwitch won't write into. Add the identity check to that tool's pre-commit hook, or unset core.hooksPath here.",
            p
        )));
    }
    let hooks = resolved.dir;
    std::fs::create_dir_all(&hooks)?;
    let hook = hooks.join("pre-commit");

    // The hook that was here before GitSwitch is kept and run after the check:
    // moved aside on the first install, and still chained when the guard is
    // written again (a new email, say).
    let saved = hooks.join("pre-commit.gitswitch-saved");
    if hook.exists() {
        let existing = std::fs::read_to_string(&hook)?;
        if !existing.contains(MARKER) {
            std::fs::rename(&hook, &saved)?;
        }
    }
    let chain = if saved.exists() {
        "# run the hook that was here before GitSwitch\nif [ -x \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" ]; then\n  \"$(dirname \"$0\")/pre-commit.gitswitch-saved\" \"$@\" || exit $?\nfi".to_string()
    } else {
        String::new()
    };

    let body = hook_body(expected_email, &chain);
    std::fs::write(&hook, body)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(format!(
        "Guard installed{}",
        if chain.is_empty() {
            String::new()
        } else {
            " (existing hook preserved and chained)".to_string()
        }
    ))
}

/// The email a GitSwitch guard in this hooks directory expects; None when the
/// pre-commit hook there is not GitSwitch's. Hooks written before the email
/// had its own comment line carry it as `expected="…"`.
pub(crate) fn guard_email_in(hooks: &Path) -> Option<String> {
    let text = std::fs::read_to_string(hooks.join("pre-commit")).ok()?;
    if !text.contains(MARKER) {
        return None;
    }
    let lines = || text.lines().map(str::trim);
    lines()
        .find_map(|l| l.strip_prefix("# gitswitch-expected-email:").map(|e| e.trim().to_string()))
        .or_else(|| lines().find_map(|l| l.strip_prefix("expected=\"").and_then(|e| e.strip_suffix('"')).map(str::to_string)))
}

/// The profile a repository commits as: by folder, else by the include file
/// git reads its email from (a remote rule's clone outside every folder).
pub(crate) fn profile_for_repo<'a>(repo: &Path, profs: &'a [profiles::Profile]) -> Option<&'a profiles::Profile> {
    let path = crate::paths::norm(&repo.to_string_lossy());
    owning_profile(&path, profs).or_else(|| {
        let origin = git(repo, &["config", "--show-origin", "--get", "user.email"]).ok()?;
        let file = origin.split('\t').next()?.trim().trim_start_matches("file:").to_string();
        profiles::profile_for_include_origin(&file, profs)
    })
}

/// Bring every GitSwitch guard under these folders in line with the profile
/// that now owns its repository: after a profile's email changed, or a folder
/// moved to another profile, a guard expecting the old email would refuse
/// every commit. A repository no profile owns any more keeps its guard — the
/// commit box offers to switch it off; protection is never dropped silently.
/// One line per guard rewritten.
pub async fn refresh_guards(dirs: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for dir in dirs {
        if let Ok(repos) = repo_scan::scan_repos(dir.clone()).await {
            for r in repos {
                if !paths.contains(&r.path) {
                    paths.push(r.path);
                }
            }
        }
    }
    tokio::task::spawn_blocking(move || {
        let Ok(store) = profiles::load_profiles() else { return Vec::new() };
        let mut out = Vec::new();
        for path in paths {
            let repo = PathBuf::from(&path);
            let Ok(hooks) = hooks_dir(&repo) else { continue };
            let Some(expects) = guard_email_in(&hooks.dir) else { continue };
            let Some(owner) = profile_for_repo(&repo, &store.profiles) else { continue };
            let email = owner.git_email.trim();
            if expects.trim().eq_ignore_ascii_case(email) {
                continue;
            }
            if install_guard(&path, email).is_ok() {
                out.push(format!("{}: the identity guard now expects {} (was {})", path, email, expects));
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

pub fn uninstall_guard(repo_path: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let hooks = hooks_dir(&repo).map(|h| h.dir).unwrap_or_else(|_| repo.join(".git").join("hooks"));
    let hook = hooks.join("pre-commit");
    if !hook.exists() {
        return Ok("No guard installed.".into());
    }
    let content = std::fs::read_to_string(&hook)?;
    if !content.contains(MARKER) {
        return Err(AppError::Config(
            "The pre-commit hook here isn't GitSwitch's — leaving it alone.".into(),
        ));
    }
    std::fs::remove_file(&hook)?;

    // Put back whatever we displaced when installing.
    let saved = hooks.join("pre-commit.gitswitch-saved");
    if saved.exists() {
        std::fs::rename(&saved, &hook)?;
        return Ok("Guard removed; your original pre-commit hook is restored.".into());
    }
    Ok("Guard removed.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Both of the audit's runners are the one runner; the rewrite's own
    /// variables still reach the script on top of the hardening.
    #[test]
    fn the_audit_runners_go_through_gitcmd() {
        use crate::git_exec::test_support::{assert_hardened, SHOW_ENV};
        let dir = std::env::temp_dir();
        assert_hardened(&git(&dir, &SHOW_ENV).expect("git runs"));
        let env = git_env(&dir, &SHOW_ENV, &[("GITSWITCH_FIX_WRONG", "<a@example.com>")]).expect("git runs");
        assert_hardened(&env);
        assert!(env.lines().any(|l| l.trim() == "GITSWITCH_FIX_WRONG=<a@example.com>"));
    }

    fn prof(name: &str, email: &str, dirs: &[&str]) -> profiles::Profile {
        profiles::Profile {
            id: name.into(),
            name: name.into(),
            git_name: name.into(),
            git_email: email.into(),
            ssh_key_path: None,
            is_default: false,
            allow_push: true,
            signing_enabled: false,
            remote_patterns: Vec::new(),
            github_login: None,
            directories: dirs.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            push_over_ssh: None,
        }
    }

    #[test]
    fn longest_matching_folder_owns_the_repo() {
        let profs = vec![
            prof("Personal", "me@home", &["/dev"]),
            prof("Work", "me@work", &["/dev/acme"]),
        ];
        assert_eq!(owning_profile("/dev/acme/api", &profs).unwrap().name, "Work");
        assert_eq!(owning_profile("/dev/side", &profs).unwrap().name, "Personal");
        assert!(owning_profile("/elsewhere/x", &profs).is_none());
    }

    #[test]
    fn trailing_slashes_do_not_break_ownership() {
        let profs = vec![prof("P", "e", &["/dev/work/"])];
        assert!(owning_profile("/dev/work/repo", &profs).is_some());
        // A sibling that merely shares a prefix must NOT match.
        assert!(owning_profile("/dev/workshop/repo", &profs).is_none());
    }

    #[test]
    fn hook_template_embeds_email_and_marker() {
        let body = hook_body("a@b.com", "");
        assert!(body.contains("expected='a@b.com'"));
        assert!(body.contains("# gitswitch-expected-email: a@b.com"));
        assert!(body.contains(MARKER));
        assert!(body.starts_with("#!/bin/sh"));
        // A quote in the email cannot end the shell's string.
        assert!(hook_body("o'neil@b.com", "").contains(r"expected='o'\''neil@b.com'"));
    }

    #[test]
    fn the_guard_email_is_read_back_from_new_and_old_hooks() {
        let dir = std::env::temp_dir().join(format!("gitswitch-guard-email-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hook = dir.join("pre-commit");
        std::fs::write(&hook, hook_body("new@x.test", "")).unwrap();
        assert_eq!(guard_email_in(&dir).as_deref(), Some("new@x.test"));
        // Written by a build before the email had a line of its own.
        std::fs::write(&hook, format!("#!/bin/sh\n# >>> {} >>>\nexpected=\"old@x.test\"\n", MARKER)).unwrap();
        assert_eq!(guard_email_in(&dir).as_deref(), Some("old@x.test"));
        // Somebody else's hook: no guard of ours to read.
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(guard_email_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
