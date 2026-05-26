use crate::credentials;
use crate::error::AppError;
use crate::git_config;
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
pub fn test_connection() -> Result<String, AppError> {
    ssh_keys::test_ssh_connection()
}

#[tauri::command]
pub fn test_connection_with_key(key_path: String) -> Result<String, AppError> {
    ssh_keys::test_ssh_connection_with_key(&key_path)
}

#[tauri::command]
pub fn list_ssh_keys() -> Result<Vec<String>, AppError> {
    ssh_keys::list_ssh_keys()
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
