use crate::error::AppError;
use serde::Serialize;
use crate::profiles::{self, Profile};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct SparseInfo {
    pub path: String,
    pub branch: String,
    pub is_sparse: bool,
    /// Folders currently in the sparse-checkout set (git sparse-checkout list).
    pub sparse_dirs: Vec<String>,
    /// Top-level folders that exist in the repo (git ls-tree -d).
    pub available_dirs: Vec<String>,
}

async fn run_git(repo: Option<&Path>, args: &[&str]) -> Result<String, AppError> {
    let mut cmd = tokio::process::Command::new("git");
    if let Some(dir) = repo {
        cmd.current_dir(dir);
    }
    // No terminal exists for a GUI app, so an HTTPS credential prompt would
    // hang forever — make git fail fast with a readable error instead.
    let output = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run git: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Command(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Derive the repo folder name from a clone URL (strip .git, take last segment).
fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let last = trimmed.rsplit(['/', ':']).next()?;
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

/// Where a clone of `url` would land: (parent dir, folder name, full path).
fn destination_path(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<(PathBuf, String, PathBuf), AppError> {
    let parent = PathBuf::from(parent_dir);
    if !parent.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", parent_dir)));
    }
    let name = match folder_name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => repo_name_from_url(url)
            .ok_or_else(|| AppError::Config("Could not derive a folder name from the URL".into()))?,
    };
    // A plain folder name only — never a path that escapes the chosen parent.
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(AppError::Config(format!("\"{}\" isn't a valid folder name", name)));
    }
    let dest = parent.join(&name);
    Ok((parent, name, dest))
}

/// Resolve and validate where a clone will land — it must not exist yet.
fn clone_destination(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<(PathBuf, String, PathBuf), AppError> {
    let (parent, name, dest) = destination_path(url, parent_dir, folder_name)?;
    if dest.exists() {
        return Err(AppError::Config(format!("{} already exists", dest.display())));
    }
    Ok((parent, name, dest))
}

#[derive(Debug, Serialize)]
pub struct DestinationStatus {
    pub path: String,
    pub exists: bool,
    pub is_repo: bool,
    /// The existing repo's origin, credentials masked.
    pub origin: Option<String>,
    /// That origin is the same GitHub repository as the URL being cloned.
    pub same_repo: bool,
    /// ...and already exactly this URL.
    pub same_url: bool,
    pub origin_is_https: bool,
    /// A repo folder with no commit: either a clone that was interrupted or an
    /// empty repository. Either way it can't be cloned into or switched.
    pub incomplete: bool,
}

fn same_github_repo(a: &str, b: &str) -> bool {
    match (
        crate::repo_scan::parse_github_owner_name(a),
        crate::repo_scan::parse_github_owner_name(b),
    ) {
        (Some((ao, an)), Some((bo, bn))) => ao.eq_ignore_ascii_case(&bo) && an.eq_ignore_ascii_case(&bn),
        _ => false,
    }
}

fn is_http_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("https://") || u.starts_with("http://")
}

/// Is the clone destination already taken — and if so, by this same repo?
/// Local reads only: no fetch, no network.
pub async fn destination_status(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<DestinationStatus, AppError> {
    let (_, _, dest) = match destination_path(url, parent_dir, folder_name) {
        Ok(d) => d,
        // no usable destination yet — nothing is "taken"
        Err(_) => {
            return Ok(DestinationStatus {
                path: String::new(), exists: false, is_repo: false, origin: None,
                same_repo: false, same_url: false, origin_is_https: false, incomplete: false,
            })
        }
    };
    let exists = dest.exists();
    let is_repo = dest.join(".git").exists();
    let origin = if is_repo {
        run_git(Some(&dest), &["remote", "get-url", "origin"]).await.ok()
    } else {
        None
    };
    let incomplete = is_repo && run_git(Some(&dest), &["rev-parse", "--verify", "HEAD"]).await.is_err();
    Ok(DestinationStatus {
        path: dest.to_string_lossy().to_string(),
        exists,
        is_repo,
        same_repo: origin.as_deref().is_some_and(|o| same_github_repo(o, url)),
        same_url: origin.as_deref().is_some_and(|o| o.trim() == url.trim()),
        origin_is_https: origin.as_deref().is_some_and(is_http_url),
        origin: origin.as_deref().map(crate::repo_scan::mask_token),
        incomplete,
    })
}

/// Point an existing clone's `origin` at `url` — e.g. from HTTPS to the SSH
/// link, so pulls and pushes go through the profile's SSH key. Deliberately
/// narrow: only `origin`'s URL changes, and only to another address of the
/// SAME repository, so this can never silently retarget a repo.
pub async fn switch_origin(repo_path: &str, url: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let url = url.trim();
    if !repo.join(".git").exists() {
        return Err(AppError::Config(format!("{} is not a git repository", repo_path)));
    }
    if url.is_empty() || url.starts_with('-') {
        return Err(AppError::Config("That isn't a valid repository URL.".into()));
    }
    if is_http_url(url) {
        return Err(AppError::Config(
            "Switching to an HTTPS link wouldn't use your SSH key — use the SSH link (Code → SSH).".into(),
        ));
    }
    if run_git(Some(&repo), &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return Err(AppError::Config(
            "That folder has no commits — it looks like a clone that was interrupted. Delete it and clone again.".into(),
        ));
    }
    let current = run_git(Some(&repo), &["remote", "get-url", "origin"])
        .await
        .map_err(|_| AppError::Config("This repo has no \"origin\" remote to switch.".into()))?;
    if !same_github_repo(&current, url) {
        return Err(AppError::Config(format!(
            "Refusing to switch: origin is {}, which is a different repository than {}.",
            crate::repo_scan::mask_token(&current),
            url
        )));
    }
    run_git(Some(&repo), &["remote", "set-url", "--", "origin", url]).await?;
    Ok(format!(
        "origin switched from {} to {}",
        crate::repo_scan::mask_token(&current),
        url
    ))
}

#[derive(Debug, Serialize)]
pub struct CloneResult {
    pub path: String,
    /// Profile whose SSH key authenticated the clone (None = no profile at all).
    pub profile_name: Option<String>,
    /// Set when the new folder was added to that profile, so later pulls and
    /// pushes in it keep using the same key.
    pub mapped_to: Option<String>,
    /// Present when submodules were requested.
    pub submodules: Option<SubmoduleReport>,
    /// Present whenever the repository uses Git LFS — whether or not the
    /// download was requested, so the result can say what state the large
    /// files are in either way.
    pub lfs: Option<LfsReport>,
}

/// What happened to the large files after a checkout. Measured by counting
/// pointer stubs before and after, like the Changes page's LFS card.
#[derive(Debug, Serialize, Clone)]
pub struct LfsReport {
    pub installed: bool,
    pub requested: bool,
    pub tracked: usize,
    pub fetched: usize,
    pub pointers_left: usize,
    /// `git lfs install --local` had to be run first (a fresh clone with no
    /// global LFS filters is exactly the state in which `git lfs pull` does nothing).
    pub configured_now: bool,
    pub error: Option<String>,
    /// One sentence, written here so the Clone page and its result say the same thing.
    pub note: String,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

/// After a clone or a sparse checkout: download the LFS content when asked
/// and possible, and in every case say what state the large files are in.
/// `None` when the repository does not use LFS. Never an error — the
/// repository is usable and the Changes page can retry.
pub(crate) async fn lfs_after_checkout(repo: &Path, requested: bool) -> Option<LfsReport> {
    let repo_str = repo.to_string_lossy().to_string();
    let before = crate::lfs::lfs_status(&repo_str).await.ok()?;
    if !before.uses_lfs {
        return None;
    }
    let stubs = |n: usize| plural(n, "large file is a pointer stub", "large files are pointer stubs");
    let go_to_changes = "open the repository in Changes → Git LFS → Pull LFS files";
    if requested && before.installed && before.pointers > 0 {
        return Some(match crate::git_ops::fetch_lfs_content(&repo_str).await {
            Ok(f) => {
                let note = if let Some(a) = &f.error {
                    format!("The large files could not be downloaded: {} To retry, {}.", a.headline, go_to_changes)
                } else if f.after.pointers == 0 {
                    format!("Downloaded {} (Git LFS).", plural(f.fetched, "large file", "large files"))
                } else {
                    format!("Downloaded {}; {} — {}.", plural(f.fetched, "large file", "large files"), stubs(f.after.pointers), go_to_changes)
                };
                LfsReport {
                    installed: true,
                    requested,
                    tracked: f.after.tracked,
                    fetched: f.fetched,
                    pointers_left: f.after.pointers,
                    configured_now: f.configured_now,
                    error: f.error.map(|a| a.headline),
                    note,
                }
            }
            Err(e) => LfsReport {
                installed: true,
                requested,
                tracked: before.tracked,
                fetched: 0,
                pointers_left: before.pointers,
                configured_now: false,
                error: Some(e.to_string()),
                note: format!("The large files could not be downloaded ({}). To retry, {}.", e, go_to_changes),
            },
        });
    }
    let note = if !before.installed {
        format!(
            "This repository uses Git LFS, but git-lfs isn't installed on this computer, so {}. Install it ({}) and then {}.",
            stubs(before.pointers),
            crate::lfs::install_hint(),
            go_to_changes
        )
    } else if before.pointers == 0 {
        format!("All {} present (Git LFS).", plural(before.tracked, "large file is", "large files are"))
    } else {
        format!("{} — {} to download {}.", stubs(before.pointers), go_to_changes, if before.pointers == 1 { "it" } else { "them" })
    };
    Some(LfsReport {
        installed: before.installed,
        requested,
        tracked: before.tracked,
        fetched: 0,
        pointers_left: before.pointers,
        configured_now: false,
        error: None,
        note,
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct SubmoduleReport {
    /// Submodules that have an address in .gitmodules.
    pub listed: usize,
    /// Of those, how many are now checked out.
    pub downloaded: usize,
    /// Submodule entries with NO address in .gitmodules — git can't fetch
    /// these, so they stay empty (as in any clone).
    pub unlisted: usize,
    /// Which listed submodules failed, by path. The loop below already knows;
    /// collapsing them into one error string threw the detail away.
    pub failed_paths: Vec<String>,
    pub error: Option<String>,
}

/// Download the submodules a repo lists in `.gitmodules` — the same end state
/// as `git clone --recurse-submodules`, but sturdier when one of them fails.
///
/// Why not just pass `--recurse-submodules` to the clone: if one submodule is
/// refused, git stops before checking out the others it already cloned, and
/// no later `git submodule update` fills those folders in. So each listed
/// path gets its own call. (A bare `git submodule update --init` is no good
/// either: it aborts on the first entry with no address in .gitmodules —
/// some repos have dozens — and downloads nothing.)
pub(crate) async fn update_submodules(repo: &Path, ssh_override: Option<&str>, key: Option<&str>, url: &str) -> SubmoduleReport {
    // `-z` (`key\nvalue\0` records): a submodule's name defaults to its path,
    // and a path with a space would otherwise split the key in the middle.
    let listed: Vec<String> = run_git(Some(repo), &["config", "-f", ".gitmodules", "--get-regexp", "-z", r"\.path$"])
        .await
        .map(|out| {
            out.split('\0')
                .filter_map(|rec| rec.split_once('\n').map(|(_, p)| p.trim().to_string()))
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let gitlinks = run_git(Some(repo), &["ls-files", "-s"])
        .await
        .map(|out| out.lines().filter(|l| l.starts_with("160000 ")).count())
        .unwrap_or(0);
    let mut report = SubmoduleReport {
        listed: listed.len(),
        downloaded: 0,
        unlisted: gitlinks.saturating_sub(listed.len()),
        failed_paths: Vec::new(),
        error: None,
    };
    // Leave the repo exactly as `git clone --recurse-submodules` does: every
    // submodule counts as active, so later `git pull --recurse-submodules` /
    // `git submodule update` also fetch submodules the team adds afterwards.
    // (Set even when there are none yet — the team command does too.)
    let _ = run_git(Some(repo), &["config", "submodule.active", "."]).await;
    if listed.is_empty() {
        return report;
    }

    // One submodule per git call. In a single batch, one refused submodule
    // makes git stop before checking out the ones it DID clone — and a retry
    // then thinks they're up to date, leaving empty folders behind.
    let mut failed: Vec<String> = Vec::new();
    let mut first_error: Option<String> = None;
    for path in &listed {
        // The hardened runner: a network timeout, no credential prompt that
        // could hang a GUI forever, and the ssh override as a `-c` for this
        // command only.
        let mut cmd = crate::git_exec::GitCmd::at(repo);
        if let Some(kv) = ssh_override {
            cmd = cmd.cfg_raw(kv);
        }
        let res = cmd
            .args(["submodule", "update", "--init", "--recursive", "--", path])
            .timeout(crate::git_exec::NET_TIMEOUT)
            .text()
            .await;
        if let Err(e) = res {
            failed.push(path.clone());
            if first_error.is_none() {
                first_error = Some(match e {
                    AppError::Command(msg) => explain_clone_error(&msg, key, url),
                    other => other.to_string(),
                });
            }
        }
    }
    report.failed_paths = failed.clone();
    if let Some(err) = first_error {
        report.error = Some(format!(
            "Couldn't download: {}.\n\n{}\n\nOnce that's sorted, run `git submodule update --recursive` inside the repo to fetch {}.",
            failed.join(", "),
            err,
            if failed.len() == 1 { "it" } else { "them" }
        ));
    }
    let mut downloaded = 0;
    for path in &listed {
        if is_checked_out(&repo.join(path)).await {
            downloaded += 1;
        }
    }
    report.downloaded = downloaded;
    report
}

/// A submodule counts as downloaded only if its files are actually there:
/// cloned-but-never-checked-out leaves `.git` with an empty folder.
async fn is_checked_out(dir: &Path) -> bool {
    if !dir.join(".git").exists() || run_git(Some(dir), &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return false;
    }
    let head_has_files = run_git(Some(dir), &["ls-tree", "--name-only", "HEAD"])
        .await
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false);
    if !head_has_files {
        return true; // an empty commit has nothing to check out
    }
    // A checked-out tree has a populated index; `--no-checkout` leaves none.
    match run_git(Some(dir), &["rev-parse", "--absolute-git-dir"]).await {
        Ok(gitdir) => std::fs::metadata(PathBuf::from(gitdir.trim()).join("index"))
            .map(|m| m.len() > 32)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// The profile git applies to `path`: longest matching folder, else the default.
pub(crate) fn resolve_profile<'a>(path: &str, profs: &'a [Profile]) -> Option<&'a Profile> {
    crate::commit_audit::owning_profile(path, profs).or_else(|| profs.iter().find(|p| p.is_default))
}

/// Same command GitSwitch writes into the per-profile gitconfig.
fn ssh_command_for(key: &str) -> String {
    format!("ssh -i {} -o IdentitiesOnly=yes", crate::paths::norm(key))
}

/// GitHub shows `org-<ID>@github.com:org/repo.git` instead of `git@…` when an
/// organization uses SSH certificate authorities.
pub(crate) fn uses_org_certificate_url(url: &str) -> bool {
    let u = url.trim();
    let u = u.strip_prefix("ssh://").unwrap_or(u);
    u.split_once('@')
        .map(|(user, rest)| user.starts_with("org-") && rest.starts_with("github.com"))
        .unwrap_or(false)
}

/// Turn git/ssh's terse failures into something that says what to do.
pub(crate) fn explain_clone_error(stderr: &str, key: Option<&str>, url: &str) -> String {
    let key_name = key
        .map(crate::paths::base_name)
        .unwrap_or_else(|| "your default SSH key".into());
    let refused = stderr.contains("Permission denied (publickey)")
        || stderr.contains("Repository not found")
        || stderr.contains("Could not read from remote repository");

    let mut hint = if stderr.contains("SAML SSO") {
        format!(
            "This organization uses single sign-on, and the SSH key {} hasn't been authorized for it yet. \
             On GitHub (signed in as the company account): Settings → SSH and GPG keys → Configure SSO next to this key → Authorize for the organization. Then clone again.",
            key_name
        )
    } else if stderr.contains("Host key verification failed") {
        "This Mac doesn't trust github.com's SSH host key yet. Run `ssh -T git@github.com` once in Terminal, answer \"yes\", then clone again.".to_string()
    } else if stderr.contains("Permission denied (publickey)") {
        format!("GitHub refused the SSH key {}. Either it isn't registered on the GitHub account, or the organization doesn't accept it (see below). Register it from SSH Keys, or pick another profile under \"Clone as\".", key_name)
    } else if stderr.contains("Repository not found") || stderr.contains("Could not read from remote repository") {
        format!("GitHub accepted the key {}, but that account can't see this repository. Check the URL, or under \"Clone as\" pick the profile whose account has access (e.g. your company profile for a company repo).", key_name)
    } else if stderr.contains("could not read Username") || stderr.contains("terminal prompts disabled") {
        "This HTTPS URL needs a username and password, which GitSwitch can't type in for you. Use the SSH URL (git@github.com:owner/repo.git) instead.".to_string()
    } else {
        String::new()
    };

    // The org-<ID>@ link means the organization uses SSH certificates. If it
    // *requires* them, GitHub refuses plain keys (and HTTPS) outright, so the
    // generic "not registered / no access" advice above would mislead.
    if refused && uses_org_certificate_url(url) && !stderr.contains("SAML SSO") {
        hint.push_str(&format!(
            "\n\nThis link (org-…@github.com) means the organization uses SSH certificates. If it requires them, a plain key is refused even when it's registered: \
             ask your company admin for a certificate for {}, and save it next to the key as {}-cert.pub — ssh picks it up automatically.",
            key_name, key_name
        ));
    }

    if hint.is_empty() {
        stderr.trim().to_string()
    } else {
        format!("{}\n\ngit said: {}", hint.trim_start(), stderr.trim())
    }
}

#[derive(Debug, Serialize)]
pub struct CertInfo {
    /// Where ssh looks for it: `<key>-cert.pub`.
    pub path: String,
    pub exists: bool,
    /// "forever" or an end timestamp, as ssh-keygen prints it.
    pub valid_until: Option<String>,
    pub expired: bool,
}

/// Is there a CA-signed certificate next to this private key? ssh loads
/// `<key>-cert.pub` automatically whenever `-i <key>` is used. Reads only the
/// public certificate file — no private key, no network.
pub async fn certificate_info(key_path: &str) -> CertInfo {
    let path = format!("{}-cert.pub", key_path);
    let mut info = CertInfo { exists: PathBuf::from(&path).is_file(), path, valid_until: None, expired: false };
    if !info.exists {
        return info;
    }
    if let Ok(out) = run_git_like("ssh-keygen", &["-L", "-f", &info.path]).await {
        if let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with("Valid:")) {
            let v = line.trim_start_matches("Valid:").trim();
            if v == "forever" {
                info.valid_until = Some("forever".into());
            } else if let Some(end) = v.rsplit(" to ").next() {
                info.valid_until = Some(end.to_string());
                // ssh-keygen prints local time as YYYY-MM-DDTHH:MM:SS
                if let Ok(t) = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%dT%H:%M:%S") {
                    info.expired = t < chrono::Local::now().naive_local();
                }
            }
        }
    }
    info
}

async fn run_git_like(bin: &str, args: &[&str]) -> Result<String, AppError> {
    let out = tokio::process::Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run {}: {}", bin, e)))?;
    if !out.status.success() {
        return Err(AppError::Command(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Clone `url` into `parent_dir`, authenticating either as the profile that
/// owns the destination folder (`clone_as` = None — exactly what a terminal
/// `git clone` would do) or as an explicitly chosen profile.
///
/// With an explicit profile the key is passed via `git -c core.sshCommand=…`,
/// which applies to this one command only. (`git clone -c …` would instead
/// store the key in the new repo's own config, where it would silently beat
/// GitSwitch's rules forever after.) The new folder is then added to that
/// profile, so pulls and pushes in it keep using the same account.
async fn clone_as_profile(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
    extra_args: &[&str],
    with_submodules: bool,
    with_lfs: bool,
) -> Result<CloneResult, AppError> {
    let (parent, name, dest) = clone_destination(url, parent_dir, folder_name)?;
    let dest_str = dest.to_string_lossy().to_string();

    let store = profiles::load_profiles()?;
    let folder_profile = resolve_profile(&dest_str, &store.profiles).cloned();
    let chosen = match clone_as.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => Some(
            store
                .profiles
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", id)))?,
        ),
        None => None,
    };
    let acting = chosen.clone().or_else(|| folder_profile.clone());
    let key = acting.as_ref().and_then(|p| p.ssh_key_path.clone());
    if let (Some(p), None) = (chosen.as_ref(), key.as_ref()) {
        return Err(AppError::Config(format!(
            "Profile \"{}\" has no SSH key, so it can't authenticate an SSH clone. Add a key to it first.",
            p.name
        )));
    }

    // Only when a profile was chosen explicitly; otherwise the folder's
    // includeIf rule already picks the key, exactly like a terminal clone.
    let ssh_override = chosen
        .as_ref()
        .map(|_| format!("core.sshCommand={}", ssh_command_for(key.as_deref().unwrap_or_default())));

    let mut args: Vec<String> = Vec::new();
    if let Some(cmd) = ssh_override.as_ref() {
        args.push("-c".into());
        args.push(cmd.clone());
    }
    args.push("clone".into());
    args.extend(extra_args.iter().map(|a| a.to_string()));
    // `--` so a URL or folder name starting with "-" can never be read as an option.
    args.push("--".into());
    args.push(url.trim().to_string());
    args.push(name);

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_git(Some(&parent), &arg_refs)
        .await
        .map_err(|e| match e {
            AppError::Command(msg) => AppError::Command(explain_clone_error(&msg, key.as_deref(), url)),
            other => other,
        })?;

    // The main clone succeeded; submodule trouble is reported, not fatal —
    // the repo is usable and `git submodule update` can be retried.
    let submodules = if with_submodules {
        Some(update_submodules(&dest, ssh_override.as_deref(), key.as_deref(), url).await)
    } else {
        None
    };
    // Same rule for the large files: reported, never fatal. A `--no-checkout`
    // clone has no work tree yet, so its LFS step waits for `sparse_set`.
    let lfs = if extra_args.contains(&"--no-checkout") {
        None
    } else {
        lfs_after_checkout(&dest, with_lfs).await
    };

    let mut mapped_to = None;
    if let Some(p) = chosen.as_ref() {
        let already = folder_profile.as_ref().is_some_and(|f| f.id == p.id);
        if !already {
            let mut dirs = p.directories.clone();
            dirs.push(dest_str.clone());
            profiles::update_profile(p.id.clone(), None, None, None, None, Some(dirs), None, None)
                .and_then(|_| crate::git_config::apply_git_config())
                .map_err(|e| {
                    AppError::Config(format!(
                        "Cloned into {}, but couldn't add it to profile \"{}\" ({}). Add the folder in Profiles so pulls and pushes use the same account.",
                        dest_str, p.name, e
                    ))
                })?;
            mapped_to = Some(p.name.clone());
        }
    }

    Ok(CloneResult {
        path: dest_str,
        profile_name: acting.map(|p| p.name),
        mapped_to,
        submodules,
        lfs,
    })
}

/// Partial clone (metadata only): `git clone --filter=blob:none --no-checkout`.
/// Nothing is materialized until folders are selected.
pub async fn sparse_clone(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
) -> Result<CloneResult, AppError> {
    clone_as_profile(url, parent_dir, folder_name, clone_as, &["--filter=blob:none", "--no-checkout"], false, false).await
}

/// Ordinary full clone — what `git clone <url>` does in a terminal.
pub async fn full_clone(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
    with_submodules: bool,
    with_lfs: bool,
) -> Result<CloneResult, AppError> {
    clone_as_profile(url, parent_dir, folder_name, clone_as, &[], with_submodules, with_lfs).await
}

/// Inspect a repo: default branch, top-level folders, and current sparse config.
pub async fn repo_info(repo_path: &str) -> Result<SparseInfo, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.join(".git").exists() {
        return Err(AppError::Config(format!("Not a git repository: {}", repo_path)));
    }

    let branch = run_git(Some(&repo), &["rev-parse", "--abbrev-ref", "HEAD"]).await?;

    // Tree objects are fetched even with --filter=blob:none, so this works
    // offline right after a partial clone.
    let available_dirs: Vec<String> = run_git(Some(&repo), &["ls-tree", "-d", "--name-only", "HEAD"])
        .await
        .map(|out| out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();

    let is_sparse = run_git(Some(&repo), &["config", "--get", "core.sparseCheckout"])
        .await
        .map(|v| v == "true")
        .unwrap_or(false);

    let sparse_dirs: Vec<String> = if is_sparse {
        run_git(Some(&repo), &["sparse-checkout", "list"])
            .await
            .map(|out| out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(SparseInfo {
        path: repo.to_string_lossy().to_string(),
        branch,
        is_sparse,
        sparse_dirs,
        available_dirs,
    })
}

#[derive(Debug, Serialize)]
pub struct SparseSetResult {
    /// Present when the repository uses Git LFS (see `lfs_after_checkout`).
    pub lfs: Option<LfsReport>,
}

/// Set the sparse folder selection (replaces the current set) and materialize:
/// `sparse-checkout init --cone` → `sparse-checkout set <dirs>` → `checkout <branch>`,
/// then the LFS step for what is now checked out.
pub async fn sparse_set(repo_path: &str, dirs: Vec<String>, with_lfs: bool) -> Result<SparseSetResult, AppError> {
    if dirs.is_empty() {
        return Err(AppError::Config("Select at least one folder".into()));
    }
    let repo = PathBuf::from(repo_path);

    run_git(Some(&repo), &["sparse-checkout", "init", "--cone"]).await?;

    let mut args: Vec<&str> = vec!["sparse-checkout", "set"];
    args.extend(dirs.iter().map(String::as_str));
    run_git(Some(&repo), &args).await?;

    // Materialize the working tree (no-op if already checked out).
    let branch = run_git(Some(&repo), &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    run_git(Some(&repo), &["checkout", &branch]).await?;

    Ok(SparseSetResult { lfs: lfs_after_checkout(&repo, with_lfs).await })
}

/// Add folders to an existing sparse checkout: `git sparse-checkout add <dirs>`.
pub async fn sparse_add(repo_path: &str, dirs: Vec<String>) -> Result<(), AppError> {
    if dirs.is_empty() {
        return Err(AppError::Config("Select at least one folder to add".into()));
    }
    let repo = PathBuf::from(repo_path);
    let mut args: Vec<&str> = vec!["sparse-checkout", "add"];
    args.extend(dirs.iter().map(String::as_str));
    run_git(Some(&repo), &args).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(id: &str, dirs: &[&str], default: bool) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            git_name: id.into(),
            git_email: format!("{id}@example.com"),
            ssh_key_path: Some(format!("/keys/{id}")),
            is_default: default,
            allow_push: true,
            signing_enabled: false,
            directories: dirs.iter().map(|d| d.to_string()).collect(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn destination_folder_decides_the_profile_not_its_parent() {
        // company owns /w/Work; personal owns a more specific path inside it
        let profs = vec![
            prof("personal", &["/w/Work/side-project"], true),
            prof("company", &["/w/Work"], false),
        ];
        let pick = |p: &str| resolve_profile(p, &profs).map(|x| x.id.clone());
        assert_eq!(pick("/w/Work/new-repo").as_deref(), Some("company"));
        assert_eq!(pick("/w/Work/side-project").as_deref(), Some("personal"));
        // not covered by anything -> the default profile, like git's global config
        assert_eq!(pick("/w/Elsewhere/repo").as_deref(), Some("personal"));
        // a name that merely shares a prefix is not "inside"
        assert_eq!(pick("/w/Work2/repo").as_deref(), Some("personal"));
    }

    #[test]
    fn rejects_folder_names_that_escape_the_parent() {
        let dir = std::env::temp_dir();
        let d = dir.to_string_lossy();
        for bad in ["..", ".", "a/b", "..\\x"] {
            assert!(
                clone_destination("git@github.com:o/r.git", &d, Some(bad)).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn clone_errors_say_what_to_do() {
        let e = explain_clone_error(
            "ERROR: Repository not found.\nfatal: Could not read from remote repository.",
            Some("/Users/me/.ssh/id_ed25519_company"),
            "git@github.com:acme/api.git",
        );
        assert!(e.contains("id_ed25519_company"), "{e}");
        assert!(e.contains("Clone as"), "{e}");

        let e = explain_clone_error("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.", None, "git@github.com:o/r.git");
        assert!(e.contains("isn't registered"), "publickey must win over the generic message: {e}");
        assert!(!e.contains("certificate"), "plain git@ links must not talk about certificates: {e}");

        let e = explain_clone_error("Host key verification failed.", None, "git@github.com:o/r.git");
        assert!(e.contains("ssh -T git@github.com"), "{e}");

        let e = explain_clone_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled", None, "https://github.com/o/r");
        assert!(e.contains("SSH URL"), "{e}");

        assert_eq!(explain_clone_error("fatal: something else", None, "git@github.com:o/r.git"), "fatal: something else");
    }

    #[test]
    fn same_repository_across_url_forms() {
        let ssh = "org-1234567@github.com:AcmeOrg/widgets.git";
        assert!(same_github_repo("https://github.com/AcmeOrg/widgets.git", ssh));
        assert!(same_github_repo("https://github.com/acmeorg/WIDGETS", ssh), "GitHub names are case-insensitive");
        assert!(same_github_repo("git@github.com:AcmeOrg/widgets.git", ssh));
        assert!(!same_github_repo("https://github.com/AcmeOrg/widgets-docs.git", ssh));
        assert!(!same_github_repo("https://gitlab.com/AcmeOrg/widgets.git", ssh));
        assert!(is_http_url(" HTTPS://github.com/o/r"));
        assert!(!is_http_url(ssh));
    }

    #[test]
    fn org_certificate_links_are_recognised() {
        assert!(uses_org_certificate_url("org-1234567@github.com:AcmeOrg/widgets.git"));
        assert!(uses_org_certificate_url("ssh://org-1@github.com/o/r.git"));
        assert!(!uses_org_certificate_url("git@github.com:o/r.git"));
        assert!(!uses_org_certificate_url("https://github.com/o/r"));
        assert!(!uses_org_certificate_url("org-1@gitlab.com:o/r.git"));
        // the folder name still comes from the repo, not the org-… user
        assert_eq!(repo_name_from_url("org-1234567@github.com:AcmeOrg/widgets.git").as_deref(), Some("widgets"));
        assert_eq!(
            crate::repo_scan::parse_github_owner_name("org-1234567@github.com:AcmeOrg/widgets.git"),
            Some(("AcmeOrg".into(), "widgets".into()))
        );
    }

    #[test]
    fn refusals_on_certificate_orgs_mention_certificates() {
        let url = "org-1234567@github.com:AcmeOrg/widgets.git";
        let key = Some("/Users/me/.ssh/id_company");
        for stderr in [
            "org-1234567@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "ERROR: Repository not found.\nfatal: Could not read from remote repository.",
        ] {
            let e = explain_clone_error(stderr, key, url);
            assert!(e.contains("SSH certificates"), "{e}");
            assert!(e.contains("id_company-cert.pub"), "{e}");
        }
    }

    #[test]
    fn saml_sso_error_points_at_configure_sso() {
        let e = explain_clone_error(
            "ERROR: The 'AcmeOrg' organization has enabled or enforced SAML SSO. To access this repository, you must use the HTTPS remote with a personal access token or SSH with an SSH key and passphrase that has been authorized for this organization.\nfatal: Could not read from remote repository.",
            Some("/Users/me/.ssh/id_company"),
            "org-1234567@github.com:AcmeOrg/widgets.git",
        );
        assert!(e.contains("Configure SSO"), "{e}");
        assert!(!e.contains("SSH certificates"), "SSO is its own fix: {e}");
    }

    #[test]
    fn ssh_command_matches_what_gitconfig_writes() {
        assert_eq!(
            ssh_command_for(r"C:\Users\me\.ssh\id_key"),
            "ssh -i C:/Users/me/.ssh/id_key -o IdentitiesOnly=yes"
        );
    }

    #[test]
    fn derives_repo_name_from_common_url_forms() {
        assert_eq!(
            repo_name_from_url("git@github.com:acme/widgets.git").as_deref(),
            Some("widgets")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/acme/widgets.git").as_deref(),
            Some("widgets")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/o/repo/").as_deref(),
            Some("repo")
        );
        assert_eq!(repo_name_from_url(""), None);
    }
}
