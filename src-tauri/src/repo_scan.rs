use crate::error::AppError;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct ScannedRepo {
    pub path: String,
    /// origin URL with any embedded token masked.
    pub remote_url: String,
    /// origin's own push URL (`remote.origin.pushurl`), masked the same way —
    /// only when it is set and differs from `remote_url`: pushes go there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_url: Option<String>,
    /// GitHub owner/name when the remote is a GitHub URL; empty otherwise.
    pub owner: String,
    pub name: String,
}

fn owner_name_from_path(path: &str) -> Option<(String, String)> {
    let p = path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut it = p.splitn(2, '/');
    let owner = it.next()?.trim();
    let name = it.next()?.trim();
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

/// Extract (owner, name) from any common GitHub remote URL form, including
/// ssh-config host aliases (git@github.com-work:o/r — the classic manual
/// multi-account setup) and port-qualified ssh (ssh://git@github.com:22/o/r,
/// ssh://git@ssh.github.com:443/o/r).
pub fn parse_github_owner_name(url: &str) -> Option<(String, String)> {
    let u = url.trim();

    // ssh://[user@]host[:port]/owner/repo
    if let Some(rest) = u.strip_prefix("ssh://") {
        let rest = match rest.split_once('@') {
            Some((_user, r)) => r,
            None => rest,
        };
        let (host_port, path) = rest.split_once('/')?;
        let host = host_port.split(':').next().unwrap_or(host_port);
        if host == "github.com" || host == "ssh.github.com" || host.starts_with("github.com-") {
            return owner_name_from_path(path);
        }
        return None;
    }

    // https/http://[creds@]github.com/owner/repo
    if let Some(after_scheme) = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
    {
        let host_rest = match after_scheme.split_once('@') {
            Some((_creds, rest)) => rest,
            None => after_scheme,
        };
        let (host, path) = host_rest.split_once('/')?;
        if host == "github.com" || host == "www.github.com" {
            return owner_name_from_path(path);
        }
        return None;
    }

    // scp-like: [user@]host:owner/repo — host may be an ssh-config alias
    // (github.com-work) that ultimately points at github.com.
    if let Some((host_part, path)) = u.split_once(':') {
        let host = host_part.rsplit('@').next().unwrap_or(host_part);
        if host == "github.com" || host.starts_with("github.com-") {
            return owner_name_from_path(path);
        }
    }

    None
}

/// Hide any credentials embedded in a remote URL — covers BOTH forms GitHub
/// documents: `https://user:token@…` and the token-as-username `https://token@…`.
pub(crate) fn mask_token(url: &str) -> String {
    // One redactor for the whole app. This used to be two byte-identical
    // copies that only understood a URL anchored at the start of the string;
    // `cmdlog::redact` also catches one quoted back mid-sentence by git, and
    // the bare `ghp_` token shapes. It still produces `***@`, which is what
    // Doctor's remote-embedded-token check looks for.
    crate::cmdlog::redact(url)
}

pub(crate) fn git_text(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let started = std::time::Instant::now();
    let d = dir.to_string_lossy().to_string();
    let out = crate::spawn::command("git").arg("-C").arg(&d).args(args).output().ok()?;
    crate::cmdlog::record(
        dir,
        ["git", "-C", &d].iter().map(|a| a.to_string()).chain(args.iter().map(|a| a.to_string())).collect(),
        out.status.code().unwrap_or(-1),
        started.elapsed(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    );
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// origin's URL, or — in a clone whose remote is called something else —
/// the first remote's.
/// The origin's URL as stored — or the first remote's, when there is no origin.
pub(crate) fn get_origin(dir: &std::path::Path) -> Option<String> {
    git_text(dir, &["remote", "get-url", "origin"]).or_else(|| {
        let first = git_text(dir, &["remote"])?.lines().next()?.trim().to_string();
        git_text(dir, &["remote", "get-url", &first])
    })
}

/// Folders the walk passes over — unless one is itself a repository (a
/// project called `build`, say).
pub(crate) fn noise_folder(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "Library" | "dist" | "build")
}

/// The repositories under `root`, as folders: up to five levels down, noise
/// folders passed over, a repository's own subfolders never entered, a clone
/// still being written left out. Nothing is asked of git — the Repository
/// page's list and Find repositories' "already listed" are this same walk.
pub(crate) fn walk_repo_dirs(root: &std::path::Path) -> Vec<PathBuf> {
    walk_repo_dirs_observed(root, &mut |_, _, _| true)
}

/// `walk_repo_dirs`, telling `observe` about each folder it takes up — the
/// folder, how many it has taken up so far and how many repositories it has
/// found — and stopping, with what it found until then, once `observe`
/// answers false. The walk itself is the same, folder for folder.
pub(crate) fn walk_repo_dirs_observed(root: &std::path::Path, observe: &mut dyn FnMut(&std::path::Path, usize, usize) -> bool) -> Vec<PathBuf> {
    walk(root, false, observe)
}

/// The walk itself. `with_clones`: a clone still being written counts as a
/// repository too — for asking whether a folder holds one at all.
fn walk(root: &std::path::Path, with_clones: bool, observe: &mut dyn FnMut(&std::path::Path, usize, usize) -> bool) -> Vec<PathBuf> {
    const MAX_DEPTH: u32 = 5;
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    let mut folders = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        folders += 1;
        if !observe(&dir, folders, out.len()) {
            break;
        }
        if dir.join(".git").exists() {
            // A clone still writing this folder: not a repository yet.
            if !with_clones && crate::sparse::clone_running_at(&dir.to_string_lossy()) {
                continue;
            }
            out.push(dir);
            // Don't descend into a repo's own subfolders.
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let dirname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if noise_folder(dirname) && !path.join(".git").exists() {
                    continue;
                }
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// What a folder is to git. A profile folder usually holds repositories; one
/// that holds none is still the person's project, and the Repository page
/// lists it rather than letting it vanish.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FolderState {
    /// Not on this computer, or not a folder.
    Missing,
    /// A repository itself.
    Repo,
    /// Inside the repository at `top`: a folder there can't be a repository
    /// of its own, and a profile's `gitdir:` rule for it never applies.
    InsideRepo { top: String },
    /// Holds repositories (a clone still being written counts).
    ContainsRepos { count: usize },
    /// Holds no repository and is in none: a project git doesn't know yet.
    NoRepo,
}

/// The repository `dir` is inside, as git finds it walking up: its top
/// folder, spelled the way this system writes paths.
pub(crate) fn enclosing_repo(dir: &std::path::Path) -> Option<String> {
    let top = git_text(dir, &["rev-parse", "--show-toplevel"])?;
    Some(if cfg!(windows) { top.replace('/', "\\") } else { top })
}

pub(crate) fn folder_state_sync(dir: &std::path::Path) -> FolderState {
    if !dir.is_dir() {
        return FolderState::Missing;
    }
    if dir.join(".git").exists() {
        return FolderState::Repo;
    }
    let count = walk(dir, true, &mut |_, _, _| true).len();
    if count > 0 {
        return FolderState::ContainsRepos { count };
    }
    match enclosing_repo(dir) {
        Some(top) => FolderState::InsideRepo { top },
        None => FolderState::NoRepo,
    }
}

/// `folder_state_sync` on a blocking thread: the walk can be large.
pub async fn folder_state(path: String) -> Result<FolderState, AppError> {
    tokio::task::spawn_blocking(move || folder_state_sync(std::path::Path::new(&path)))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))
}

/// One repository folder as the scan reports it: its origin (masked), the
/// GitHub owner/name behind it, and its own push URL.
pub(crate) fn scanned_repo(dir: &std::path::Path) -> ScannedRepo {
    let (remote_url, owner, name) = match get_origin(dir) {
        Some(url) => {
            let (o, n) = parse_github_owner_name(&url).unwrap_or_default();
            (mask_token(&url), o, n)
        }
        None => (String::new(), String::new(), String::new()),
    };
    // The raw value: `get-url --push` would apply a push block's rewrite.
    let push_url = git_text(dir, &["config", "--get", "remote.origin.pushurl"]).map(|u| mask_token(&u)).filter(|u| *u != remote_url);
    ScannedRepo {
        path: dir.to_string_lossy().to_string(),
        remote_url,
        push_url,
        owner,
        name,
    }
}

fn scan_sync(root: &str) -> Result<Vec<ScannedRepo>, AppError> {
    let root_path = PathBuf::from(root);
    if !root_path.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", root)));
    }
    let mut repos: Vec<ScannedRepo> = walk_repo_dirs(&root_path).iter().map(|d| scanned_repo(d)).collect();
    repos.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(repos)
}

/// Walk `root` (skipping noise dirs) and report every git repo with its origin.
/// Runs on a blocking thread — filesystem walks can be large.
pub async fn scan_repos(root: String) -> Result<Vec<ScannedRepo>, AppError> {
    tokio::task::spawn_blocking(move || scan_sync(&root))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// A folder scan the page can watch and stop: what `scan_repos` finds, the
/// folder it searched, and how many folders it took up.
#[derive(Debug, Serialize, Default)]
pub struct TrackedScan {
    pub root: String,
    pub repos: Vec<ScannedRepo>,
    pub folders: usize,
    /// Stopped before it ended: `repos` is then empty — half a list would
    /// read as the whole folder.
    pub cancelled: bool,
}

/// Progress is published every this many folders (and on each repository found).
const PROGRESS_EVERY: usize = 256;

pub(crate) fn scan_tracked_sync(root: &str, state: &crate::find_clones::ScanState) -> Result<TrackedScan, AppError> {
    let root_path = PathBuf::from(root);
    if !root_path.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", root)));
    }
    let mut out = TrackedScan { root: root.to_string(), ..Default::default() };
    let mut last_found = 0usize;
    let dirs = walk_repo_dirs_observed(&root_path, &mut |dir, folders, found| {
        if state.cancelled() {
            return false;
        }
        if folders == 1 || folders.is_multiple_of(PROGRESS_EVERY) || found != last_found {
            last_found = found;
            state.set(|p| {
                p.phase = "looking".into();
                p.folders = folders;
                p.found = found;
                p.place = dir.to_string_lossy().to_string();
            });
        }
        out.folders = folders;
        true
    });
    if state.cancelled() {
        out.cancelled = true;
        return Ok(out);
    }
    let total = dirs.len();
    state.set(|p| {
        p.phase = "reading".into();
        p.found = total;
        p.checked = 0;
        p.total = total;
    });
    for (i, d) in dirs.iter().enumerate() {
        if state.cancelled() {
            out.cancelled = true;
            out.repos.clear();
            return Ok(out);
        }
        state.set(|p| {
            p.checked = i;
            p.place = d.to_string_lossy().to_string();
        });
        out.repos.push(scanned_repo(d));
    }
    out.repos.sort_by(|a, b| a.path.cmp(&b.path));
    state.set(|p| p.checked = total);
    Ok(out)
}

/// `scan_repos`, with progress and Stop through the scan id (polled like
/// Find repositories' search).
pub async fn scan_tracked(scan_id: String, root: String) -> Result<TrackedScan, AppError> {
    let state = crate::find_clones::scan(&scan_id);
    tokio::task::spawn_blocking(move || scan_tracked_sync(&root, &state))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_github_url_forms() {
        let cases = [
            "git@github.com:acme/widgets.git",
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://user:ghp_tok@github.com/acme/widgets.git",
            "ssh://git@github.com/acme/widgets.git",
        ];
        for c in cases {
            assert_eq!(
                parse_github_owner_name(c),
                Some(("acme".to_string(), "widgets".to_string())),
                "failed for {c}"
            );
        }
    }

    #[test]
    fn rejects_non_github_and_malformed() {
        assert_eq!(parse_github_owner_name("https://gitlab.com/o/r.git"), None);
        assert_eq!(parse_github_owner_name("git@github.com:only-owner"), None);
        assert_eq!(parse_github_owner_name("git@gitlab.com:o/r.git"), None);
        assert_eq!(parse_github_owner_name(""), None);
    }

    #[test]
    fn parses_alias_and_port_qualified_forms() {
        // ssh-config host alias — the classic manual multi-account setup
        assert_eq!(
            parse_github_owner_name("git@github.com-work:acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
        // port-qualified ssh
        assert_eq!(
            parse_github_owner_name("ssh://git@github.com:22/acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
        // ssh-over-443 endpoint
        assert_eq!(
            parse_github_owner_name("ssh://git@ssh.github.com:443/acme/api.git"),
            Some(("acme".into(), "api".into()))
        );
    }

    #[test]
    fn mask_token_hides_all_credential_forms() {
        let m1 = mask_token("https://user:ghp_aaa@github.com/o/r.git");
        assert!(!m1.contains("ghp_aaa") && m1.contains("***@"));
        // token-as-username (no colon) — previously leaked unmasked
        let m2 = mask_token("https://ghp_bbb@github.com/o/r.git");
        assert!(!m2.contains("ghp_bbb") && m2.contains("***@"));
        let m3 = mask_token("http://ghp_ccc@github.com/o/r.git");
        assert!(!m3.contains("ghp_ccc"));
    }

    #[test]
    fn scan_finds_repos_and_skips_noise_dirs() {
        let base = std::env::temp_dir().join(format!("gitswitch_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // repo at top level, repo nested two deep, decoy inside node_modules,
        // and a repo-inside-a-repo that must NOT be descended into.
        std::fs::create_dir_all(base.join("repo_a/.git")).unwrap();
        std::fs::create_dir_all(base.join("repo_a/vendor/inner/.git")).unwrap();
        std::fs::create_dir_all(base.join("group/sub/repo_b/.git")).unwrap();
        std::fs::create_dir_all(base.join("node_modules/dep/.git")).unwrap();

        let repos = scan_sync(&base.to_string_lossy()).unwrap();
        let names: Vec<String> = repos
            .iter()
            .map(|r| crate::paths::base_name(&r.path))
            .collect();

        let has = |n: &str| names.iter().any(|x| x == n);
        assert!(has("repo_a"));
        assert!(has("repo_b"));
        assert!(!has("inner"), "must not descend into a repo");
        assert!(!has("dep"), "must skip node_modules");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_observed_walk_is_the_same_walk_and_can_stop() {
        let base = std::env::temp_dir().join(format!("gitswitch_scan_observed_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["a/.git", "a/vendor/inner/.git", "g/s/b/.git", "g/s/c/.git", "node_modules/dep/.git", "deep/1/2/3/4/5/six/.git", "x/y/z"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let plain = walk_repo_dirs(&base);
        let mut seen = Vec::new();
        let observed = walk_repo_dirs_observed(&base, &mut |d, n, found| {
            seen.push((d.to_path_buf(), n, found));
            true
        });
        assert_eq!(plain, observed, "same folders, same order");
        assert_eq!(seen.first().map(|s| (s.0.clone(), s.1, s.2)), Some((base.clone(), 1, 0)));
        assert!(seen.windows(2).all(|w| w[1].1 == w[0].1 + 1), "folders counted one by one");
        assert_eq!(seen.last().unwrap().2 + usize::from(seen.last().unwrap().0.join(".git").exists()), plain.len());
        // Stopped after three folders: what was found until then, no more.
        let stopped = walk_repo_dirs_observed(&base, &mut |_, n, _| n <= 3);
        assert!(stopped.len() <= plain.len() && plain.starts_with(&stopped), "{stopped:?} is a prefix of {plain:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_tracked_scan_reports_where_it_is_and_stops_with_nothing() {
        let base = std::env::temp_dir().join(format!("gitswitch_scan_tracked_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["one/.git", "two/.git", "deeper/three/.git"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let root = base.to_string_lossy().to_string();
        let state = crate::find_clones::ScanState::new();
        let out = scan_tracked_sync(&root, &state).unwrap();
        let names: Vec<String> = out.repos.iter().map(|r| crate::paths::base_name(&r.path)).collect();
        assert_eq!(names, vec!["three", "one", "two"], "sorted by path");
        assert!(!out.cancelled && out.root == root && out.folders >= 5, "{} folders", out.folders);
        let p = state.snapshot();
        assert_eq!((p.phase.as_str(), p.found, p.checked, p.total), ("reading", 3, 3, 3));

        let stopped = crate::find_clones::ScanState::new();
        stopped.stop();
        let out = scan_tracked_sync(&root, &stopped).unwrap();
        assert!(out.cancelled && out.repos.is_empty());
        assert!(scan_tracked_sync(&base.join("missing").to_string_lossy(), &crate::find_clones::ScanState::new()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn scan_finds_repositories_whatever_their_folder_is_called() {
        let base = std::env::temp_dir().join(format!("gitswitch-scan-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["build", "dist", "node_modules/dep", "plain"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
            let ok = crate::spawn::command("git").args(["init", "-q"]).arg(base.join(d)).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                return; // no git here: nothing to prove
            }
        }
        let _ = crate::spawn::command("git").arg("-C").arg(base.join("plain")).args(["remote", "add", "upstream", "git@github.com:acme/x.git"]).status();
        let found = scan_sync(&base.to_string_lossy()).unwrap();
        let names: Vec<String> = found.iter().map(|r| crate::paths::base_name(&r.path)).collect();
        assert!(names.contains(&"build".to_string()) && names.contains(&"dist".to_string()), "{names:?}");
        assert!(!names.contains(&"dep".to_string()), "a repository inside node_modules is not walked into: {names:?}");
        let plain = found.iter().find(|r| r.path.ends_with("plain")).unwrap();
        assert_eq!((plain.owner.as_str(), plain.name.as_str()), ("acme", "x"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_folder_is_told_apart_from_a_repository_a_folder_of_them_and_a_folder_inside_one() {
        // Only where git finds no repository above the temporary folder.
        if enclosing_repo(&std::env::temp_dir()).is_some() {
            return;
        }
        let base = std::env::temp_dir().join(format!("gitswitch-folder-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["project/app/src", "holder/one", "holder/two", "repo/sub"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        for r in ["holder/one", "holder/two", "repo"] {
            if !crate::spawn::command("git").args(["init", "-q"]).arg(base.join(r)).status().is_ok_and(|s| s.success()) {
                return; // no git here: nothing to prove
            }
        }
        assert_eq!(folder_state_sync(&base.join("project")), FolderState::NoRepo, "a project git doesn't know");
        assert_eq!(folder_state_sync(&base.join("holder")), FolderState::ContainsRepos { count: 2 });
        assert_eq!(folder_state_sync(&base.join("repo")), FolderState::Repo);
        match folder_state_sync(&base.join("repo/sub")) {
            FolderState::InsideRepo { top } => assert!(top.replace('\\', "/").ends_with("/repo"), "{top}"),
            other => panic!("a folder inside a repository: {other:?}"),
        }
        assert_eq!(folder_state_sync(&base.join("nowhere")), FolderState::Missing);
        let _ = std::fs::remove_dir_all(&base);
    }
}
