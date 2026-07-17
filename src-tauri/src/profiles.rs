use crate::error::AppError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub git_name: String,
    pub git_email: String,
    pub ssh_key_path: Option<String>,
    pub github_token: Option<String>,
    pub is_default: bool,
    pub directories: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<Profile>,
}

impl ProfileStore {
    pub fn new() -> Self {
        Self {
            profiles: Vec::new(),
        }
    }
}

/// Normalize a directory path for comparison (trim trailing slashes).
fn norm_dir(d: &str) -> String {
    d.trim_end_matches('/').to_string()
}

/// Remove `dirs` from every profile EXCEPT `keep_id`, so a folder can belong to
/// only one profile at a time (git's includeIf is "last match wins", which would
/// otherwise let a stale mapping silently override the intended one).
fn dedupe_directories(store: &mut ProfileStore, keep_id: &str, dirs: &[String]) {
    let claimed: Vec<String> = dirs.iter().map(|d| norm_dir(d)).collect();
    for p in store.profiles.iter_mut() {
        if p.id == keep_id {
            continue;
        }
        p.directories.retain(|d| !claimed.contains(&norm_dir(d)));
    }
}

fn get_store_path() -> Result<PathBuf, AppError> {
    let data_dir = dirs::data_dir().ok_or(AppError::Config("Cannot find app data directory".into()))?;
    let app_dir = data_dir.join("com.gitswitch.app");
    fs::create_dir_all(&app_dir)?;
    Ok(app_dir.join("profiles.json"))
}

pub fn load_profiles() -> Result<ProfileStore, AppError> {
    let path = get_store_path()?;
    if !path.exists() {
        let store = ProfileStore::new();
        save_profiles(&store)?;
        return Ok(store);
    }
    let content = fs::read_to_string(&path)?;
    let store: ProfileStore = serde_json::from_str(&content)?;
    Ok(store)
}

pub fn save_profiles(store: &ProfileStore) -> Result<(), AppError> {
    let path = get_store_path()?;
    let content = serde_json::to_string_pretty(store)?;
    fs::write(&path, content)?;
    Ok(())
}

pub fn create_profile(
    name: String,
    git_name: String,
    git_email: String,
    ssh_key_path: Option<String>,
    directories: Vec<String>,
) -> Result<Profile, AppError> {
    let mut store = load_profiles()?;

    let is_default = store.profiles.is_empty();
    let now = Utc::now();

    let profile = Profile {
        id: Uuid::new_v4().to_string(),
        name,
        git_name,
        git_email,
        ssh_key_path,
        github_token: None,
        is_default,
        directories,
        created_at: now,
        updated_at: now,
    };

    // A folder belongs to exactly one profile — strip it from any others.
    dedupe_directories(&mut store, &profile.id, &profile.directories);

    store.profiles.push(profile.clone());
    save_profiles(&store)?;
    Ok(profile)
}

pub fn update_profile(
    id: String,
    name: Option<String>,
    git_name: Option<String>,
    git_email: Option<String>,
    ssh_key_path: Option<String>,
    directories: Option<Vec<String>>,
) -> Result<Profile, AppError> {
    let mut store = load_profiles()?;

    let profile = store
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or(AppError::NotFound(format!("Profile {} not found", id)))?;

    if let Some(n) = name {
        profile.name = n;
    }
    if let Some(n) = git_name {
        profile.git_name = n;
    }
    if let Some(e) = git_email {
        profile.git_email = e;
    }
    if let Some(k) = ssh_key_path {
        profile.ssh_key_path = Some(k);
    }
    if let Some(d) = directories {
        profile.directories = d;
    }
    profile.updated_at = Utc::now();

    let updated = profile.clone();

    // A folder belongs to exactly one profile — strip these dirs from any others.
    dedupe_directories(&mut store, &updated.id, &updated.directories);

    save_profiles(&store)?;
    Ok(updated)
}

pub fn delete_profile(id: String) -> Result<(), AppError> {
    let mut store = load_profiles()?;
    let initial_len = store.profiles.len();
    store.profiles.retain(|p| p.id != id);

    if store.profiles.len() == initial_len {
        return Err(AppError::NotFound(format!("Profile {} not found", id)));
    }

    // If we deleted the default, make the first remaining one default
    if !store.profiles.is_empty() && !store.profiles.iter().any(|p| p.is_default) {
        store.profiles[0].is_default = true;
    }

    save_profiles(&store)?;
    Ok(())
}

pub fn set_default_profile(id: String) -> Result<Profile, AppError> {
    let mut store = load_profiles()?;

    let exists = store.profiles.iter().any(|p| p.id == id);
    if !exists {
        return Err(AppError::NotFound(format!("Profile {} not found", id)));
    }

    for p in store.profiles.iter_mut() {
        p.is_default = p.id == id;
    }

    let profile = store.profiles.iter().find(|p| p.id == id).unwrap().clone();
    save_profiles(&store)?;
    Ok(profile)
}
