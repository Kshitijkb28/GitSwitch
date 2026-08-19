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
    // NOTE: GitHub tokens are stored in the OS keyring (see credentials.rs),
    // never in this plaintext JSON store.
    pub is_default: bool,
    /// When false, `git push` is blocked locally for repos in this profile's
    /// folders (via a pushInsteadOf URL rewrite). Fetch/pull are unaffected.
    #[serde(default = "default_true")]
    pub allow_push: bool,
    /// Sign commits/tags in this profile's folders with its SSH key, so GitHub
    /// shows them as Verified. Off by default (older stores lack the field).
    #[serde(default)]
    pub signing_enabled: bool,
    pub directories: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
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

/// Normalize a directory path for comparison (separator-agnostic, so folder
/// dedup works on Windows too).
fn norm_dir(d: &str) -> String {
    crate::paths::norm(d)
}

/// Expand a leading `~/` to the home directory so stored paths always compare
/// equal to absolute paths (scanners and pickers produce absolute paths).
fn expand_tilde(dir: &str) -> String {
    if let Some(rest) = dir.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    dir.to_string()
}

/// Blocking pushes on the DEFAULT profile would inject an un-overridable
/// pushInsteadOf rewrite into the main ~/.gitconfig — git accumulates URL
/// rewrite rules across includes, so folder profiles that ALLOW pushes could
/// never undo it. Hence: only non-default profiles may block pushes.
fn ensure_default_can_push(is_default: bool, allow_push: bool) -> Result<(), AppError> {
    if is_default && !allow_push {
        return Err(AppError::Config(
            "The default profile can't block pushes — it would affect every folder. \
             Use a non-default profile with specific folders instead."
                .into(),
        ));
    }
    Ok(())
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
    allow_push: bool,
    signing_enabled: bool,
) -> Result<Profile, AppError> {
    let mut store = load_profiles()?;

    let is_default = store.profiles.is_empty();
    ensure_default_can_push(is_default, allow_push)?;
    let now = Utc::now();

    let profile = Profile {
        id: Uuid::new_v4().to_string(),
        name,
        git_name,
        git_email,
        // Normalize "" to None so an empty picker never stores an empty path.
        ssh_key_path: ssh_key_path.filter(|k| !k.is_empty()),
        is_default,
        allow_push,
        signing_enabled,
        directories: directories.iter().map(|d| expand_tilde(d)).collect(),
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
    allow_push: Option<bool>,
    signing_enabled: Option<bool>,
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
        // "" means the user chose "No SSH key" — clear it. (None = not provided.)
        profile.ssh_key_path = if k.is_empty() { None } else { Some(k) };
    }
    if let Some(d) = directories {
        profile.directories = d.iter().map(|dir| expand_tilde(dir)).collect();
    }
    if let Some(ap) = allow_push {
        ensure_default_can_push(profile.is_default, ap)?;
        profile.allow_push = ap;
    }
    if let Some(se) = signing_enabled {
        profile.signing_enabled = se;
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

    let target = store
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", id)))?;
    if !target.allow_push {
        return Err(AppError::Config(
            "This profile blocks pushes, so it can't become the default \
             (the block would leak into every folder). Enable push access first."
                .into(),
        ));
    }

    for p in store.profiles.iter_mut() {
        p.is_default = p.id == id;
    }

    let profile = store.profiles.iter().find(|p| p.id == id).unwrap().clone();
    save_profiles(&store)?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, dirs: &[&str]) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            git_name: "n".into(),
            git_email: "e".into(),
            ssh_key_path: None,
            is_default: false,
            allow_push: true,
            signing_enabled: false,
            directories: dirs.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn folder_belongs_to_exactly_one_profile() {
        let mut store = ProfileStore {
            profiles: vec![mk("A", &["/x/carpool", "/x/other"]), mk("B", &["/x/foo"])],
        };
        // Assign /x/carpool (trailing-slash variant) to B — must vanish from A.
        dedupe_directories(&mut store, "B", &["/x/carpool/".to_string()]);

        let a = store.profiles.iter().find(|p| p.id == "A").unwrap();
        assert_eq!(a.directories, vec!["/x/other".to_string()]);
        let b = store.profiles.iter().find(|p| p.id == "B").unwrap();
        assert_eq!(b.directories, vec!["/x/foo".to_string()]);
    }

    #[test]
    fn norm_dir_strips_trailing_slashes() {
        assert_eq!(norm_dir("/a/b/"), "/a/b");
        assert_eq!(norm_dir("/a/b"), "/a/b");
        assert_eq!(norm_dir("/a/b//"), "/a/b");
    }

    #[test]
    fn old_store_json_with_github_token_still_loads() {
        // Profiles saved by older versions carry a github_token field; serde
        // must ignore it rather than fail deserialization.
        let json = r#"{"profiles":[{"id":"1","name":"p","git_name":"n","git_email":"e",
            "ssh_key_path":null,"github_token":null,"is_default":true,"directories":[],
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}]}"#;
        let store: ProfileStore = serde_json::from_str(json).unwrap();
        assert_eq!(store.profiles.len(), 1);
        // Stores that predate the allow_push field must default to allowing pushes.
        assert!(store.profiles[0].allow_push);
    }

    #[test]
    fn tilde_paths_expand_to_home() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(
            crate::paths::norm(&expand_tilde("~/projects/work")),
            crate::paths::norm(&format!("{}/projects/work", home))
        );
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("relative"), "relative");
    }

    #[test]
    fn default_profile_may_never_block_pushes() {
        assert!(ensure_default_can_push(true, false).is_err());
        assert!(ensure_default_can_push(true, true).is_ok());
        assert!(ensure_default_can_push(false, false).is_ok());
        assert!(ensure_default_can_push(false, true).is_ok());
    }
}
