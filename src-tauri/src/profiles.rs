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
