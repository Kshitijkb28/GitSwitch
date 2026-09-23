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
use crate::git_exec::GitCmd;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// How many pointer paths to send to the UI before summarising the rest.
const MAX_LISTED: usize = 50;

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

    let (tracked, pointer_paths) = if uses_lfs && installed {
        GitCmd::at(&repo)
            .args(["lfs", "ls-files"])
            .ok_text()
            .await
            .map(|out| parse_ls_files(&out))
            .unwrap_or((0, Vec::new()))
    } else {
        (0, Vec::new())
    };
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
}
