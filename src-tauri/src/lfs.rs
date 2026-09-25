//! Git LFS: whether a repository's large files are actually on disk, and
//! fetching the ones that aren't.
//!
//! A clone made without `git-lfs` installed — or with `GIT_LFS_SKIP_SMUDGE=1`,
//! which some CI images and clone tools set — leaves every LFS-tracked file as
//! a ~130-byte *pointer*: a text stub naming the object. The repo looks
//! complete, builds fail in confusing ways, and nothing in `git status` says
//! so. `git lfs ls-files` does: it marks each tracked file `*` (content
//! present) or `-` (still a pointer).

use crate::error::AppError;
use crate::git_exec::{GitCmd, NET_TIMEOUT};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How many pointer paths to send to the UI before summarising the rest.
const MAX_LISTED: usize = 50;
/// How many individual files the browser is given. Folder totals are computed
/// over every file first, so the counts stay right even when the rows are cut.
const MAX_FILES: usize = 5000;
/// And how many folders. A repository with more than this is browsed by the
/// folders that fit; the rest are reachable through "everything".
const MAX_FOLDERS: usize = 2000;

#[derive(Debug, Serialize, Clone)]
pub struct LfsStatus {
    /// `git lfs` answers at all.
    pub installed: bool,
    pub version: Option<String>,
    /// Some tracked `.gitattributes` routes files through the lfs filter.
    pub uses_lfs: bool,
    /// The repository's own config has the lfs filters (`git lfs install
    /// --local` has been run here, or globally). Without them `git lfs pull`
    /// exits 0 and silently checks out nothing: "Skipping object checkout,
    /// Git LFS is not installed for this repository."
    pub filters_configured: bool,
    /// LFS-tracked files in the current checkout.
    pub tracked: usize,
    /// Of those, how many are still pointer stubs — the number `git lfs pull`
    /// would fetch.
    pub pointers: usize,
    pub pointer_paths: Vec<String>,
    pub more_pointers: usize,
    /// One plain sentence.
    pub summary: String,
}

/// Parse `git lfs ls-files`: one `<oid> <*|-> <path>` line per tracked file.
/// `*` means the content is present, `-` that the file is still a pointer.
/// Returns (tracked, pointer paths). The path is everything after the second
/// space, so paths containing spaces survive.
pub fn parse_ls_files(out: &str) -> (usize, Vec<String>) {
    let mut tracked = 0usize;
    let mut pointers = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(_oid), Some(marker), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        tracked += 1;
        if marker == "-" {
            pointers.push(path.to_string());
        }
    }
    (tracked, pointers)
}

/// One LFS-tracked file in the current checkout.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct LfsFile {
    /// Root-relative, exactly as git-lfs reports it.
    pub path: String,
    /// The folder holding it, "" for the repository root.
    pub dir: String,
    /// Bytes the real content takes — known even while the file is a stub.
    pub size: u64,
    /// The real content is in the working file (git-lfs `*`).
    pub present: bool,
    /// The object is in this clone's local store, whether or not the working
    /// file holds it. Such a file needs no download, only a checkout.
    pub downloaded: bool,
}

/// A folder's totals, so a person can download one folder without expanding it.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct LfsFolder {
    pub path: String,
    /// Depth from the root, for indenting without re-parsing the path.
    pub depth: usize,
    pub files: usize,
    pub missing: usize,
    pub bytes: u64,
    pub missing_bytes: u64,
}

/// Every LFS file in the checkout, grouped so the UI can offer one folder, one
/// file, or everything.
#[derive(Debug, Serialize, Clone)]
pub struct LfsListing {
    pub installed: bool,
    pub version: Option<String>,
    pub uses_lfs: bool,
    pub filters_configured: bool,
    pub files: Vec<LfsFile>,
    pub folders: Vec<LfsFolder>,
    pub total: usize,
    pub present: usize,
    pub missing: usize,
    pub total_bytes: u64,
    pub missing_bytes: u64,
    /// An older git-lfs without `ls-files --json` reports no sizes, so the UI
    /// must not print "0 B" as though it had measured them.
    pub sizes_known: bool,
    /// Files beyond `MAX_FILES` that were not sent.
    pub truncated: usize,
    pub folders_truncated: usize,
    pub summary: String,
}

/// Parse `git lfs ls-files --json`. Returns `None` when the output is not the
/// JSON shape at all (an older git-lfs that doesn't know the flag), so the
/// caller can fall back to the marker form rather than report an empty repo.
pub fn parse_ls_json(out: &str) -> Option<Vec<LfsFile>> {
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    let files = v.get("files")?.as_array()?;
    let mut list = Vec::with_capacity(files.len());
    for f in files {
        let Some(path) = f.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        list.push(LfsFile {
            dir: parent_dir(path).to_string(),
            path: path.to_string(),
            size: f.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
            present: f.get("checkout").and_then(|c| c.as_bool()).unwrap_or(false),
            downloaded: f.get("downloaded").and_then(|c| c.as_bool()).unwrap_or(false),
        });
    }
    Some(list)
}

/// The folder part of a root-relative path; "" for a file at the root.
pub fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// Folder totals for every ancestor of every file, so selecting `assets` counts
/// what is inside `assets/textures` too.
pub fn folders_from(files: &[LfsFile]) -> Vec<LfsFolder> {
    let mut acc: BTreeMap<&str, (usize, usize, u64, u64)> = BTreeMap::new();
    for f in files {
        let mut dir = f.dir.as_str();
        loop {
            if dir.is_empty() {
                break;
            }
            let e = acc.entry(dir).or_insert((0, 0, 0, 0));
            e.0 += 1;
            e.2 += f.size;
            if !f.present {
                e.1 += 1;
                e.3 += f.size;
            }
            dir = parent_dir(dir);
        }
    }
    acc.into_iter()
        .map(|(path, (files, missing, bytes, missing_bytes))| LfsFolder {
            depth: path.matches('/').count(),
            path: path.to_string(),
            files,
            missing,
            bytes,
            missing_bytes,
        })
        .collect()
}

/// A path the UI may ask to download. It is matched against the repository's
/// own listing before any git runs, so this only has to reject what could not
/// be a tracked path in the first place — and never an option or an escape.
pub fn validate_lfs_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("An empty path can't be downloaded.".into());
    }
    if path.len() > 1024 {
        // Slicing by bytes would panic in the middle of a multi-byte
        // character — and this function exists to reject bad input safely.
        let head: String = path.chars().take(60).collect();
        return Err(format!("That path is too long to pass to git: {}…", head));
    }
    if path.starts_with('-') {
        return Err(format!("`{}` starts with a dash, which git would read as an option.", path));
    }
    if path.starts_with('/') || path.starts_with('\\') || is_drive_letter(path) {
        return Err(format!("`{}` is an absolute path; only paths inside the repository can be downloaded.", path));
    }
    if path.split(['/', '\\']).any(|seg| seg == ".." || seg == ".git") {
        return Err(format!("`{}` leaves the repository.", path));
    }
    Ok(())
}

/// `C:` or `C:/…` — a Windows absolute path. A colon anywhere else is an
/// ordinary character in a Unix filename and must not be mistaken for one.
fn is_drive_letter(path: &str) -> bool {
    let mut c = path.chars();
    matches!(c.next(), Some(a) if a.is_ascii_alphabetic())
        && c.next() == Some(':')
        && matches!(c.next(), None | Some('/') | Some('\\'))
}

/// A literal path written as the pattern that matches only itself.
///
/// EVERY path given to git-lfs is a wildmatch pattern — the `--include` list
/// and `git lfs checkout`'s arguments alike. A file really named `st*r.bin`
/// would otherwise fetch and write out `star.bin` as well, and `x[1].bin`
/// would fetch `x1.bin` instead of itself while the app blamed the server for
/// the file that never arrived. One-member character classes make each
/// metacharacter literal; a backslash would too, but a backslash is a path
/// separator on Windows, so it is used only for a backslash in a name, which
/// can only happen where it is not a separator.
pub fn literal_pattern(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for ch in path.chars() {
        match ch {
            '*' | '?' | '[' => {
                out.push('[');
                out.push(ch);
                out.push(']');
            }
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}

/// The `--include` value for `git lfs fetch`.
///
/// git-lfs splits this option on commas, so a path that itself contains one
/// can't be written literally. `?` matches any single character in the same
/// pattern language, so the comma becomes `?`: it still matches the file, and
/// the worst case is fetching a same-shaped neighbour that is never checked
/// out, because the checkout step is given the exact paths.
pub fn include_patterns(paths: &[String]) -> String {
    paths
        .iter()
        .map(|p| {
            let trimmed = p.trim_end_matches('/');
            // Make the name literal first, then turn its commas into the one
            // wildcard that has to stay: git-lfs splits this option on commas,
            // so a comma cannot be written any other way. That can fetch a
            // same-shaped neighbour's object; the checkout step is exact, so
            // nothing outside the selection is ever written.
            format!("/{}", literal_pattern(trimmed).replace(',', "?"))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Split a selection into runs small enough to pass to git as arguments.
/// Without this a very large selection would have to fall back to fetching and
/// writing out the whole repository, which is not what was asked for.
pub fn chunk_selection(paths: &[String]) -> Vec<Vec<String>> {
    const MAX_CHARS: usize = 8000;
    const MAX_PATHS: usize = 100;
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut len = 0usize;
    for p in paths {
        let cost = p.len() * 2 + 2;
        if !current.is_empty() && (current.len() >= MAX_PATHS || len + cost > MAX_CHARS) {
            chunks.push(std::mem::take(&mut current));
            len = 0;
        }
        len += cost;
        current.push(p.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Removes the progress file however the operation ends — including a timeout,
/// which returns early and would otherwise leave a finished run's numbers on
/// screen the next time anything ran in this repository.
pub struct ProgressFile(pub PathBuf);

impl ProgressFile {
    pub fn start_at(path: PathBuf) -> Self {
        let _ = std::fs::write(&path, "");
        ProgressFile(path)
    }
}

impl Drop for ProgressFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One line of `GIT_LFS_PROGRESS`: `<direction> <n>/<total> <bytes>/<total> <path>`.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct LfsProgress {
    pub file: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: u64,
}

pub fn parse_progress_line(line: &str) -> Option<LfsProgress> {
    let mut parts = line.splitn(4, ' ');
    let _direction = parts.next()?;
    let counts = parts.next()?;
    let bytes = parts.next()?;
    let file = parts.next()?.trim();
    let (done, total) = counts.split_once('/')?;
    let (got, want) = bytes.split_once('/')?;
    if file.is_empty() {
        return None;
    }
    Some(LfsProgress {
        file: file.to_string(),
        done: done.parse().ok()?,
        total: total.parse().ok()?,
        bytes: got.parse().ok()?,
        total_bytes: want.parse().ok()?,
    })
}

/// The last line git-lfs wrote, which is the file it is working on now.
pub fn latest_progress(contents: &str) -> Option<LfsProgress> {
    contents.lines().rev().find_map(parse_progress_line)
}

/// "2.4 GB" — sizes are the whole point of this feature, so they are written
/// the way a person reads them rather than in bytes.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{} B", n);
    }
    let mut v = n as f64;
    let mut u = 0usize;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[u])
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

/// The sentence shown on the card. Pure, so the wording is testable.
pub fn summarise(installed: bool, uses_lfs: bool, tracked: usize, pointers: usize) -> String {
    if !uses_lfs {
        return "This repository doesn't use Git LFS.".to_string();
    }
    if !installed {
        return "This repository uses Git LFS, but git-lfs isn't installed on this Mac — large files will stay as pointer stubs until it is.".to_string();
    }
    if tracked == 0 {
        return "Git LFS is set up here, but no files in this checkout are tracked by it.".to_string();
    }
    if pointers == 0 {
        return format!(
            "All {} LFS file{} present.",
            tracked,
            if tracked == 1 { " is" } else { "s are" }
        );
    }
    format!(
        "{} of {} LFS file{} still a pointer stub — the real content hasn't been downloaded.",
        pointers,
        tracked,
        if pointers == 1 { " is" } else { "s are" }
    )
}

/// git's own failure when a repository requires the LFS filters but git-lfs
/// is missing: `git-lfs filter-process: git-lfs: command not found` followed by
/// `fatal: the remote end hung up unexpectedly`. That second line reads like a
/// network outage, and it takes down `git status` itself — so it is translated
/// wherever git output becomes a message a person sees.
pub fn missing_lfs_message(stderr: &str) -> Option<String> {
    let mentions_lfs = stderr.contains("git-lfs");
    let missing = stderr.contains("command not found")
        || stderr.contains("No such file or directory")
        || stderr.contains("not found");
    if mentions_lfs && missing {
        Some(
            "This repository requires Git LFS (its config sets filter.lfs.required), but git-lfs isn't installed on this Mac, so git refuses to read its files. Install it with `brew install git-lfs` and try again."
                .to_string(),
        )
    } else {
        None
    }
}

/// Does any tracked `.gitattributes` route files through LFS? One process,
/// and no dependency on git-lfs being installed.
pub async fn repo_uses_lfs(repo: &Path) -> bool {
    GitCmd::at(repo)
        .args([
            "grep",
            "-l",
            "--cached",
            "-e",
            "filter=lfs",
            "--",
            ".gitattributes",
            "*.gitattributes",
        ])
        .ok_text()
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// `filter.lfs.required` is what `git lfs install` writes; its absence is the
/// exact state in which `git lfs pull` does nothing and still exits 0.
pub async fn filters_configured(repo: &Path) -> bool {
    GitCmd::at(repo)
        .args(["config", "--get", "filter.lfs.required"])
        .ok_text()
        .await
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Whether git-lfs is on this computer at all — asked by the Clone page
/// before any repository exists. `git lfs version` needs no repository.
#[derive(Debug, Serialize, Clone)]
pub struct LfsTool {
    pub installed: bool,
    pub version: Option<String>,
    /// How to get it on this platform, for the UI.
    pub install_hint: String,
}

pub fn install_hint() -> String {
    if cfg!(target_os = "macos") {
        "brew install git-lfs".into()
    } else if cfg!(target_os = "linux") {
        "apt install git-lfs (or your distribution's package)".into()
    } else {
        "the Git LFS installer from git-lfs.com".into()
    }
}

pub async fn lfs_available() -> LfsTool {
    let version = lfs_version(&std::env::temp_dir()).await;
    LfsTool { installed: version.is_some(), version, install_hint: install_hint() }
}

pub async fn lfs_version(repo: &Path) -> Option<String> {
    GitCmd::at(repo)
        .args(["lfs", "version"])
        .ok_text()
        .await
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Where git-lfs writes its per-file progress for a run in this repository.
///
/// Inside the repository's own git directory, never in the shared temp folder:
/// a predictable name under a world-writable `/tmp` can be pre-created or
/// pointed elsewhere by another user on the same machine, and git-lfs would
/// then write through it.
pub fn progress_file_in(git_dir: &Path) -> PathBuf {
    git_dir.join("gitswitch-lfs.progress")
}

/// The repository's git directory, found without starting a process.
///
/// The progress file is read once a second while a download runs, and spawning
/// `git rev-parse` for each of those polls is exactly the kind of cost this
/// feature exists to remove. `.git` is a directory in an ordinary clone and a
/// file holding `gitdir: <path>` in a submodule or a linked worktree.
pub fn git_dir_of(repo_path: &str) -> PathBuf {
    let repo = PathBuf::from(repo_path);
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        return dot_git;
    }
    if let Ok(text) = std::fs::read_to_string(&dot_git) {
        if let Some(rest) = text.trim().strip_prefix("gitdir:") {
            let target = PathBuf::from(rest.trim());
            return if target.is_absolute() { target } else { repo.join(target) };
        }
    }
    dot_git
}

pub fn progress_path(repo_path: &str) -> PathBuf {
    progress_file_in(&git_dir_of(repo_path))
}

/// How far the download running in this repository has got, or `None` when
/// nothing has been reported yet.
pub fn read_progress(repo_path: &str) -> Option<LfsProgress> {
    let raw = std::fs::read_to_string(progress_path(repo_path)).ok()?;
    latest_progress(&raw)
}

/// One scan of the checkout, shared by the card's counts and the file browser
/// so the two can never disagree and a big repository is walked once.
///
/// `--json` carries the size and whether the object is already in the local
/// store; an older git-lfs that doesn't know the flag falls back to the marker
/// form, which the same UI renders with sizes it doesn't have.
async fn scan_files(repo: &Path) -> (Vec<LfsFile>, bool) {
    let json = GitCmd::at(repo)
        .args(["lfs", "ls-files", "--json"])
        // A scan is a read, but it stats every tracked file: a repository with
        // tens of thousands of them takes longer than an ordinary read.
        .timeout(NET_TIMEOUT)
        .run()
        .await;
    if let Ok(out) = &json {
        if out.ok() {
            if let Some(files) = parse_ls_json(&out.text()) {
                return (files, true);
            }
        }
    }
    let fallback = GitCmd::at(repo)
        .args(["lfs", "ls-files"])
        .timeout(NET_TIMEOUT)
        .ok_text()
        .await
        .map(|out| {
            let (_, pointers) = parse_ls_files(&out);
            let missing: std::collections::HashSet<&str> = pointers.iter().map(|s| s.as_str()).collect();
            out.lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, ' ');
                    let (_oid, _marker, path) = (parts.next()?, parts.next()?, parts.next()?);
                    if path.is_empty() {
                        return None;
                    }
                    let present = !missing.contains(path);
                    Some(LfsFile {
                        dir: parent_dir(path).to_string(),
                        path: path.to_string(),
                        size: 0,
                        present,
                        downloaded: present,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (fallback, false)
}

/// Every large file in the checkout, with folder totals — what the browser
/// needs to offer one file, one folder, or all of them.
pub async fn lfs_files(repo_path: &str) -> Result<LfsListing, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.is_dir() {
        return Err(AppError::NotFound(format!("{} no longer exists on disk", repo_path)));
    }
    let uses_lfs = repo_uses_lfs(&repo).await;
    let version = lfs_version(&repo).await;
    let installed = version.is_some();
    let filters_configured = if installed { filters_configured(&repo).await } else { false };
    let (files, sizes_known) = if uses_lfs && installed { scan_files(&repo).await } else { (Vec::new(), true) };

    let total = files.len();
    let present = files.iter().filter(|f| f.present).count();
    let missing = total - present;
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let missing_bytes: u64 = files.iter().filter(|f| !f.present).map(|f| f.size).sum();

    // Folders are aggregated over every file before the rows are capped, so a
    // folder's count is the truth even when its files were not all sent.
    let mut folders = folders_from(&files);
    let folders_truncated = folders.len().saturating_sub(MAX_FOLDERS);
    // Shallowest first, so whatever survives the cap still has every one of its
    // ancestors — a folder whose parent was dropped would be unreachable, and
    // its files with it.
    folders.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.path.cmp(&b.path)));
    folders.truncate(MAX_FOLDERS);
    folders.sort_by(|a, b| a.path.cmp(&b.path));

    let mut files = files;
    // Missing files first: they are the ones a person came here to fetch.
    files.sort_by(|a, b| a.present.cmp(&b.present).then_with(|| a.path.cmp(&b.path)));
    let truncated = total.saturating_sub(MAX_FILES);
    files.truncate(MAX_FILES);
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(LfsListing {
        summary: summarise(installed, uses_lfs, total, missing),
        installed,
        version,
        uses_lfs,
        filters_configured,
        files,
        folders,
        total,
        present,
        missing,
        total_bytes,
        missing_bytes,
        sizes_known,
        truncated,
        folders_truncated,
    })
}

impl LfsListing {
    /// The card's shape, from the same scan the browser already did — so the
    /// two never disagree and a big repository is walked once per action.
    pub fn to_status(&self) -> LfsStatus {
        let pointer_paths: Vec<String> =
            self.files.iter().filter(|f| !f.present).map(|f| f.path.clone()).collect();
        LfsStatus {
            installed: self.installed,
            version: self.version.clone(),
            uses_lfs: self.uses_lfs,
            filters_configured: self.filters_configured,
            tracked: self.total,
            pointers: self.missing,
            more_pointers: self.missing.saturating_sub(pointer_paths.len().min(MAX_LISTED)),
            pointer_paths: pointer_paths.into_iter().take(MAX_LISTED).collect(),
            summary: self.summary.clone(),
        }
    }

    /// Does this selected file or folder cover `path`?
    pub fn covers(selected: &str, path: &str) -> bool {
        let sel = selected.trim_end_matches('/');
        path == sel || path.starts_with(&format!("{}/", sel))
    }

    /// Is this a file or folder the repository actually has? The UI can only
    /// offer what a listing gave it, so anything else is a stale selection.
    pub fn knows(&self, path: &str) -> bool {
        let p = path.trim_end_matches('/');
        self.files.iter().any(|f| f.path == p) || self.folders.iter().any(|f| f.path == p)
    }
}

pub async fn lfs_status(repo_path: &str) -> Result<LfsStatus, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} no longer exists on disk",
            repo_path
        )));
    }
    let uses_lfs = repo_uses_lfs(&repo).await;
    let version = lfs_version(&repo).await;
    let installed = version.is_some();
    let filters = if installed { filters_configured(&repo).await } else { false };

    // The same one scan the browser uses, so a count on the card and a row in
    // the list can never disagree about whether a file is here.
    let (files, _sizes_known) = if uses_lfs && installed { scan_files(&repo).await } else { (Vec::new(), true) };
    let tracked = files.len();
    let pointer_paths: Vec<String> = files.iter().filter(|f| !f.present).map(|f| f.path.clone()).collect();
    let pointers = pointer_paths.len();
    let more_pointers = pointers.saturating_sub(MAX_LISTED);

    Ok(LfsStatus {
        summary: summarise(installed, uses_lfs, tracked, pointers),
        installed,
        version,
        uses_lfs,
        filters_configured: filters,
        tracked,
        pointers,
        pointer_paths: pointer_paths.into_iter().take(MAX_LISTED).collect(),
        more_pointers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_files_separates_present_content_from_pointers() {
        // The real argos output shape: 10-char oid, marker, path.
        let out = "a12c192097 - touchstones/Kong__insomnia_dataset.jsonl\n\
                   64217dadc2 * assets/logo.png\n\
                   b409503d27 - touchstones/expressjs__express_dataset.jsonl\n";
        let (tracked, pointers) = parse_ls_files(out);
        assert_eq!(tracked, 3);
        assert_eq!(
            pointers,
            vec![
                "touchstones/Kong__insomnia_dataset.jsonl",
                "touchstones/expressjs__express_dataset.jsonl"
            ]
        );
    }

    #[test]
    fn a_path_with_spaces_is_kept_whole() {
        let (_, pointers) = parse_ls_files("abc1234567 - data/my big file.bin\n");
        assert_eq!(pointers, vec!["data/my big file.bin"]);
    }

    #[test]
    fn garbage_lines_are_skipped_not_fatal() {
        let (tracked, pointers) = parse_ls_files("\nnonsense\nabc - ok.bin\n");
        assert_eq!(tracked, 1);
        assert_eq!(pointers, vec!["ok.bin"]);
    }

    #[test]
    fn a_missing_git_lfs_is_named_instead_of_blamed_on_the_network() {
        let stderr = "git-lfs filter-process: git-lfs: command not found\nfatal: the remote end hung up unexpectedly";
        let msg = missing_lfs_message(stderr).expect("recognised");
        assert!(msg.contains("brew install git-lfs"));
        assert!(msg.contains("filter.lfs.required"));
        // A genuine network failure must not be mistaken for it.
        assert!(missing_lfs_message("fatal: the remote end hung up unexpectedly").is_none());
        assert!(missing_lfs_message("git-lfs/3.8.0 (GitHub; darwin arm64)").is_none());
    }

    #[test]
    fn the_summary_says_what_a_person_needs_to_do() {
        assert!(summarise(true, false, 0, 0).contains("doesn't use Git LFS"));
        assert!(summarise(false, true, 0, 0).contains("isn't installed"));
        assert!(summarise(true, true, 0, 0).contains("no files"));
        assert_eq!(summarise(true, true, 1, 0), "All 1 LFS file is present.");
        assert_eq!(summarise(true, true, 4, 0), "All 4 LFS files are present.");
        // The argos case: every tracked file is still a stub.
        let msg = summarise(true, true, 5, 5);
        assert!(msg.starts_with("5 of 5 LFS files are still a pointer stub"), "{}", msg);
        assert!(summarise(true, true, 5, 1).starts_with("1 of 5 LFS file is still"));
    }

    fn f(path: &str, size: u64, present: bool) -> LfsFile {
        LfsFile { dir: parent_dir(path).to_string(), path: path.to_string(), size, present, downloaded: present }
    }

    #[test]
    fn the_json_listing_carries_sizes_and_what_is_already_here() {
        let out = r#"{"files":[
          {"name":"assets/big files/tex ture.bin","size":2048,"checkout":false,"downloaded":true,"oid":"aa"},
          {"name":"model.bin","size":4096,"checkout":true,"downloaded":true,"oid":"bb"}
        ]}"#;
        let files = parse_ls_json(out).expect("parsed");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "assets/big files/tex ture.bin");
        assert_eq!(files[0].dir, "assets/big files");
        assert_eq!(files[0].size, 2048);
        assert!(!files[0].present);
        // Fetched but not written out: nothing to download, only to check out.
        assert!(files[0].downloaded);
        assert_eq!(files[1].dir, "");
        assert!(files[1].present);
    }

    #[test]
    fn an_older_git_lfs_that_doesnt_know_json_is_not_read_as_an_empty_repo() {
        assert!(parse_ls_json("a12c192097 - model.bin\n").is_none());
        assert!(parse_ls_json("").is_none());
        assert!(parse_ls_json("Error: unknown flag: --json").is_none());
    }

    #[test]
    fn a_folder_counts_everything_beneath_it_not_just_its_own_files() {
        let files = vec![
            f("assets/tex/a.bin", 100, false),
            f("assets/tex/deep/b.bin", 200, false),
            f("assets/c.bin", 300, true),
            f("root.bin", 400, false),
        ];
        let folders = folders_from(&files);
        let get = |p: &str| folders.iter().find(|x| x.path == p).unwrap_or_else(|| panic!("no {}", p));
        assert_eq!(get("assets").files, 3);
        assert_eq!(get("assets").missing, 2);
        assert_eq!(get("assets").bytes, 600);
        assert_eq!(get("assets").missing_bytes, 300);
        assert_eq!(get("assets/tex").files, 2);
        assert_eq!(get("assets/tex/deep").files, 1);
        assert_eq!(get("assets/tex/deep").depth, 2);
        // A file at the root belongs to no folder.
        assert!(folders.iter().all(|x| !x.path.is_empty()));
    }

    #[test]
    fn selecting_a_folder_covers_what_is_inside_it_and_nothing_that_merely_starts_the_same() {
        assert!(LfsListing::covers("assets", "assets/a.bin"));
        assert!(LfsListing::covers("assets/", "assets/deep/a.bin"));
        assert!(LfsListing::covers("model.bin", "model.bin"));
        // "assets2" is a different folder, even though the name is a prefix.
        assert!(!LfsListing::covers("assets", "assets2/a.bin"));
        assert!(!LfsListing::covers("assets", "assetsX"));
        assert!(!LfsListing::covers("model.bin", "other/model.bin"));
    }

    #[test]
    fn include_patterns_are_anchored_and_survive_a_comma_in_a_path() {
        assert_eq!(include_patterns(&["assets/tex".into()]), "/assets/tex");
        assert_eq!(include_patterns(&["assets/tex/".into()]), "/assets/tex");
        assert_eq!(
            include_patterns(&["a.bin".into(), "b/c.bin".into()]),
            "/a.bin,/b/c.bin"
        );
        // git-lfs splits --include on commas, so the comma becomes a
        // single-character wildcard rather than a pattern break.
        assert_eq!(include_patterns(&["assets/we,ird.bin".into()]), "/assets/we?ird.bin");
    }

    #[test]
    fn a_path_that_could_escape_the_repository_or_be_read_as_an_option_is_refused() {
        assert!(validate_lfs_path("assets/a.bin").is_ok());
        assert!(validate_lfs_path("assets/big files/tex ture.bin").is_ok());
        assert!(validate_lfs_path("").is_err());
        assert!(validate_lfs_path("--upload").is_err());
        assert!(validate_lfs_path("/etc/passwd").is_err());
        assert!(validate_lfs_path("C:/Windows").is_err());
        assert!(validate_lfs_path("../secrets.bin").is_err());
        assert!(validate_lfs_path("a/../../b.bin").is_err());
        assert!(validate_lfs_path(".git/config").is_err());
    }

    #[test]
    fn progress_is_read_from_the_last_line_git_lfs_wrote() {
        let log = "download 1/3 100/900 a.bin\ndownload 2/3 500/900 b/c.bin\n";
        let p = latest_progress(log).expect("parsed");
        assert_eq!(p.done, 2);
        assert_eq!(p.total, 3);
        assert_eq!(p.bytes, 500);
        assert_eq!(p.total_bytes, 900);
        assert_eq!(p.file, "b/c.bin");
        // A path with spaces is the rest of the line, not a fourth field.
        let q = latest_progress("download 1/1 1/2 assets/big files/tex ture.bin").expect("parsed");
        assert_eq!(q.file, "assets/big files/tex ture.bin");
        assert!(latest_progress("").is_none());
        assert!(latest_progress("nonsense\n").is_none());
    }

    #[test]
    fn a_submodules_git_directory_is_found_without_starting_a_process() {
        // `.git` is a file holding `gitdir: …` in a submodule and in a linked
        // worktree; the progress file has to land in the real directory.
        let root = std::env::temp_dir().join(format!("gitswitch-gitdir-{}", std::process::id()));
        let repo = root.join("sub");
        let real = root.join("modules/sub");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::create_dir_all(&real).expect("real");
        std::fs::write(repo.join(".git"), format!("gitdir: {}\n", real.display())).expect("pointer");
        assert_eq!(git_dir_of(repo.to_str().unwrap()), real);

        // An ordinary clone: `.git` is the directory itself.
        let plain = root.join("plain");
        std::fs::create_dir_all(plain.join(".git")).expect("plain");
        assert_eq!(git_dir_of(plain.to_str().unwrap()), plain.join(".git"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_progress_file_lives_in_the_repository_not_in_shared_temp() {
        // A predictable name in a world-writable /tmp can be pre-created or
        // pointed elsewhere by another user, and git-lfs writes through it.
        let a = progress_file_in(Path::new("/x/one/.git"));
        let b = progress_file_in(Path::new("/x/two/.git"));
        assert_ne!(a, b);
        assert!(a.starts_with("/x/one/.git"), "{:?}", a);
        assert!(!a.starts_with(std::env::temp_dir()), "{:?}", a);
    }

    #[test]
    fn sizes_are_written_the_way_a_person_reads_them() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn a_very_long_path_is_refused_without_slicing_a_character_in_half() {
        // A 1025-byte path whose 60th byte falls inside a character: byte
        // slicing here would panic in the very function meant to reject it.
        let path = format!("a{}", "é".repeat(600));
        assert!(path.len() > 1024);
        let err = validate_lfs_path(&path).expect_err("too long");
        assert!(err.contains("too long"), "{}", err);
    }

    #[test]
    fn a_colon_in_a_unix_filename_is_not_a_windows_drive() {
        // Legal on macOS and Linux, and refusing it would make the file
        // undownloadable for no reason.
        assert!(validate_lfs_path("a:b.bin").is_ok());
        assert!(validate_lfs_path("notes/10:30 recording.bin").is_ok());
        // Still a drive letter, still refused.
        assert!(validate_lfs_path("C:").is_err());
        assert!(validate_lfs_path("C:/Windows/x.bin").is_err());
        assert!(validate_lfs_path("d:\\data\\x.bin").is_err());
    }

    #[test]
    fn a_name_holding_a_wildcard_is_written_as_the_pattern_matching_only_itself() {
        // git-lfs matches every path argument as a pattern. Unescaped,
        // `st*r.bin` also writes out `star.bin`, and `x[1].bin` fetches
        // `x1.bin` INSTEAD of itself.
        assert_eq!(literal_pattern("st*r.bin"), "st[*]r.bin");
        assert_eq!(literal_pattern("q?m.bin"), "q[?]m.bin");
        assert_eq!(literal_pattern("x[1].bin"), "x[[]1].bin");
        // A closing bracket on its own is already literal.
        assert_eq!(literal_pattern("a]b.bin"), "a]b.bin");
        // A backslash cannot be a separator where it is part of a name, so
        // doubling it is safe and is the only form git-lfs accepts.
        assert_eq!(literal_pattern("back\\slash.bin"), "back\\\\slash.bin");
        assert_eq!(literal_pattern("plain/file.bin"), "plain/file.bin");
        assert_eq!(literal_pattern("assets/big files/tex ture.bin"), "assets/big files/tex ture.bin");
    }

    #[test]
    fn include_patterns_escape_the_name_before_the_comma_becomes_a_wildcard() {
        // The comma must end up as the one wildcard that survives; the rest of
        // the name must not.
        assert_eq!(include_patterns(&["we,ird.bin".into()]), "/we?ird.bin");
        assert_eq!(include_patterns(&["st*r.bin".into()]), "/st[*]r.bin");
        assert_eq!(include_patterns(&["a,b*c.bin".into()]), "/a?b[*]c.bin");
    }

    #[test]
    fn a_huge_selection_is_split_rather_than_turned_into_everything() {
        let many: Vec<String> = (0..250).map(|i| format!("folder/file-{}.bin", i)).collect();
        let chunks = chunk_selection(&many);
        assert!(chunks.len() >= 3, "250 paths should not travel as one chunk");
        assert!(chunks.iter().all(|c| c.len() <= 100));
        // Nothing is lost and nothing is duplicated.
        let flat: Vec<&String> = chunks.iter().flatten().collect();
        assert_eq!(flat.len(), many.len());
        assert_eq!(*flat[0], many[0]);
        assert_eq!(*flat[249], many[249]);
        // One long path still gets a chunk of its own rather than being dropped.
        let long = vec!["x".repeat(9000)];
        assert_eq!(chunk_selection(&long).len(), 1);
        assert!(chunk_selection(&[]).is_empty());
    }

    #[test]
    fn the_progress_file_is_removed_even_when_the_operation_returns_early() {
        let dir = std::env::temp_dir().join(format!("gitswitch-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = progress_file_in(&dir);
        {
            let _guard = ProgressFile::start_at(path.clone());
            assert!(path.exists(), "the run should have created it");
        }
        assert!(!path.exists(), "it must not outlive the run that wrote it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
