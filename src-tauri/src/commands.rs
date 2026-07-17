use crate::credentials;
use crate::error::AppError;
use crate::gh_cli;
use crate::git_config;
use crate::git_remote::{self, RemoteChange};
use crate::github::{self, GitHubKey, GitHubUser};
use crate::oauth::{self, DeviceCodeResponse, OAuthTokenResponse};
use crate::profiles::{self, Profile};
use crate::ssh_keys;

#[tauri::command]
pub fn get_profiles() -> Result<Vec<Profile>, AppError> {
    let store = profiles::load_profiles()?;
    Ok(store.profiles)
}

#[tauri::command]
pub fn create_profile(
    name: String,
    git_name: String,
    git_email: String,
    ssh_key_path: Option<String>,
    directories: Vec<String>,
) -> Result<Profile, AppError> {
    let profile = profiles::create_profile(name, git_name, git_email, ssh_key_path, directories)?;
    git_config::apply_git_config()?;
    Ok(profile)
}

#[tauri::command]
pub fn update_profile(
    id: String,
    name: Option<String>,
    git_name: Option<String>,
    git_email: Option<String>,
    ssh_key_path: Option<String>,
    directories: Option<Vec<String>>,
) -> Result<Profile, AppError> {
    let profile = profiles::update_profile(id, name, git_name, git_email, ssh_key_path, directories)?;
    git_config::apply_git_config()?;
    Ok(profile)
}

#[tauri::command]
pub fn delete_profile(id: String) -> Result<(), AppError> {
    credentials::delete_token(&id).ok();
    profiles::delete_profile(id)?;
    git_config::apply_git_config()?;
    Ok(())
}

#[tauri::command]
pub fn set_default_profile(id: String) -> Result<Profile, AppError> {
    let profile = profiles::set_default_profile(id)?;
    git_config::apply_git_config()?;
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

/// Automate "Step 4": convert HTTPS (incl. token) remotes to SSH for all repos under a folder.
#[tauri::command]
pub fn convert_repos_to_ssh(directory: String) -> Result<Vec<RemoteChange>, AppError> {
    git_remote::convert_repos_in_dir(&directory)
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
