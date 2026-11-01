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
    /// Remotes this profile claims, as `owner/` (every repository of that
    /// GitHub owner) or `owner/repo`, or a full URL glob for other hosts. A
    /// clone whose remote matches commits as this profile wherever its folder
    /// is — through `includeIf "hasconfig:remote.*.url:…"` (git ≥ 2.36).
    #[serde(default)]
    pub remote_patterns: Vec<String>,
    /// The GitHub account this profile pushes as, when known (picked in the
    /// form's "Autofill from GitHub", or carried over from the sign-in page).
    /// The Changes page compares it with the account a push would really use.
    #[serde(default)]
    pub github_login: Option<String>,
    /// HTTPS clones in this profile's folders push over SSH with its key —
    /// git's pushInsteadOf: fetch stays on HTTPS — so a push goes as the key's
    /// account, not whichever one the credential helper holds. Off unless
    /// turned on (`None` in older stores): an organisation that requires SSH
    /// certificates refuses a plain key, and a push that works over HTTPS
    /// today must not start failing after an update.
    #[serde(default)]
    pub push_over_ssh: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One remote pattern as the person typed it, normalised: `acme` → `acme/`.
/// Accepted: `owner/`, `owner/repo`, or a full glob beginning with a scheme
/// or `git@` (for hosts other than github.com).
pub fn normalize_remote_pattern(raw: &str) -> Result<String, AppError> {
    let p = raw.trim();
    let bad = |m: &str| Err(AppError::Config(format!("Remote rule `{}`: {}", p, m)));
    if p.is_empty() {
        return Err(AppError::Config("A remote rule can't be empty.".into()));
    }
    if p.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\\') {
        return bad("it can't contain spaces, quotes or backslashes");
    }
    if p.starts_with("https://") || p.starts_with("http://") || p.starts_with("ssh://") || p.starts_with("git@") {
        return Ok(p.to_string());
    }
    let ok_part = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    match p.split_once('/') {
        None if ok_part(p) => Ok(format!("{}/", p)),
        Some((owner, "")) if ok_part(owner) => Ok(format!("{}/", owner)),
        Some((owner, repo)) if ok_part(owner) && ok_part(repo) && !repo.contains('/') => Ok(format!("{}/{}", owner, repo.trim_end_matches(".git"))),
        _ => bad("write it as `owner/` (every repository of that owner), `owner/repo`, or a full URL such as https://gitlab.example.com/team/**"),
    }
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

/// The folders as they are stored: `~` expanded, each spelled as the disk
/// spells it (git matches folder rules letter for letter), each once.
fn stored_folders(dirs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        let d = crate::paths::disk_spelling(&expand_tilde(d));
        if !out.iter().any(|o| norm_dir(o) == norm_dir(&d)) {
            out.push(d);
        }
    }
    out
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

/// What every field must satisfy before it is written into a git config.
/// Values are quoted on the way out (paths.rs), so this is not about git's
/// syntax; it is about what makes no sense in a commit: a line break in a
/// name, an email with a space or an angle bracket (git writes it between
/// `<` and `>` in every commit), a folder git could never match.
fn validate_fields(name: &str, git_name: &str, git_email: &str, ssh_key_path: Option<&str>, directories: &[String]) -> Result<(), AppError> {
    let control = |s: &str| s.chars().any(char::is_control);
    let bad = |m: &str| Err(AppError::Config(m.to_string()));
    if name.trim().is_empty() {
        return bad("A profile needs a name.");
    }
    if control(name) {
        return bad("The profile name can't contain line breaks or control characters.");
    }
    if git_name.trim().is_empty() {
        return bad("A git name is needed: it goes on every commit made in this profile's folders.");
    }
    if control(git_name) {
        return bad("The git name can't contain line breaks or control characters.");
    }
    let email = git_email.trim();
    if email.is_empty() {
        return bad("An email is needed: it is how GitHub attributes commits to an account.");
    }
    if control(email) || email.chars().any(char::is_whitespace) || email.contains(['<', '>']) {
        return bad("The email can't contain spaces, `<` or `>`: git writes it between angle brackets in every commit.");
    }
    if let Some(k) = ssh_key_path {
        if control(k) {
            return bad("The SSH key path can't contain line breaks or control characters.");
        }
    }
    for d in directories {
        if control(d) {
            return bad("A folder path can't contain line breaks or control characters.");
        }
        if d.contains(['*', '?', '[']) {
            return Err(AppError::Config(format!(
                "The folder `{}` contains `*`, `?` or `[`, which git reads as a wildcard in a folder rule, so the rule could never match it exactly. Rename the folder or choose its parent.",
                d
            )));
        }
        if !std::path::Path::new(d).is_absolute() {
            let example = if cfg!(windows) { r"C:\Users\you\work" } else { "/Users/you/work" };
            return Err(AppError::Config(format!("The folder `{}` needs a full path, like {}.", d, example)));
        }
    }
    Ok(())
}

/// The profile whose include file git read a value from —
/// `…/gitconfigs/<profile id>.gitconfig` in `git config --show-origin`. This is
/// how a clone matched only by a remote rule is tied back to its profile.
pub fn profile_for_include_origin<'a>(origin: &str, profs: &'a [Profile]) -> Option<&'a Profile> {
    let file = crate::paths::base_name(origin);
    let id = file.strip_suffix(".gitconfig")?;
    profs.iter().find(|p| p.id == id)
}

/// Every pattern normalised, duplicates dropped, order kept.
fn normalize_remote_patterns(raw: &[String]) -> Result<Vec<String>, AppError> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        if r.trim().is_empty() {
            continue;
        }
        let n = normalize_remote_pattern(r)?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

/// Blocking pushes on the DEFAULT profile would put a pushInsteadOf rewrite in
/// the main ~/.gitconfig, i.e. in every folder on the machine. git would let a
/// folder profile undo it — longest matching rewrite wins, in any scope — but
/// only with a longer rewrite for every URL form, which is fragile and
/// surprising. Hence: only non-default profiles may block pushes.
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
/// otherwise let a stale mapping silently override the intended one). A folder
/// is the same folder however it is named: in another letter case, or through
/// a symbolic link.
fn dedupe_directories(store: &mut ProfileStore, keep_id: &str, dirs: &[String]) {
    let claimed: Vec<crate::paths::FolderId> = dirs.iter().map(|d| crate::paths::FolderId::of(d)).collect();
    for p in store.profiles.iter_mut() {
        if p.id == keep_id {
            continue;
        }
        p.directories.retain(|d| {
            let id = crate::paths::FolderId::of(d);
            !claimed.iter().any(|c| c.same(&id))
        });
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

/// One writer of the store at a time. Every change reads the store, changes
/// it and writes it back; two at once would each write back what the other
/// never saw — a clone finishing while Find repositories adds folders, say,
/// would put the profile back as it was when the clone began.
fn store_writer() -> std::sync::MutexGuard<'static, ()> {
    static WRITER: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

/// Written whole or not at all: to a file beside it, then renamed over it,
/// so a crash or a full disk mid-write never leaves half a store. Where the
/// rename is refused (a file another program holds open on Windows), the
/// store is written in place as before.
pub fn save_profiles(store: &ProfileStore) -> Result<(), AppError> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = get_store_path()?;
    let content = serde_json::to_string_pretty(store)?;
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp = path.with_file_name(format!("profiles.json.{}-{}.tmp", std::process::id(), n));
    fs::write(&tmp, &content)?;
    if fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
        fs::write(&path, content)?;
    }
    Ok(())
}

/// Add folders to one profile: the store read afresh under the writer lock,
/// so whatever another change added meanwhile stays. Each folder is stored as
/// the disk spells it (`~` expanded), taken from any other profile that had
/// it (a folder belongs to one profile), and returned as stored — one entry
/// per folder given, in order.
pub fn add_folders(id: &str, folders: &[String]) -> Result<Vec<String>, AppError> {
    let _writer = store_writer();
    let mut store = load_profiles()?;
    let stored: Vec<String> = folders.iter().map(|f| crate::paths::disk_spelling(&expand_tilde(f))).collect();
    let profile = store
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", id)))?;
    let mut dirs = profile.directories.clone();
    dirs.extend(stored.iter().cloned());
    let dirs = stored_folders(&dirs);
    validate_fields(&profile.name, &profile.git_name, &profile.git_email, profile.ssh_key_path.as_deref(), &dirs)?;
    profile.directories = dirs;
    profile.updated_at = Utc::now();
    dedupe_directories(&mut store, id, &stored);
    save_profiles(&store)?;
    Ok(stored)
}

/// Take folders off one profile, the store read afresh under the writer lock.
/// A folder matches however it is spelled or linked. The folders removed, as
/// they were stored.
pub fn remove_folders(id: &str, folders: &[String]) -> Result<Vec<String>, AppError> {
    let _writer = store_writer();
    let mut store = load_profiles()?;
    let profile = store
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", id)))?;
    let gone: Vec<crate::paths::FolderId> = folders.iter().map(|f| crate::paths::FolderId::of(&expand_tilde(f))).collect();
    let (removed, kept): (Vec<String>, Vec<String>) =
        profile.directories.iter().cloned().partition(|d| gone.iter().any(|g| g.same(&crate::paths::FolderId::of(d))));
    if !removed.is_empty() {
        profile.directories = kept;
        profile.updated_at = Utc::now();
        save_profiles(&store)?;
    }
    Ok(removed)
}

#[allow(clippy::too_many_arguments)] // one value per field of the form
pub fn create_profile(
    name: String,
    git_name: String,
    git_email: String,
    ssh_key_path: Option<String>,
    directories: Vec<String>,
    allow_push: bool,
    signing_enabled: bool,
    remote_patterns: Vec<String>,
    github_login: Option<String>,
    push_over_ssh: Option<bool>,
) -> Result<Profile, AppError> {
    let _writer = store_writer();
    let mut store = load_profiles()?;

    let is_default = store.profiles.is_empty();
    ensure_default_can_push(is_default, allow_push)?;
    let now = Utc::now();

    // Normalize "" to None so an empty picker never stores an empty path, and
    // expand `~` here as for folders, so Doctor can check the file exists.
    let ssh_key_path = ssh_key_path.filter(|k| !k.is_empty()).map(|k| expand_tilde(&k));
    let directories = stored_folders(&directories);
    let (name, git_name, git_email) = (name.trim().to_string(), git_name.trim().to_string(), git_email.trim().to_string());
    validate_fields(&name, &git_name, &git_email, ssh_key_path.as_deref(), &directories)?;
    let remote_patterns = normalize_remote_patterns(&remote_patterns)?;

    let profile = Profile {
        id: Uuid::new_v4().to_string(),
        name,
        git_name,
        git_email,
        ssh_key_path,
        is_default,
        allow_push,
        signing_enabled,
        directories,
        remote_patterns,
        github_login: github_login.map(|l| l.trim().trim_start_matches('@').to_string()).filter(|l| !l.is_empty()),
        created_at: now,
        updated_at: now,
        push_over_ssh,
    };

    // A folder belongs to exactly one profile — strip it from any others.
    dedupe_directories(&mut store, &profile.id, &profile.directories);

    store.profiles.push(profile.clone());
    save_profiles(&store)?;
    Ok(profile)
}

#[allow(clippy::too_many_arguments)] // one optional value per editable field
pub fn update_profile(
    id: String,
    name: Option<String>,
    git_name: Option<String>,
    git_email: Option<String>,
    ssh_key_path: Option<String>,
    directories: Option<Vec<String>>,
    allow_push: Option<bool>,
    signing_enabled: Option<bool>,
    remote_patterns: Option<Vec<String>>,
    github_login: Option<String>,
    push_over_ssh: Option<bool>,
) -> Result<Profile, AppError> {
    let _writer = store_writer();
    let mut store = load_profiles()?;

    let profile = store
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or(AppError::NotFound(format!("Profile {} not found", id)))?;

    if push_over_ssh.is_some() {
        profile.push_over_ssh = push_over_ssh;
    }
    if let Some(n) = name {
        profile.name = n.trim().to_string();
    }
    if let Some(n) = git_name {
        profile.git_name = n.trim().to_string();
    }
    if let Some(e) = git_email {
        profile.git_email = e.trim().to_string();
    }
    if let Some(k) = ssh_key_path {
        // "" means the user chose "No SSH key" — clear it. (None = not provided.)
        profile.ssh_key_path = if k.is_empty() { None } else { Some(expand_tilde(&k)) };
    }
    if let Some(d) = directories {
        profile.directories = stored_folders(&d);
    }
    if let Some(ap) = allow_push {
        ensure_default_can_push(profile.is_default, ap)?;
        profile.allow_push = ap;
    }
    if let Some(se) = signing_enabled {
        profile.signing_enabled = se;
    }
    if let Some(r) = remote_patterns {
        profile.remote_patterns = normalize_remote_patterns(&r)?;
    }
    if let Some(l) = github_login {
        // "" clears it, as for the SSH key.
        let l = l.trim().trim_start_matches('@').to_string();
        profile.github_login = if l.is_empty() { None } else { Some(l) };
    }
    // Checked on the whole profile, so an edit of one field cannot slip past
    // because the others were fine before.
    validate_fields(&profile.name, &profile.git_name, &profile.git_email, profile.ssh_key_path.as_deref(), &profile.directories)?;
    profile.updated_at = Utc::now();

    let updated = profile.clone();

    // A folder belongs to exactly one profile — strip these dirs from any others.
    dedupe_directories(&mut store, &updated.id, &updated.directories);

    save_profiles(&store)?;
    Ok(updated)
}

pub fn delete_profile(id: String) -> Result<(), AppError> {
    let _writer = store_writer();
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
    let _writer = store_writer();
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

    #[test]
    fn fields_that_would_corrupt_a_commit_or_a_config_are_refused_with_a_reason() {
        // A full path is platform-shaped: `/w` is relative on Windows.
        let dir = if cfg!(windows) { r"C:\w" } else { "/w" };
        let ok = validate_fields("Work", "Ann Example", "ann@example.test", Some("/k/id"), &[dir.into()]);
        assert!(ok.is_ok());
        // Odd but legitimate: quotes, a hash, a semicolon. Quoting handles these.
        assert!(validate_fields("W", r#"Ann #1 "The Hammer" O'Brien"#, "ann;x@example.test", None, &[]).is_ok());

        let err = |r: Result<(), AppError>| r.unwrap_err().to_string();
        assert!(err(validate_fields("", "n", "e@x", None, &[])).contains("needs a name"));
        assert!(err(validate_fields("W", "Ann\nExample", "e@x", None, &[])).contains("line breaks"));
        assert!(err(validate_fields("W", "  ", "e@x", None, &[])).contains("git name is needed"));
        assert!(err(validate_fields("W", "n", "", None, &[])).contains("email is needed"));
        assert!(err(validate_fields("W", "n", "ann example@x", None, &[])).contains("spaces"));
        assert!(err(validate_fields("W", "n", "<ann@x>", None, &[])).contains("angle brackets"));
        assert!(err(validate_fields("W", "n", "e@x", Some("/k\n"), &[])).contains("SSH key path"));
        assert!(err(validate_fields("W", "n", "e@x", None, &["/w/[archive]".into()])).contains("wildcard"));
        assert!(err(validate_fields("W", "n", "e@x", None, &["relative/dir".into()])).contains("full path"));
    }

    #[test]
    fn remote_rules_are_normalised_or_refused_with_a_reason() {
        assert_eq!(normalize_remote_pattern("acme").unwrap(), "acme/");
        assert_eq!(normalize_remote_pattern("acme/").unwrap(), "acme/");
        assert_eq!(normalize_remote_pattern(" Acme/Thing.git ").unwrap(), "Acme/Thing");
        assert_eq!(normalize_remote_pattern("https://gitlab.example.com/team/**").unwrap(), "https://gitlab.example.com/team/**");
        assert_eq!(normalize_remote_pattern("git@gitlab.example.com:team/**").unwrap(), "git@gitlab.example.com:team/**");
        for bad in ["", "acme/thing/more", "ac me/", "acme/\"x", "acme\\x"] {
            assert!(normalize_remote_pattern(bad).is_err(), "{bad:?} should be refused");
        }
        let list = normalize_remote_patterns(&["acme".into(), "acme/".into(), "".into(), "acme/thing".into()]).unwrap();
        assert_eq!(list, vec!["acme/", "acme/thing"]);
    }

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
            remote_patterns: Vec::new(),
            github_login: None,
            directories: dirs.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            push_over_ssh: None,
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
    fn a_folder_named_another_way_moves_to_the_profile_that_claims_it() {
        let root = std::env::temp_dir().join(format!("gitswitch-moves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Work")).expect("dirs");
        let root = std::fs::canonicalize(&root).expect("canonical");
        let work = root.join("Work").to_string_lossy().to_string();
        let shouted = root.join("WORK").to_string_lossy().to_string();
        if std::path::Path::new(&shouted).exists() {
            // A disk that ignores letter case: one folder, stored as the disk spells it, once.
            assert_eq!(stored_folders(&[shouted.clone(), work.clone()]), vec![work.clone()]);
            let mut store = ProfileStore { profiles: vec![mk("A", &[&work]), mk("B", &[])] };
            dedupe_directories(&mut store, "B", std::slice::from_ref(&shouted));
            assert!(store.profiles[0].directories.is_empty(), "{:?}", store.profiles[0].directories);
        }
        #[cfg(unix)]
        {
            let link = root.join("link").to_string_lossy().to_string();
            std::os::unix::fs::symlink(root.join("Work"), &link).expect("symlink");
            // Named through a link: the folder moves, it is never shared.
            let mut store = ProfileStore { profiles: vec![mk("A", &[&link]), mk("B", &[])] };
            dedupe_directories(&mut store, "B", std::slice::from_ref(&work));
            assert!(store.profiles[0].directories.is_empty());
            let mut store = ProfileStore { profiles: vec![mk("A", &[&work]), mk("B", &[])] };
            dedupe_directories(&mut store, "B", std::slice::from_ref(&link));
            assert!(store.profiles[0].directories.is_empty());
            // The link keeps its name in the store.
            assert_eq!(stored_folders(std::slice::from_ref(&link)), vec![link]);
        }
        // A neighbour is a different folder.
        std::fs::create_dir_all(root.join("Workshop")).expect("neighbour");
        let mut store = ProfileStore { profiles: vec![mk("A", &[&root.join("Workshop").to_string_lossy()]), mk("B", &[])] };
        dedupe_directories(&mut store, "B", &[work]);
        assert_eq!(store.profiles[0].directories.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
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
