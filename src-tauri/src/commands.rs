use crate::commit_audit::{self, RepoAudit};
use crate::credentials;
use crate::doctor::{self, Finding};
use crate::error::AppError;
use crate::gh_cli;
use crate::git_config;
use crate::git_history::{self, BranchInfo, CommitDetail, HistoryPage, FetchResult, MergeInfo, RepoRef, SyncStatus};
use crate::git_remote::{self, RemoteChange};
use crate::git_ops::{self, OpResult};
use crate::git_status::{self, FileDiff, RepoStatus};
use crate::push_guard::PushState;
use crate::push_lock::{self, HelperStatus, JobOutcome, PushModeResult};
use crate::github::{self, GitHubKey, GitHubUser, RepoPermissions};
use crate::remote_repos::{self, LocalClones, RepoAccount, RepoListing};
use crate::repo_scan::{self, ScannedRepo};
use crate::oauth::{self, DeviceCodeResponse, OAuthTokenResponse};
use crate::profiles::{self, Profile};
use crate::signing;
use crate::sparse::{self, CertInfo, CloneResult, DestinationStatus, SparseInfo, SparseSetResult};
use crate::submodules::{self, SubmoduleInfo};
use crate::lfs::{self, LfsListing, LfsProgress, LfsStatus, LfsTool};
use crate::ssh_keys;

#[tauri::command]
pub fn get_profiles() -> Result<Vec<Profile>, AppError> {
    let store = profiles::load_profiles()?;
    Ok(store.profiles)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // mirrors the profile form field-for-field
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
#[allow(clippy::too_many_arguments)] // mirrors the profile form field-for-field
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
    app: tauri::AppHandle,
    url: String,
    parent_dir: String,
    folder_name: Option<String>,
    clone_as: Option<String>,
) -> Result<CloneResult, AppError> {
    let res = sparse::sparse_clone(&url, &parent_dir, folder_name.as_deref(), clone_as.as_deref()).await?;
    if res.mapped_to.is_some() {
        crate::tray::update_active_label(&app);
    }
    Ok(res)
}

/// Full clone. `clone_as` = profile id to authenticate as; None lets the
/// destination folder's profile decide.
#[tauri::command]
pub async fn full_clone(
    app: tauri::AppHandle,
    url: String,
    parent_dir: String,
    folder_name: Option<String>,
    clone_as: Option<String>,
    with_submodules: Option<bool>,
    with_lfs: Option<bool>,
) -> Result<CloneResult, AppError> {
    let res = sparse::full_clone(
        &url,
        &parent_dir,
        folder_name.as_deref(),
        clone_as.as_deref(),
        with_submodules.unwrap_or(true),
        with_lfs.unwrap_or(true),
    )
    .await?;
    if res.mapped_to.is_some() {
        crate::tray::update_active_label(&app);
    }
    Ok(res)
}

/// Is the clone destination already taken (and by this same repo)?
#[tauri::command]
pub async fn clone_destination_status(
    url: String,
    parent_dir: String,
    folder_name: Option<String>,
) -> Result<DestinationStatus, AppError> {
    sparse::destination_status(&url, &parent_dir, folder_name.as_deref()).await
}

/// Point an existing clone's origin at another address of the same repo.
#[tauri::command]
pub async fn switch_repo_origin(repo_path: String, url: String) -> Result<String, AppError> {
    sparse::switch_origin(&repo_path, &url).await
}

/// Is there a company-signed SSH certificate (`<key>-cert.pub`) for this key?
#[tauri::command]
pub async fn ssh_certificate_info(key_path: String) -> Result<CertInfo, AppError> {
    Ok(sparse::certificate_info(&key_path).await)
}

/// Sparse checkout: inspect a repo (branch, top-level folders, current sparse set).
#[tauri::command]
pub async fn sparse_repo_info(repo_path: String) -> Result<SparseInfo, AppError> {
    sparse::repo_info(&repo_path).await
}

/// Sparse checkout: replace the folder selection and materialize the tree,
/// downloading LFS content for it when asked.
#[tauri::command]
pub async fn sparse_set(repo_path: String, dirs: Vec<String>, with_lfs: Option<bool>) -> Result<SparseSetResult, AppError> {
    sparse::sparse_set(&repo_path, dirs, with_lfs.unwrap_or(true)).await
}

/// Is git-lfs installed on this computer? Asked before a repository exists.
#[tauri::command]
pub async fn lfs_available() -> LfsTool {
    lfs::lfs_available().await
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

/// Fetch remote refs for one repo. Safe: never touches the working tree.
#[tauri::command]
pub async fn history_fetch(repo_path: String) -> Result<FetchResult, AppError> {
    tokio::task::spawn_blocking(move || git_history::fetch_repo(&repo_path))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

/// Ahead/behind vs the remote, plus how stale that answer is.
#[tauri::command]
pub async fn history_sync_status(
    repo_path: String,
    branch: String,
) -> Result<SyncStatus, AppError> {
    tokio::task::spawn_blocking(move || git_history::sync_status(&repo_path, &branch))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
}

// --- Repositories: everything the signed-in GitHub accounts can reach ---

#[tauri::command]
pub async fn repo_accounts() -> Result<Vec<RepoAccount>, AppError> {
    Ok(remote_repos::accounts().await)
}

/// One page of the listing (`per_page` ≤ 100): the page paints the first few
/// at once and keeps loading the rest in the background.
#[tauri::command]
pub async fn list_remote_repos_page(account: String, page: u32, per_page: u32) -> Result<crate::remote_repos::RepoPage, AppError> {
    crate::remote_repos::list_repos_page(&account, page, per_page).await
}

#[tauri::command]
pub async fn list_remote_repos(account: String) -> Result<RepoListing, AppError> {
    remote_repos::list_repos(&account).await
}

/// Which repositories are on this machine right now (local disk only).
#[tauri::command]
pub async fn local_clone_index() -> Result<LocalClones, AppError> {
    remote_repos::local_clones().await
}

/// Does this path still exist? Used before acting on a folder the UI listed
/// earlier — it may have been deleted or moved since.
#[tauri::command]
pub fn path_exists(path: String) -> bool {
    !path.is_empty() && std::path::Path::new(&path).exists()
}

// --- Changes page: working-tree state, diffs, and per-repo push access ---

/// Everything the Changes page needs about one repo, in one call: files,
/// branch, ahead/behind, identity, in-progress operation and push state.
#[tauri::command]
pub async fn changes_repo_status(repo_path: String) -> Result<RepoStatus, AppError> {
    git_status::repo_status(&repo_path).await
}

/// One file's diff, capped and binary-aware. `staged` picks the index-vs-HEAD
/// side; `untracked` diffs a brand-new file against nothing so it still shows.
#[tauri::command]
pub async fn changes_file_diff(
    repo_path: String,
    path: String,
    staged: bool,
    untracked: bool,
) -> Result<FileDiff, AppError> {
    git_status::file_diff(&repo_path, &path, staged, untracked).await
}

#[tauri::command]
pub async fn changes_push_state(repo_path: String) -> Result<PushState, AppError> {
    git_status::push_state_for(&repo_path).await
}

/// Switch push access: "allowed" | "guardrail" | "locked". Locking and
/// unlocking run through the privileged helper behind the operating system's
/// administrator prompt; the result says what happened — including that the
/// prompt was cancelled — and carries the re-measured state.
#[tauri::command]
pub async fn changes_set_push_mode(repo_path: String, mode: String) -> Result<PushModeResult, AppError> {
    push_lock::set_push_mode(&repo_path, &mode).await
}

/// Repair a lock whose admin-owned layers drifted (one prompt) after
/// re-applying the user-level mirrors (no prompt).
#[tauri::command]
pub async fn changes_repair_push_lock(repo_path: String) -> Result<PushModeResult, AppError> {
    push_lock::repair_lock(&repo_path).await
}

/// The installed and bundled lock helper, the registry and every lock — for
/// Settings and Doctor.
#[tauri::command]
pub async fn push_lock_helper_status() -> HelperStatus {
    push_lock::helper_status().await
}

/// Remove every lock, the registry, the remote helper and the helper itself
/// (one prompt), then the per-repo mirrors.
#[tauri::command]
pub async fn push_lock_uninstall() -> JobOutcome {
    push_lock::uninstall_all().await
}

/// Doctor's `lock-*` fixes.
#[tauri::command]
pub async fn push_lock_fix(fix: String) -> JobOutcome {
    push_lock::fix(&fix).await
}

/// After a `manual-required` outcome, read the helper's result for that job.
#[tauri::command]
pub async fn push_lock_finish_manual(nonce: String) -> JobOutcome {
    push_lock::finish_manual(&nonce).await
}

#[tauri::command]
pub async fn doctor_check_push_locks() -> Result<Vec<Finding>, AppError> {
    doctor::check_push_locks().await
}

/// Re-apply a block that has drifted — usually because a remote was added
/// after it was turned on.
#[tauri::command]
pub async fn changes_repair_push_block(repo_path: String) -> Result<PushState, AppError> {
    git_status::repair_push_block_for(&repo_path).await
}

/// Stage the named files. Never "everything" by accident: an empty list is an
/// error, and staging all is a separate command.
#[tauri::command]
pub async fn changes_stage(repo_path: String, paths: Vec<String>) -> Result<OpResult, AppError> {
    git_ops::stage(&repo_path, paths).await
}

#[tauri::command]
pub async fn changes_stage_all(repo_path: String) -> Result<OpResult, AppError> {
    git_ops::stage_all(&repo_path).await
}

#[tauri::command]
pub async fn changes_unstage(repo_path: String, paths: Vec<String>) -> Result<OpResult, AppError> {
    git_ops::unstage(&repo_path, paths).await
}

/// Destructive: tracked files go back to HEAD, untracked ones are deleted.
/// Only ever the files named here.
#[tauri::command]
pub async fn changes_discard(repo_path: String, paths: Vec<String>) -> Result<OpResult, AppError> {
    git_ops::discard(&repo_path, paths).await
}

/// Commits with the folder's own identity. Hooks always run, so the identity
/// guard keeps working.
#[tauri::command]
pub async fn changes_commit(
    repo_path: String,
    message: String,
    amend: bool,
) -> Result<OpResult, AppError> {
    git_ops::commit(&repo_path, &message, amend).await
}

#[tauri::command]
pub async fn changes_push(repo_path: String, set_upstream: bool) -> Result<OpResult, AppError> {
    git_ops::push(&repo_path, set_upstream).await
}

/// `mode` is one of "ff-only", "merge", "rebase" — chosen by the user, never
/// inferred from their git settings. `with_lfs` additionally downloads the
/// content behind any LFS pointer stub, which a plain pull does not do unless
/// the repository's LFS filters happen to be configured.
#[tauri::command]
pub async fn changes_pull(
    repo_path: String,
    mode: String,
    with_lfs: bool,
    autostash: bool,
) -> Result<OpResult, AppError> {
    let mode = git_ops::PullMode::parse(&mode)?;
    git_ops::pull(&repo_path, mode, with_lfs, autostash).await
}

/// Every gitlink in the repo with its real state — where it moved, what is
/// dirty inside it, and whether git can fetch it at all.
#[tauri::command]
pub async fn changes_submodules(repo_path: String) -> Result<Vec<SubmoduleInfo>, AppError> {
    submodules::list_submodules(&repo_path).await
}

#[tauri::command]
pub async fn changes_submodule_update(repo_path: String) -> Result<OpResult, AppError> {
    git_ops::submodule_update(&repo_path).await
}

/// Are this repo's large files really here, or still LFS pointer stubs?
#[tauri::command]
pub async fn changes_lfs_status(repo_path: String) -> Result<LfsStatus, AppError> {
    lfs::lfs_status(&repo_path).await
}

/// Every large file in the checkout, with folder totals — what the browser
/// needs to offer one file, one folder, or all of them.
#[tauri::command]
pub async fn changes_lfs_files(repo_path: String) -> Result<LfsListing, AppError> {
    lfs::lfs_files(&repo_path).await
}

/// Download only the files and folders picked in the browser.
#[tauri::command]
pub async fn changes_lfs_pull_paths(repo_path: String, paths: Vec<String>) -> Result<OpResult, AppError> {
    git_ops::lfs_pull_paths(&repo_path, paths).await
}

/// How far the download running in this repository has got. `None` when no run
/// has reported anything yet, which is the normal state between operations.
#[tauri::command]
pub async fn changes_lfs_progress(repo_path: String) -> Option<LfsProgress> {
    lfs::read_progress(&repo_path).await
}

/// `git lfs pull`: download the content for every pointer stub. Reports how
/// many actually arrived, measured by counting the stubs again afterwards.
#[tauri::command]
pub async fn changes_lfs_pull(repo_path: String) -> Result<OpResult, AppError> {
    git_ops::lfs_pull(&repo_path).await
}

/// Abort the merge or rebase that is in progress, putting the branch back.
/// Sync, step 1: fetch, measure, decide. Nothing is written. `stash` says
/// whether uncommitted changes may be stashed around the rebase.
#[tauri::command]
pub async fn changes_sync_plan(repo_path: String, stash: bool, fetch: bool) -> Result<crate::sync::SyncPlan, AppError> {
    crate::sync::sync_plan(&repo_path, stash, fetch).await
}

/// Sync, step 2: run the plan. `fingerprint` comes from the plan; the run
/// refuses if upstream moved since.
#[tauri::command]
pub async fn changes_sync_run(
    repo_path: String,
    stash: bool,
    bundles: bool,
    fingerprint: Vec<(String, String)>,
) -> Result<OpResult, AppError> {
    crate::sync::sync_run(&repo_path, crate::sync::RunOptions { stash, bundles, fingerprint }).await
}

/// Resume a paused sync after conflicts were resolved and staged.
#[tauri::command]
pub async fn changes_sync_continue(repo_path: String) -> Result<OpResult, AppError> {
    crate::sync::sync_continue(&repo_path).await
}

/// Undo a paused sync.
#[tauri::command]
pub async fn changes_sync_abort(repo_path: String) -> Result<OpResult, AppError> {
    crate::sync::sync_abort(&repo_path).await
}

/// One commit recording submodule pointers that sit ahead of the branch.
#[tauri::command]
pub async fn changes_record_pointers(repo_path: String) -> Result<OpResult, AppError> {
    crate::sync::record_pointers(&repo_path).await
}

/// Finish the operation in progress once its conflicts are staged.
#[tauri::command]
pub async fn changes_continue(repo_path: String) -> Result<OpResult, AppError> {
    git_ops::continue_op(&repo_path).await
}

#[tauri::command]
pub async fn changes_abort(repo_path: String) -> Result<OpResult, AppError> {
    git_ops::abort(&repo_path).await
}


// --- Tree management: submodule sections, branches, undo/reset, revert, stashes ---

/// The full status of every populated submodule, one section each on the
/// Changes page (stage, resolve and commit inside them).
#[tauri::command]
pub async fn changes_submodule_statuses(repo_path: String) -> Result<Vec<submodules::SubmoduleStatus>, AppError> {
    submodules::submodule_statuses(&repo_path).await
}

/// `git switch --no-guess <name>`; a remote-only name is never created implicitly.
#[tauri::command]
pub async fn changes_switch_branch(repo_path: String, name: String) -> Result<OpResult, AppError> {
    crate::tree::switch_branch(&repo_path, &name).await
}

/// `from` None = HEAD. A remote-tracking start point sets the upstream.
#[tauri::command]
pub async fn changes_create_branch(
    repo_path: String,
    name: String,
    from: Option<String>,
    switch_to: bool,
) -> Result<OpResult, AppError> {
    crate::tree::create_branch(&repo_path, &name, from.as_deref(), switch_to).await
}

/// `force` deletes a branch whose commits no other branch holds; the refusal
/// without it names the count. Remotes are never touched.
#[tauri::command]
pub async fn changes_delete_branch(repo_path: String, name: String, force: bool) -> Result<OpResult, AppError> {
    crate::tree::delete_branch(&repo_path, &name, force).await
}

#[tauri::command]
pub async fn changes_rename_branch(repo_path: String, old_name: String, new_name: String) -> Result<OpResult, AppError> {
    crate::tree::rename_branch(&repo_path, &old_name, &new_name).await
}

/// Soft-reset HEAD~1: the last commit's changes come back staged. Refused when
/// the commit is already on a remote (that would need a force-push).
#[tauri::command]
pub async fn changes_undo_commit(repo_path: String) -> Result<OpResult, AppError> {
    crate::tree::undo_commit(&repo_path).await
}

/// Reset the branch to `target` (any commit id or ref; `@{u}` for the
/// upstream). Makes a backup branch when commits would leave; refuses when
/// commits already on the upstream would be dropped.
#[tauri::command]
pub async fn changes_reset(
    repo_path: String,
    target: String,
    mode: String,
    stash_first: bool,
) -> Result<OpResult, AppError> {
    let mode = crate::tree::ResetMode::parse(&mode)?;
    crate::tree::reset_to(&repo_path, &target, mode, stash_first).await
}

/// Look at a commit without moving any branch (detached HEAD).
#[tauri::command]
pub async fn changes_detach(repo_path: String, target: String) -> Result<OpResult, AppError> {
    crate::tree::detach(&repo_path, &target).await
}

/// `mainline` is only needed for a merge commit; the refusal lists the parents.
#[tauri::command]
pub async fn changes_revert(repo_path: String, target: String, mainline: Option<u32>) -> Result<OpResult, AppError> {
    crate::tree::revert(&repo_path, &target, mainline).await
}

#[tauri::command]
pub async fn changes_cherry_pick(repo_path: String, target: String) -> Result<OpResult, AppError> {
    crate::tree::cherry_pick(&repo_path, &target).await
}

/// Resolve conflicted files by taking one side. `side` is "mine" or "theirs"
/// in the user's words; the mapping to --ours/--theirs depends on the operation.
#[tauri::command]
pub async fn changes_resolve_side(repo_path: String, paths: Vec<String>, side: String) -> Result<OpResult, AppError> {
    let side = crate::tree::Side::parse(&side)?;
    crate::tree::resolve_side(&repo_path, paths, side).await
}

/// Everything back to HEAD. Untracked files only when asked, ignored files
/// never; an optional stash first keeps it all recoverable.
#[tauri::command]
pub async fn changes_discard_all(repo_path: String, include_untracked: bool, stash_first: bool) -> Result<OpResult, AppError> {
    crate::tree::discard_all(&repo_path, include_untracked, stash_first).await
}

#[tauri::command]
pub async fn changes_stash_list(repo_path: String) -> Result<Vec<crate::stash::StashEntry>, AppError> {
    crate::stash::stash_list(&repo_path).await
}

#[tauri::command]
pub async fn changes_stash_show(repo_path: String, index: usize) -> Result<crate::stash::StashDetail, AppError> {
    crate::stash::stash_show(&repo_path, index).await
}

#[tauri::command]
pub async fn changes_stash_file_diff(repo_path: String, index: usize, path: String) -> Result<git_status::FileDiff, AppError> {
    crate::stash::stash_file_diff(&repo_path, index, &path).await
}

/// `git stash push` (never `--all`): ignored files stay where they are.
#[tauri::command]
pub async fn changes_stash_push(repo_path: String, message: Option<String>, include_untracked: bool) -> Result<OpResult, AppError> {
    crate::stash::stash_push(&repo_path, message.as_deref(), include_untracked, None).await
}

/// `oid`, when given, is the commit id the UI showed for that row: the
/// operation is refused (`no-stash`) if `stash@{index}` now holds a different
/// entry, so a list that went stale never acts on a neighbour.
#[tauri::command]
pub async fn changes_stash_apply(repo_path: String, index: usize, pop: bool, restore_index: bool, oid: Option<String>) -> Result<OpResult, AppError> {
    crate::stash::stash_apply(&repo_path, index, pop, restore_index, oid.as_deref()).await
}

/// The result carries `git stash store …` so a dropped entry can be put back.
/// `oid` as for `changes_stash_apply`.
#[tauri::command]
pub async fn changes_stash_drop(repo_path: String, index: usize, oid: Option<String>) -> Result<OpResult, AppError> {
    crate::stash::stash_drop(&repo_path, index, oid.as_deref()).await
}

/// `oid` as for `changes_stash_apply`.
#[tauri::command]
pub async fn changes_stash_restore_file(repo_path: String, index: usize, path: String, oid: Option<String>) -> Result<OpResult, AppError> {
    crate::stash::stash_restore_file(&repo_path, index, &path, oid.as_deref()).await
}

/// What one commit did to one file (first-parent diff for merges).
#[tauri::command]
pub async fn history_commit_file_diff(repo_path: String, hash: String, path: String) -> Result<git_status::FileDiff, AppError> {
    git_status::commit_file_diff(&repo_path, &hash, &path).await
}

/// What is new on the remote branch, without downloading anything (ls-remote,
/// plus GitHub's compare API for a count when an account is signed in).
#[tauri::command]
pub async fn history_peek(repo_path: String) -> Result<crate::peek::RemotePeek, AppError> {
    crate::peek::peek_remote(&repo_path).await
}

/// A commit id, short id or ref typed by the user, with what a reset to it
/// would do. `None` when nothing matches.
#[tauri::command]
pub async fn history_resolve(repo_path: String, text: String) -> Result<Option<crate::tree::CommitTarget>, AppError> {
    crate::tree::resolve_commit(&repo_path, &text).await
}
