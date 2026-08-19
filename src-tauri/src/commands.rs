use crate::commit_audit::{self, RepoAudit};
use crate::credentials;
use crate::doctor::{self, Finding};
use crate::error::AppError;
use crate::gh_cli;
use crate::git_config;
use crate::git_history::{self, BranchInfo, CommitDetail, HistoryPage, MergeInfo, RepoRef};
use crate::git_remote::{self, RemoteChange};
use crate::github::{self, GitHubKey, GitHubUser, RepoPermissions};
use crate::repo_scan::{self, ScannedRepo};
use crate::oauth::{self, DeviceCodeResponse, OAuthTokenResponse};
use crate::profiles::{self, Profile};
use crate::signing;
use crate::sparse::{self, SparseInfo};
use crate::ssh_keys;

#[tauri::command]
pub fn get_profiles() -> Result<Vec<Profile>, AppError> {
    let store = profiles::load_profiles()?;
    Ok(store.profiles)
}

#[tauri::command]
pub fn create_profile(
    app: tauri::AppHandle,
    name: String,
    git_name: String,
    git_email: String,
    ssh_key_path: Option<String>,
    directories: Vec<String>,
    allow_push: Option<bool>,
    signing_enabled: Option<bool>,
) -> Result<Profile, AppError> {
    let profile = profiles::create_profile(
        name,
        git_name,
        git_email,
        ssh_key_path,
        directories,
        allow_push.unwrap_or(true),
        signing_enabled.unwrap_or(false),
    )?;
    git_config::apply_git_config()?;
    crate::tray::update_active_label(&app);
    Ok(profile)
}

#[tauri::command]
pub fn update_profile(
    app: tauri::AppHandle,
    id: String,
    name: Option<String>,
    git_name: Option<String>,
    git_email: Option<String>,
    ssh_key_path: Option<String>,
    directories: Option<Vec<String>>,
    allow_push: Option<bool>,
    signing_enabled: Option<bool>,
) -> Result<Profile, AppError> {
    let profile = profiles::update_profile(
        id, name, git_name, git_email, ssh_key_path, directories, allow_push, signing_enabled,
    )?;
    git_config::apply_git_config()?;
    crate::tray::update_active_label(&app);
    Ok(profile)
}

#[tauri::command]
pub fn delete_profile(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    credentials::delete_token(&id).ok();
    profiles::delete_profile(id)?;
    git_config::apply_git_config()?;
    crate::tray::update_active_label(&app);
    Ok(())
}

#[tauri::command]
pub fn set_default_profile(app: tauri::AppHandle, id: String) -> Result<Profile, AppError> {
    let profile = profiles::set_default_profile(id)?;
    git_config::apply_git_config()?;
    crate::tray::update_active_label(&app);
    Ok(profile)
}

#[tauri::command]
pub fn generate_ssh_key(profile_name: String, email: String) -> Result<(String, String), AppError> {
    ssh_keys::generate_ssh_key(&profile_name, &email)
}

#[tauri::command]
pub fn get_public_key(private_key_path: String) -> Result<String, AppError> {
    ssh_keys::get_public_key(&private_key_path)
}

#[tauri::command]
pub async fn test_connection() -> Result<String, AppError> {
    ssh_keys::test_ssh_connection().await
}

#[tauri::command]
pub async fn test_connection_with_key(key_path: String) -> Result<String, AppError> {
    ssh_keys::test_ssh_connection_with_key(&key_path).await
}

#[tauri::command]
pub fn list_ssh_keys() -> Result<Vec<String>, AppError> {
    ssh_keys::list_ssh_keys()
}

#[tauri::command]
pub fn delete_ssh_key(key_path: String) -> Result<(), AppError> {
    ssh_keys::delete_ssh_key(&key_path)
}

#[tauri::command]
pub async fn gh_list_accounts() -> Result<Vec<String>, AppError> {
    gh_cli::gh_list_accounts().await
}

#[tauri::command]
pub async fn gh_get_token(account: String) -> Result<String, AppError> {
    gh_cli::gh_get_token(&account).await
}

#[tauri::command]
pub async fn gh_logout(account: String) -> Result<String, AppError> {
    gh_cli::gh_logout(&account).await
}

/// Automate "Step 2": register a local key's public half on a specific gh account.
/// `key_path` is the PRIVATE key path; we read `<key_path>.pub`.
#[tauri::command]
pub async fn gh_register_ssh_key(
    account: String,
    key_path: String,
    title: String,
) -> Result<String, AppError> {
    let token = gh_cli::gh_get_token(&account).await?;
    let public_key = ssh_keys::get_public_key(&key_path)?;

    match github::upload_ssh_key(&token, &title, &public_key).await {
        Ok(k) => Ok(format!("Registered '{}' on {}", k.title, account)),
        Err(AppError::Command(msg))
            if msg.contains("already in use") || msg.contains("key is already") =>
        {
            Ok(format!("Key already registered on {}", account))
        }
        Err(e) => Err(e),
    }
}

/// Automate "Step 5": which GitHub account does this key authenticate as?
#[tauri::command]
pub async fn resolve_key_account(key_path: String) -> Result<Option<String>, AppError> {
    ssh_keys::resolve_key_account(&key_path).await
}

// --- Doctor: health checks, run as separate steps so the UI can show progress ---

#[tauri::command]
pub async fn doctor_check_environment() -> Result<Vec<Finding>, AppError> {
    doctor::check_environment().await
}

#[tauri::command]
pub async fn doctor_check_profiles() -> Result<Vec<Finding>, AppError> {
    doctor::check_profiles().await
}

#[tauri::command]
pub async fn doctor_check_keys() -> Result<Vec<Finding>, AppError> {
    doctor::check_keys().await
}

#[tauri::command]
pub async fn doctor_check_repos() -> Result<Vec<Finding>, AppError> {
    doctor::check_repos().await
}

#[tauri::command]
pub async fn doctor_fix_ssh_config() -> Result<String, AppError> {
    tokio::task::spawn_blocking(doctor::fix_ssh_config)
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Auto-assign: find every git repo under a folder with its GitHub owner/name.
#[tauri::command]
pub async fn scan_repos(root: String) -> Result<Vec<ScannedRepo>, AppError> {
    repo_scan::scan_repos(root).await
}

/// Auto-assign: what access does this token's account have on owner/repo?
/// None = the account can't even see the repo.
#[tauri::command]
pub async fn check_repo_access(
    token: String,
    owner: String,
    repo: String,
) -> Result<Option<RepoPermissions>, AppError> {
    github::repo_permission(&token, &owner, &repo).await
}

/// Sparse checkout: partial-clone a repo (blob:none, no checkout).
#[tauri::command]
pub async fn sparse_clone(
    url: String,
    parent_dir: String,
    folder_name: Option<String>,
) -> Result<String, AppError> {
    sparse::sparse_clone(&url, &parent_dir, folder_name.as_deref()).await
}

/// Sparse checkout: inspect a repo (branch, top-level folders, current sparse set).
#[tauri::command]
pub async fn sparse_repo_info(repo_path: String) -> Result<SparseInfo, AppError> {
    sparse::repo_info(&repo_path).await
}

/// Sparse checkout: replace the folder selection and materialize the tree.
#[tauri::command]
pub async fn sparse_set(repo_path: String, dirs: Vec<String>) -> Result<(), AppError> {
    sparse::sparse_set(&repo_path, dirs).await
}

/// Sparse checkout: add folders to the existing selection.
#[tauri::command]
pub async fn sparse_add(repo_path: String, dirs: Vec<String>) -> Result<(), AppError> {
    sparse::sparse_add(&repo_path, dirs).await
}

/// Automate "Step 4": convert HTTPS (incl. token) remotes to SSH for all repos under a folder.
/// Runs on a blocking thread — it walks the directory tree and shells out to git
/// per repo, which would freeze the UI if run as a sync command.
#[tauri::command]
pub async fn convert_repos_to_ssh(directory: String) -> Result<Vec<RemoteChange>, AppError> {
    tokio::task::spawn_blocking(move || git_remote::convert_repos_in_dir(&directory))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[tauri::command]
pub fn store_github_token(profile_id: String, token: String) -> Result<(), AppError> {
    credentials::store_token(&profile_id, &token)
}

#[tauri::command]
pub fn get_github_token(profile_id: String) -> Result<Option<String>, AppError> {
    credentials::get_token(&profile_id)
}

#[tauri::command]
pub fn apply_git_config() -> Result<(), AppError> {
    git_config::apply_git_config()
}

#[tauri::command]
pub fn get_current_git_config() -> Result<String, AppError> {
    git_config::get_current_git_config()
}

#[tauri::command]
pub async fn verify_github_token(token: String) -> Result<GitHubUser, AppError> {
    github::verify_token(&token).await
}

#[tauri::command]
pub async fn upload_ssh_key_to_github(
    token: String,
    title: String,
    public_key: String,
) -> Result<GitHubKey, AppError> {
    github::upload_ssh_key(&token, &title, &public_key).await
}

#[tauri::command]
pub async fn list_github_ssh_keys(token: String) -> Result<Vec<GitHubKey>, AppError> {
    github::list_ssh_keys_github(&token).await
}

#[tauri::command]
pub async fn github_device_code(client_id: Option<String>) -> Result<DeviceCodeResponse, AppError> {
    oauth::request_device_code(client_id.as_deref()).await
}

#[tauri::command]
pub async fn github_poll_token(
    device_code: String,
    client_id: Option<String>,
) -> Result<OAuthTokenResponse, AppError> {
    oauth::poll_for_token(&device_code, client_id.as_deref()).await
}

// --- Commit audit: find & fix commits made with the wrong identity ---

/// Scan every profile folder for commits whose author email doesn't match the
/// profile that owns that folder.
#[tauri::command]
pub async fn audit_commits() -> Result<Vec<RepoAudit>, AppError> {
    commit_audit::audit_all().await
}

/// Rewrite the author of UNPUSHED commits in one repo. Pushed history is never
/// touched, and the previous history is kept on a backup branch.
#[tauri::command]
pub async fn fix_unpushed_commits(repo_path: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || commit_audit::fix_unpushed(&repo_path))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Install a pre-commit hook that blocks commits with the wrong identity.
#[tauri::command]
pub async fn install_commit_guard(
    repo_path: String,
    expected_email: String,
) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || commit_audit::install_guard(&repo_path, &expected_email))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[tauri::command]
pub async fn uninstall_commit_guard(repo_path: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || commit_audit::uninstall_guard(&repo_path))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

// --- Commit signing (SSH): "Verified" badges, per profile ---

/// Register a profile's key on GitHub as a SIGNING key. This is a different key
/// type from an authentication key — auth-only keys never produce "Verified".
#[tauri::command]
pub async fn register_signing_key(account: String, key_path: String) -> Result<String, AppError> {
    let token = gh_cli::gh_get_token(&account).await?;
    let public_key = ssh_keys::get_public_key(&key_path)?;
    signing::register_signing_key(&token, "GitSwitch signing key", &public_key).await
}

/// Is this key already registered as a signing key on the account?
#[tauri::command]
pub async fn signing_key_registered(account: String, key_path: String) -> Result<bool, AppError> {
    let token = gh_cli::gh_get_token(&account).await?;
    let public_key = ssh_keys::get_public_key(&key_path)?;
    signing::signing_key_registered(&token, &public_key).await
}

/// Path of the allowed_signers file GitSwitch maintains (shown in the UI).
#[tauri::command]
pub fn allowed_signers_path() -> Result<String, AppError> {
    Ok(signing::allowed_signers_path().to_string_lossy().to_string())
}

// --- History browser: branches, paginated commits, graph, merges ---

#[tauri::command]
pub async fn history_list_repos() -> Result<Vec<RepoRef>, AppError> {
    git_history::list_repos().await
}

#[tauri::command]
pub async fn history_branches(repo_path: String) -> Result<Vec<BranchInfo>, AppError> {
    tokio::task::spawn_blocking(move || git_history::list_branches(&repo_path))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Paginated: git log is fast, but a 100k-commit repo would still choke the UI
/// if we handed it everything at once.
#[tauri::command]
pub async fn history_page(
    repo_path: String,
    rev: String,
    offset: usize,
    limit: usize,
    search: Option<String>,
    author: Option<String>,
) -> Result<HistoryPage, AppError> {
    tokio::task::spawn_blocking(move || {
        git_history::history_page(
            &repo_path,
            &rev,
            offset,
            limit,
            search.as_deref(),
            author.as_deref(),
        )
    })
    .await
    .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[tauri::command]
pub async fn history_commit_detail(
    repo_path: String,
    hash: String,
) -> Result<CommitDetail, AppError> {
    tokio::task::spawn_blocking(move || git_history::commit_detail(&repo_path, &hash))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

#[tauri::command]
pub async fn history_branch_merges(
    repo_path: String,
    branch: String,
) -> Result<MergeInfo, AppError> {
    tokio::task::spawn_blocking(move || git_history::branch_merge_info(&repo_path, &branch))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}
