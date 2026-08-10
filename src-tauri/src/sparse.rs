use crate::error::AppError;
use serde::Serialize;
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
    let output = cmd
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

/// Partial clone (metadata only): `git clone --filter=blob:none --no-checkout`.
/// Nothing is materialized until folders are selected. Returns the repo path.
pub async fn sparse_clone(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<String, AppError> {
    let parent = PathBuf::from(parent_dir);
    if !parent.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", parent_dir)));
    }
    let name = match folder_name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => repo_name_from_url(url)
            .ok_or_else(|| AppError::Config("Could not derive a folder name from the URL".into()))?,
    };
    let dest = parent.join(&name);
    if dest.exists() {
        return Err(AppError::Config(format!("{} already exists", dest.display())));
    }

    run_git(
        Some(&parent),
        &["clone", "--filter=blob:none", "--no-checkout", url.trim(), &name],
    )
    .await?;

    Ok(dest.to_string_lossy().to_string())
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

/// Set the sparse folder selection (replaces the current set) and materialize:
/// `sparse-checkout init --cone` → `sparse-checkout set <dirs>` → `checkout <branch>`.
pub async fn sparse_set(repo_path: &str, dirs: Vec<String>) -> Result<(), AppError> {
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

    Ok(())
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

    #[test]
    fn derives_repo_name_from_common_url_forms() {
        assert_eq!(
            repo_name_from_url("git@github.com:Ethara-Ai/unified-personas.git").as_deref(),
            Some("unified-personas")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/Ethara-Ai/unified-personas.git").as_deref(),
            Some("unified-personas")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/o/repo/").as_deref(),
            Some("repo")
        );
        assert_eq!(repo_name_from_url(""), None);
    }
}
