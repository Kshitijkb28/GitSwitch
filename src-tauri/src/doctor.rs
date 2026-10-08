use crate::error::AppError;
use crate::gh_cli;
use crate::profiles;
use crate::repo_scan;
use crate::ssh_keys;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Clone)]
pub struct Finding {
    /// Stable identifier for the check that produced this.
    pub id: String,
    /// "error" | "warning" | "info" | "ok"
    pub severity: String,
    pub title: String,
    pub detail: String,
    /// Action id the frontend can run to fix this, if any.
    pub fix: Option<String>,
    pub fix_label: Option<String>,
}

impl Finding {
    fn new(id: &str, severity: &str, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            severity: severity.into(),
            title: title.into(),
            detail: detail.into(),
            fix: None,
            fix_label: None,
        }
    }
    fn with_fix(mut self, action: impl Into<String>, label: impl Into<String>) -> Self {
        self.fix = Some(action.into());
        self.fix_label = Some(label.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Pure parsing helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Minimal ssh_config-style glob: `*` matches any run, `?` any single char.
fn pattern_matches(pattern: &str, host: &str) -> bool {
    fn helper(p: &[u8], h: &[u8]) -> bool {
        match p.first() {
            None => h.is_empty(),
            Some(b'*') => (0..=h.len()).any(|i| helper(&p[1..], &h[i..])),
            Some(b'?') => !h.is_empty() && helper(&p[1..], &h[1..]),
            Some(c) => !h.is_empty() && h[0] == *c && helper(&p[1..], &h[1..]),
        }
    }
    helper(pattern.as_bytes(), host.as_bytes())
}

/// Host patterns in an ssh config whose block pins an IdentityFile AND applies
/// to github.com. Such a block overrides the per-folder key GitSwitch sets via
/// `core.sshCommand`, which silently forces every repo onto one account.
pub fn ssh_config_github_overrides(config: &str) -> Vec<String> {
    let mut offenders = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut has_identity = false;

    let finish = |patterns: &Vec<String>, has_identity: bool, out: &mut Vec<String>| {
        if has_identity
            && patterns
                .iter()
                .any(|p| pattern_matches(p, "github.com"))
        {
            out.push(patterns.join(" "));
        }
    };

    for raw in config.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("host ") {
            finish(&current, has_identity, &mut offenders);
            current = rest.split_whitespace().map(|s| s.to_string()).collect();
            has_identity = false;
        } else if lower.starts_with("identityfile ") {
            has_identity = true;
        }
    }
    finish(&current, has_identity, &mut offenders);
    offenders
}

/// `includeIf` directives that live OUTSIDE the GitSwitch-managed block. These
/// are hand-written rules that can overlap (and silently fight) ours.
pub fn manual_includeif_rules(gitconfig: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut managed = false;
    for raw in gitconfig.lines() {
        let line = raw.trim();
        if line.contains(">>> GitSwitch managed") {
            managed = true;
            continue;
        }
        if line.contains("<<< GitSwitch managed") {
            managed = false;
            continue;
        }
        if !managed && line.starts_with("[includeIf") {
            rules.push(line.to_string());
        }
    }
    rules
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

async fn command_exists(bin: &str) -> bool {
    crate::spawn::async_command(bin)
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Environment-level checks: tooling, ssh config, gitconfig hygiene.
pub async fn check_environment() -> Result<Vec<Finding>, AppError> {
    let mut out = Vec::new();

    if !command_exists("git").await {
        out.push(Finding::new(
            "git-missing",
            "error",
            "git is not installed",
            "GitSwitch drives your system's git. Install the Xcode command line tools (`xcode-select --install`) or git for your platform.",
        ));
    }
    // Remote rules are `includeIf "hasconfig:…"`, which git before 2.36 does
    // not know — and silently ignores, so nothing else would ever say why a
    // clone outside every folder still commits as the default.
    let store = profiles::load_profiles().unwrap_or_else(|_| profiles::ProfileStore::new());
    if store.profiles.iter().any(|p| !p.remote_patterns.is_empty()) {
        if let Some(v) = git_version().await {
            if !gitswitch_lock_core::version_at_least(&v, "2.36.0") {
                out.push(Finding::new(
                    "git-too-old-for-remote-rules",
                    "warning",
                    format!("git {} is too old for remote rules", v),
                    "A profile claims remotes (`owner/` rules). Those are applied through `includeIf \"hasconfig:remote.*.url\"`, which exists from git 2.36 (April 2022); this git ignores them, so such clones commit as the default profile. Update git.",
                ));
            }
        }
    }
    if !command_exists("gh").await {
        out.push(Finding::new(
            "gh-missing",
            "info",
            "GitHub CLI (gh) not found",
            "Optional, but it powers one-click key registration, account sign-in, autofill and Auto Assign. Install from https://cli.github.com",
        ));
    } else {
        match gh_cli::gh_auth_status().await {
            Ok(accounts) if !accounts.is_empty() => {
                // What each github.com token lacks for what GitSwitch asks of
                // it. A token gh lists no scopes for (fine-grained, an app's)
                // has none to lack, so it is not reported.
                for a in accounts.iter().filter(|a| a.host == "github.com") {
                    let Some(scopes) = &a.scopes else { continue };
                    let needs_signing = store
                        .profiles
                        .iter()
                        .any(|p| p.signing_enabled && p.github_login.as_deref().is_none_or(|l| l.trim_start_matches('@').eq_ignore_ascii_case(&a.login)));
                    let missing = gh_cli::missing_scopes(scopes, needs_signing);
                    if missing.is_empty() {
                        continue;
                    }
                    let names: Vec<&str> = missing.iter().map(|(n, _)| *n).collect();
                    let why: Vec<String> = missing.iter().map(|(n, w)| format!("{} for {}", n, w)).collect();
                    // `gh auth refresh` acts on the active account only.
                    let refresh = format!("gh auth refresh -h github.com -s {}", names.join(","));
                    let command = if a.active == Some(false) {
                        format!("gh auth switch -h github.com -u {} && {}", a.login, refresh)
                    } else {
                        refresh
                    };
                    out.push(
                        Finding::new(
                            &format!("gh-token-scopes:{}", a.login),
                            "info",
                            format!("gh's token for @{} lacks {}", a.login, names.join(", ")),
                            format!(
                                "Its scopes: {}. It needs {}. In a terminal: {}",
                                if scopes.is_empty() { "none".to_string() } else { scopes.join(", ") },
                                why.join("; "),
                                command
                            ),
                        )
                        .with_fix(format!("copy:{}", command), "Copy the command"),
                    );
                }
            }
            _ => out.push(
                Finding::new(
                    "gh-not-authenticated",
                    "info",
                    "GitHub CLI has no signed-in accounts",
                    "Run `gh auth login` in a terminal so GitSwitch can verify repo access and register keys.",
                )
                .with_fix("copy:gh auth login", "Copy the command"),
            ),
        }
    }

    // ~/.ssh/config pinning a key for github.com defeats per-folder switching.
    let ssh_config_path = home().join(".ssh").join("config");
    if let Ok(cfg) = fs::read_to_string(&ssh_config_path) {
        for host in ssh_config_github_overrides(&cfg) {
            out.push(
                Finding::new(
                    "ssh-config-override",
                    "error",
                    format!("~/.ssh/config pins an SSH key for github.com (Host {})", host),
                    "This overrides the per-folder key GitSwitch sets, so every repo authenticates as the same account no matter which profile applies. The fix comments out the IdentityFile lines in that block (a timestamped backup is written first).",
                )
                .with_fix("fix-ssh-config", "Comment out & back up"),
            );
        }
    }

    // Hand-written includeIf rules can overlap the managed ones.
    let gitconfig_path = crate::git_config::git_home().join(".gitconfig");
    if let Ok(gc) = fs::read_to_string(&gitconfig_path) {
        let manual = manual_includeif_rules(&gc);
        if !manual.is_empty() {
            out.push(Finding::new(
                "manual-includeif",
                "warning",
                format!("{} hand-written includeIf rule(s) in ~/.gitconfig", manual.len()),
                format!(
                    "These live outside the GitSwitch block and can overlap its rules — git applies the LAST matching include, so results may not match what GitSwitch shows:\n{}",
                    manual.join("\n")
                ),
            ));
        }

        // Credential helpers make HTTPS remotes authenticate via gh/keychain,
        // completely bypassing the per-folder SSH key.
        if gc.contains("[credential \"https://github.com\"]") || gc.contains("credential.https://github.com") {
            out.push(Finding::new(
                "credential-helper",
                "info",
                "A credential helper is configured for github.com",
                "HTTPS remotes will authenticate through that helper (usually the gh CLI's active account) instead of your profile's SSH key. Keep remotes on SSH (git@github.com:…) so GitSwitch stays in control.",
            ));
        }
    }

    Ok(out)
}

/// Profile-level sanity: keys present on disk, passphrases, folder overlaps.
pub async fn check_profiles() -> Result<Vec<Finding>, AppError> {
    let store = profiles::load_profiles()?;
    let mut out = Vec::new();

    if store.profiles.is_empty() {
        out.push(
            Finding::new(
                "no-profiles",
                "info",
                "No profiles yet",
                "Create a profile and assign it a folder so git picks the right identity automatically.",
            )
            .with_fix("nav:/profile/new", "Create a profile"),
        );
        return Ok(out);
    }

    if !store.profiles.iter().any(|p| p.is_default) {
        out.push(
            Finding::new(
                "no-default",
                "warning",
                "No default profile",
                "Folders that aren't covered by any profile will fall back to whatever is in your global git config.",
            )
            .with_fix("nav:/", "Open Profiles"),
        );
    }

    for p in &store.profiles {
        match &p.ssh_key_path {
            None => out.push(
                Finding::new(
                    "profile-no-key",
                    "warning",
                    format!("Profile \"{}\" has no SSH key", p.name),
                    "Without a key, repos in this profile's folders push using whatever key ssh picks — which is how commits end up on the wrong account.",
                )
                .with_fix(format!("nav:/profile/{}/edit", p.id), "Edit profile"),
            ),
            Some(key) => {
                if !PathBuf::from(key).exists() {
                    out.push(
                        Finding::new(
                            "key-missing",
                            "error",
                            format!("Profile \"{}\" points at a missing key", p.name),
                            format!("{} no longer exists on disk. Generate a new key or pick an existing one.", key),
                        )
                        .with_fix(format!("nav:/profile/{}/edit", p.id), "Edit profile"),
                    );
                } else if ssh_keys::key_has_passphrase(key) {
                    out.push(
                        Finding::new(
                            "key-passphrase",
                            "warning",
                            format!("Profile \"{}\" uses a passphrase-protected key", p.name),
                            format!("{} needs a passphrase, so pushes prompt (or fail in scripts) unless the key is loaded in ssh-agent. Run `ssh-add {}` or use a passphrase-free key for automation.", key, crate::paths::shell_quote(key)),
                        )
                        .with_fix(format!("copy:ssh-add {}", crate::paths::shell_quote(key)), "Copy the command"),
                    );
                }
            }
        }

        if p.directories.is_empty() && !p.is_default {
            out.push(
                Finding::new(
                    "profile-no-folders",
                    "info",
                    format!("Profile \"{}\" has no folders", p.name),
                    "It will never apply to anything. Assign a folder, or make it the default profile.",
                )
                .with_fix(format!("nav:/profile/{}/edit", p.id), "Edit profile"),
            );
        }

        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                out.push(
                    Finding::new(
                        "folder-missing",
                        "warning",
                        format!("Folder assigned to \"{}\" doesn't exist", p.name),
                        format!("{} is not a directory (moved or deleted?). The rule is inert until it exists.", dir),
                    )
                    .with_fix(format!("drop-folder:{}:{}", p.id, dir), "Remove it from this profile"),
                );
                continue;
            }
            // Saved before folders were stored as the disk spells them, or
            // created afterwards in another letter case.
            let disk = crate::paths::disk_spelling(dir);
            if crate::paths::norm(&disk) != crate::paths::norm(dir) {
                out.push(
                    Finding::new(
                        "folder-spelling",
                        "warning",
                        format!("A folder of \"{}\" is spelled differently on disk", p.name),
                        format!(
                            "{} is {} on disk. git matches folder rules letter for letter: until the profile is saved again, repositories there may commit and push as another profile, and the app may list them under another.",
                            dir, disk
                        ),
                    )
                    .with_fix(format!("respell-folders:{}", p.id), "Save it as the disk spells it"),
                );
            }
        }
    }

    // A folder claimed by two profiles means git's last-match-wins decides —
    // not the app. (The app dedupes on save; this catches hand-edited stores.)
    // One folder may be named two ways: in another letter case, or through a
    // symbolic link.
    let mut seen: Vec<(&str, &str, &str, crate::paths::FolderId)> = Vec::new();
    for p in &store.profiles {
        for d in &p.directories {
            let id = crate::paths::FolderId::of(d);
            if let Some((_, other, other_dir, _)) = seen.iter().find(|(pid, _, _, s)| *pid != p.id && s.same(&id)) {
                let (a, b) = (crate::paths::norm(other_dir), crate::paths::norm(d));
                out.push(
                    Finding::new(
                        "folder-conflict",
                        "error",
                        "A folder is claimed by two profiles",
                        if a == b {
                            format!("{} is listed in both \"{}\" and \"{}\". Remove it from one of them.", b, other, p.name)
                        } else {
                            format!("{} in \"{}\" and {} in \"{}\" are the same folder. Remove it from one of them.", a, other, b, p.name)
                        },
                    )
                    .with_fix(format!("drop-folder:{}:{}", p.id, d), format!("Remove it from \"{}\"", p.name)),
                );
            }
            seen.push((p.id.as_str(), p.name.as_str(), d.as_str(), id));
        }
    }

    Ok(out)
}

/// One account's view of one repository, asked once per check: tokens and
/// answers are kept for the rest of the run. Ok(None) when gh has no token.
async fn access_for(
    acct: &str,
    owner: &str,
    name: &str,
    tokens: &mut std::collections::HashMap<String, Option<String>>,
    seen: &mut std::collections::HashMap<(String, String), crate::github::RepoAccess>,
) -> Result<Option<crate::github::RepoAccess>, AppError> {
    let key = (acct.to_lowercase(), format!("{}/{}", owner, name).to_lowercase());
    if let Some(a) = seen.get(&key) {
        return Ok(Some(a.clone()));
    }
    if !tokens.contains_key(acct) {
        tokens.insert(acct.to_string(), gh_cli::gh_get_token(acct).await.ok());
    }
    let Some(Some(token)) = tokens.get(acct).cloned() else { return Ok(None) };
    let a = crate::github::repo_access(&token, owner, name).await?;
    seen.insert(key, a.clone());
    Ok(Some(a))
}

/// Whether the account each repository in a profile folder pushes as can
/// see it on GitHub: the "Repository not found" a fetch would meet, found
/// before it. Asks GitHub with the GitHub CLI's tokens, so it runs when asked,
/// not with the other checks.
pub async fn check_repo_access() -> Result<Vec<Finding>, AppError> {
    use crate::github::RepoAccess;
    use std::collections::HashMap;
    let mut out = Vec::new();
    let accounts = gh_cli::gh_list_accounts().await.unwrap_or_default();
    if accounts.is_empty() {
        out.push(
            Finding::new(
                "access-unchecked",
                "info",
                "Repository access wasn't checked",
                "The GitHub CLI has no signed-in account to ask GitHub with. Run `gh auth login`, then check again.",
            )
            .with_fix("copy:gh auth login", "Copy the command"),
        );
        return Ok(out);
    }
    let store = profiles::load_profiles()?;
    let mut repos: Vec<repo_scan::ScannedRepo> = Vec::new();
    for p in &store.profiles {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            for r in repo_scan::scan_repos(dir.clone()).await.unwrap_or_default() {
                if !repos.iter().any(|x| x.path == r.path) {
                    repos.push(r);
                }
            }
        }
    }
    let gh = |l: &str| accounts.iter().find(|a| a.eq_ignore_ascii_case(l.trim().trim_start_matches('@'))).cloned();
    let mut tokens: HashMap<String, Option<String>> = HashMap::new();
    let mut seen: HashMap<(String, String), RepoAccess> = HashMap::new();
    let mut not_in_gh: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for r in repos.iter().filter(|r| !r.owner.is_empty() && !r.name.is_empty()) {
        let Some(owner) = crate::commit_audit::owning_profile(&r.path, &store.profiles) else { continue };
        let ident = crate::push_identity::push_identity(&r.path, false).await.ok();
        // Over SSH the profile's key decides the account; over HTTPS the
        // credential helper does, whatever profile the folder has.
        let over_ssh = ident.as_ref().is_some_and(|i| i.via == "ssh-key" || i.via == "ssh-agent");
        let over_https = ident.as_ref().is_some_and(|i| i.via == "https-credential");
        let pushes_as = ident.and_then(|i| i.login).or_else(|| owner.github_login.clone());
        let Some(login) = pushes_as else { continue };
        let Some(gh_login) = gh(&login) else {
            if !not_in_gh.contains(&login) {
                not_in_gh.push(login);
            }
            continue;
        };
        let slug = format!("{}/{}", r.owner, r.name);
        let access = match access_for(&gh_login, &r.owner, &r.name, &mut tokens, &mut seen).await {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(_) => {
                out.push(Finding::new("access-unreachable", "info", "GitHub could not be reached", "Repository access was not checked; try again when GitHub answers."));
                return Ok(out);
            }
        };
        match access {
            RepoAccess::Visible(_) => {}
            RepoAccess::Unknown(why) => unknown.push(format!("{} ({})", slug, why)),
            RepoAccess::Invisible => {
                let mut f = Finding::new(
                    &format!("remote-not-reachable:{}", r.path),
                    "error",
                    format!("@{} cannot see {}", gh_login, slug),
                    format!(
                        "Fetch, pull and push in {} authenticate as @{} (profile \"{}\"), and GitHub answers \"Repository not found\" to that account.",
                        r.path, gh_login, owner.name
                    ),
                );
                // Over HTTPS moving the repository changes nothing: converting
                // it to SSH is what puts a profile's account in charge.
                if over_https && crate::git_remote::https_to_ssh(&r.remote_url).is_some() {
                    f = f.with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH");
                }
                // Over SSH a profile whose own account can push there takes it
                // in one click — its key is what will authenticate.
                for other in store.profiles.iter().filter(|o| over_ssh && o.id != owner.id && o.ssh_key_path.is_some()) {
                    let Some(ol) = other.github_login.as_deref().and_then(gh) else { continue };
                    if let Ok(Some(RepoAccess::Visible(p))) = access_for(&ol, &r.owner, &r.name, &mut tokens, &mut seen).await {
                        if p.push {
                            f = f.with_fix(format!("assign:{}:{}", other.id, r.path), format!("Move it to \"{}\" (@{})", other.name, ol));
                            break;
                        }
                    }
                }
                out.push(f);
            }
        }
    }
    if !not_in_gh.is_empty() {
        out.push(Finding::new(
            "access-account-not-in-gh",
            "info",
            "Some repositories push as an account the GitHub CLI is not signed in to",
            format!("{} — sign in to it with `gh auth login` to check those repositories too.", not_in_gh.join(", ")),
        ));
    }
    if !unknown.is_empty() {
        out.push(Finding::new(
            "access-unknown",
            "info",
            format!("Access to {} repositor{} could not be told", unknown.len(), if unknown.len() == 1 { "y" } else { "ies" }),
            unknown.into_iter().take(5).collect::<Vec<_>>().join("; "),
        ));
    }
    Ok(out)
}

/// A Doctor repair the app performs itself, from its fix id.
#[derive(Debug, PartialEq)]
enum DoctorFix {
    DropFolder { profile_id: String, dir: String },
    RegisterKey { profile_id: String },
    RegisterSigning { login: String, profile_id: String },
    Assign { profile_id: String, path: String },
}

/// Profile ids are UUIDs and logins hold no ':', so the folder can come last
/// and keep a drive letter's colon (`C:/Users/me/work`).
fn parse_fix(fix: &str) -> Option<DoctorFix> {
    if let Some(rest) = fix.strip_prefix("drop-folder:") {
        let (id, dir) = rest.split_once(':')?;
        return (!id.is_empty() && !dir.is_empty()).then(|| DoctorFix::DropFolder { profile_id: id.into(), dir: dir.into() });
    }
    if let Some(id) = fix.strip_prefix("register-key:") {
        return (!id.is_empty()).then(|| DoctorFix::RegisterKey { profile_id: id.into() });
    }
    if let Some(rest) = fix.strip_prefix("assign:") {
        let (id, path) = rest.split_once(':')?;
        return (!id.is_empty() && !path.is_empty()).then(|| DoctorFix::Assign { profile_id: id.into(), path: path.into() });
    }
    if let Some(rest) = fix.strip_prefix("register-signing:") {
        let (login, id) = rest.split_once(':')?;
        return (!login.is_empty() && !id.is_empty()).then(|| DoctorFix::RegisterSigning { login: login.into(), profile_id: id.into() });
    }
    None
}

/// Run one of Doctor's own repairs. Each re-reads what it acts on and refuses
/// anything its finding would not have offered.
pub async fn apply_fix(fix: &str) -> Result<String, AppError> {
    let Some(f) = parse_fix(fix) else { return Err(AppError::Config(format!("Unknown repair: {}", fix))) };
    let store = profiles::load_profiles()?;
    let find = |id: &str| store.profiles.iter().find(|p| p.id == id).cloned();
    match f {
        DoctorFix::DropFolder { profile_id, dir } => {
            let Some(p) = find(&profile_id) else { return Err(AppError::NotFound("That profile no longer exists.".into())) };
            let target = crate::paths::FolderId::of(&dir);
            let missing = !PathBuf::from(&dir).is_dir();
            let claimed_elsewhere = store.profiles.iter().any(|o| o.id != p.id && o.directories.iter().any(|d| crate::paths::FolderId::of(d).same(&target)));
            if !missing && !claimed_elsewhere {
                return Err(AppError::Config(format!("{} exists and no other profile lists it: there is nothing to repair.", dir)));
            }
            let keep: Vec<String> = p.directories.iter().filter(|d| crate::paths::norm(d) != crate::paths::norm(&dir)).cloned().collect();
            // As the profile form saves: git's rules rewritten, and the commit
            // guards in the folder refreshed — the repositories there now
            // belong to the other profile, and a guard still expecting this
            // one's address would refuse every commit.
            crate::commands::update_profile_core(p.id.clone(), None, None, None, None, Some(keep), None, None, None, None).await?;
            Ok(format!("Removed {} from \"{}\"", dir, p.name))
        }
        DoctorFix::RegisterKey { profile_id } => {
            let Some(p) = find(&profile_id) else { return Err(AppError::NotFound("That profile no longer exists.".into())) };
            let key = p.ssh_key_path.clone().ok_or_else(|| AppError::Config(format!("\"{}\" has no SSH key.", p.name)))?;
            let login = p.github_login.clone().map(|l| l.trim().trim_start_matches('@').to_string()).filter(|l| !l.is_empty()).ok_or_else(|| AppError::Config(format!("\"{}\" names no GitHub account.", p.name)))?;
            // gh finds its token by the login spelled as gh stores it.
            let gh_login = gh_spelling(&gh_cli::gh_list_accounts().await.unwrap_or_default(), &login)
                .ok_or_else(|| AppError::Config(format!("The GitHub CLI is not signed in to @{}.", login)))?;
            crate::commands::gh_register_ssh_key(gh_login, key).await
        }
        DoctorFix::Assign { profile_id, path } => {
            let Some(p) = find(&profile_id) else { return Err(AppError::NotFound("That profile no longer exists.".into())) };
            if !PathBuf::from(&path).is_dir() {
                return Err(AppError::NotFound(format!("{} no longer exists.", path)));
            }
            let mut dirs = p.directories.clone();
            dirs.push(path.clone());
            crate::commands::update_profile_core(p.id.clone(), None, None, None, None, Some(dirs), None, None, None, None).await?;
            Ok(format!("{} now belongs to \"{}\"", path, p.name))
        }
        DoctorFix::RegisterSigning { login, profile_id } => {
            let Some(p) = find(&profile_id) else { return Err(AppError::NotFound("That profile no longer exists.".into())) };
            let key = p.ssh_key_path.clone().ok_or_else(|| AppError::Config(format!("\"{}\" has no SSH key.", p.name)))?;
            crate::commands::register_signing_key(login, key).await
        }
    }
}

/// A login as gh spells it: GitHub logins ignore case, gh's keyring does not.
fn gh_spelling(accounts: &[String], login: &str) -> Option<String> {
    accounts.iter().find(|a| a.eq_ignore_ascii_case(login)).cloned()
}

/// Which GitHub account does each profile's key actually authenticate as?
/// This is the check that catches "key registered on the wrong account".
pub async fn check_keys() -> Result<Vec<Finding>, AppError> {
    let store = profiles::load_profiles()?;
    let known_accounts = gh_cli::gh_list_accounts().await.unwrap_or_default();
    let mut out = Vec::new();

    for p in &store.profiles {
        let Some(key) = p.ssh_key_path.as_ref() else { continue };
        if !PathBuf::from(key).exists() {
            continue; // already reported by check_profiles
        }
        match ssh_keys::resolve_key_account(key).await {
            Ok(Some(login)) => {
                if p.signing_enabled {
                    match gh_cli::gh_get_token(&login).await {
                        Ok(token) => match crate::signing::signing_key_registered(
                            &token,
                            &ssh_keys::get_public_key(key).unwrap_or_default(),
                        )
                        .await
                        {
                            Ok(false) => out.push(
                                Finding::new(
                                    "signing-key-not-registered",
                                    "warning",
                                    format!("Profile \"{}\" signs commits, but the key isn't a GitHub signing key", p.name),
                                    "GitHub treats authentication keys and signing keys as different types. Until this key is registered as a SIGNING key, commits stay Unverified.",
                                )
                                .with_fix(format!("register-signing:{}:{}", login, p.id), format!("Register as signing key on @{}", login)),
                            ),
                            Ok(true) => {}
                            Err(e) => out.push(Finding::new(
                                "signing-key-check-failed",
                                "info",
                                format!("Couldn't confirm signing-key registration for \"{}\"", p.name),
                                format!("{}", e),
                            )),
                        },
                        Err(e) => out.push(Finding::new(
                            "signing-key-check-failed",
                            "info",
                            format!("Couldn't confirm signing-key registration for \"{}\"", p.name),
                            format!("{}", e),
                        )),
                    }
                }
                if !login.eq_ignore_ascii_case(p.git_name.trim()) {
                    // Only call it a mismatch when the git username looks like a
                    // real GitHub login; otherwise it's just a display name.
                    let looks_like_login = known_accounts
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(p.git_name.trim()));
                    let sev = if looks_like_login { "warning" } else { "info" };
                    out.push(
                        Finding::new(
                            "key-account-mismatch",
                            sev,
                            format!("Profile \"{}\" pushes as {}", p.name, login),
                            format!(
                                "Its key authenticates as GitHub user {}, but the profile's git username is \"{}\". Commits in these folders will be attributed to {}.",
                                login, p.git_name, login
                            ),
                        )
                        .with_fix(format!("nav:/profile/{}/edit", p.id), "Edit profile"),
                    );
                }
            }
            Ok(None) if ssh_keys::rejected_because_locked(ssh_keys::key_has_passphrase(key), ssh_keys::agent_holds_key(key).await) => out.push(
                Finding::new(
                    "key-locked",
                    "info",
                    format!("Couldn't verify profile \"{}\"'s key: it needs its passphrase", p.name),
                    format!(
                        "{} is passphrase-protected and not loaded in ssh-agent, so GitSwitch could not unlock it to ask github.com which account it belongs to. Pushes may still work: ssh can unlock it through your ssh config or keychain. Load it with `ssh-add {}` and run the checks again for a definite answer.",
                        key,
                        crate::paths::shell_quote(key)
                    ),
                )
                .with_fix(format!("copy:ssh-add {}", crate::paths::shell_quote(key)), "Copy the command"),
            ),
            Ok(None) => {
                // The profile's own GitHub account, when gh is signed in to it,
                // can take the key in one click; otherwise the SSH Keys page.
                let account = p
                    .github_login
                    .as_deref()
                    .map(|l| l.trim().trim_start_matches('@'))
                    .and_then(|l| known_accounts.iter().find(|a| a.eq_ignore_ascii_case(l)).cloned());
                let finding = Finding::new(
                    "key-not-registered",
                    "error",
                    format!("Profile \"{}\"'s key isn't on any GitHub account", p.name),
                    format!(
                        "{} was rejected by github.com. Register its .pub on the right account, or pushes from these folders will fail.",
                        key
                    ),
                );
                out.push(match account {
                    Some(a) => finding.with_fix(format!("register-key:{}", p.id), format!("Register on @{}", a)),
                    None => finding.with_fix("nav:/ssh", "Open SSH Keys"),
                });
            }
            Err(e) => out.push(Finding::new(
                "key-check-failed",
                "info",
                format!("Couldn't verify profile \"{}\"'s key", p.name),
                format!("{}", e),
            )),
        }
    }

    Ok(out)
}

/// Repo-level checks across every profile folder: remotes that bypass GitSwitch.
pub async fn check_repos() -> Result<Vec<Finding>, AppError> {
    let store = profiles::load_profiles()?;
    let mut out = Vec::new();
    let mut checked: Vec<String> = Vec::new();

    for p in &store.profiles {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            let repos = repo_scan::scan_repos(dir.clone()).await.unwrap_or_default();
            for r in repos {
                if checked.contains(&r.path) {
                    continue;
                }
                checked.push(r.path.clone());
                let name = crate::paths::base_name(&r.path);

                // A value in the repository's own .git/config wins over every
                // profile rule, silently. The Changes page shows that for the
                // open repository; this is the same check for all of them.
                if let Some(owner) = crate::commit_audit::owning_profile(&r.path, &store.profiles) {
                    for (key, value) in local_overrides(&r.path).await {
                        let expected = expected_local_value(owner, &key);
                        if expected.as_deref() == Some(value.as_str()) {
                            continue; // the same value twice: redundant, not wrong
                        }
                        out.push(
                            Finding::new(
                                "local-override",
                                "warning",
                                format!("{} overrides {} in its own config", name, key),
                                format!(
                                    "{} sets {} = {} in .git/config, which wins over the profile \"{}\" ({}). Removing the local setting lets the profile apply again.",
                                    r.path,
                                    key,
                                    value,
                                    owner.name,
                                    expected.map(|e| format!("which expects {}", e)).unwrap_or_else(|| "which sets nothing for it".into())
                                ),
                            )
                            .with_fix(format!("unset-local:{}:{}", key, r.path), "Remove the local setting"),
                        );
                    }
                }

                if r.remote_url.is_empty() {
                    continue; // no origin — nothing to push to yet
                }
                // origin's own push URL, when it has one: pushes go there, so a
                // token or an HTTPS address in it matters as much.
                if let Some(pu) = &r.push_url {
                    if pu.contains("***@") || pu.starts_with("https://") || pu.starts_with("http://") {
                        let token = pu.contains("***@");
                        let f = Finding::new(
                            if token { "remote-push-embedded-token" } else { "remote-push-https" },
                            if token { "error" } else { "warning" },
                            format!("{} pushes to {}", name, if token { "a URL with a token in it" } else { "an HTTPS address" }),
                            format!(
                                "{} fetches from {} but pushes to {} (remote.origin.pushurl).{}",
                                r.path,
                                r.remote_url,
                                pu,
                                if token { " The credential sits in plaintext in .git/config." } else { " HTTPS pushes authenticate through a credential helper, not your profile's SSH key." }
                            ),
                        );
                        out.push(if crate::git_remote::https_to_ssh(pu).is_some() { f.with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH") } else { f });
                    }
                }
                // Converting is offered only where there is a GitHub address
                // to convert to: a gist's own, or github.com's — never another
                // host's, nor one that merely contains "github.com".
                let convertible = crate::git_remote::https_to_ssh(&r.remote_url).is_some();
                // scan_repos masks credentials as `***@`, so this detects an
                // embedded token without ever surfacing the secret.
                if r.remote_url.contains("***@") {
                    let f = Finding::new(
                        "remote-embedded-token",
                        "error",
                        format!("{} has a token embedded in its remote URL", name),
                        format!(
                            "{} stores a credential in plaintext in .git/config, and pushes bypass your profile's SSH key entirely. {}",
                            r.path,
                            if convertible { "Converting to SSH removes the token." } else { "Remove it from the address (git remote set-url origin <the address without it>)." }
                        ),
                    );
                    out.push(if convertible { f.with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH") } else { f });
                } else if r.remote_url.starts_with("https://") || r.remote_url.starts_with("http://") {
                    let f = Finding::new(
                        "remote-https",
                        "warning",
                        format!("{} uses an HTTPS remote", name),
                        format!(
                            "{} → {}\nHTTPS pushes authenticate through a credential helper, not your profile's SSH key, so GitSwitch can't control which account they land on.",
                            r.path, r.remote_url
                        ),
                    );
                    out.push(if convertible { f.with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH") } else { f });
                } else if let Some(sr) = crate::ssh_config::ssh_host_of_url(&r.remote_url) {
                    // `git@gh-work:…` or `git@github.com-work:…` is an ssh alias:
                    // which host and which key it means is in ~/.ssh/config, and
                    // only `ssh -G` reads that the way ssh does (Match blocks
                    // included). resolve() is cached, so every remote is asked.
                    let host = sr.host;
                    {
                        if let Some(h) = crate::ssh_config::resolve(&host).await {
                            if !h.hostname.eq_ignore_ascii_case(&host) {
                                let profile_key = crate::commit_audit::owning_profile(&r.path, &store.profiles).and_then(|p| p.ssh_key_path.clone());
                                let key_note = match (&profile_key, h.identity_files.iter().find(|f| std::path::Path::new(f).is_file())) {
                                    (Some(k), _) => format!("This folder's profile sets the key {} through core.sshCommand, so that is the key used — the profile's key wins over the alias's.", crate::paths::base_name(k)),
                                    (None, Some(f)) => format!("This folder's profile has no key of its own, so ssh will use the alias's: {}.", f),
                                    (None, None) => "Neither the profile nor the alias names a key that exists, so ssh will try whatever the agent holds.".to_string(),
                                };
                                out.push(Finding::new(
                                    "ssh-alias",
                                    "info",
                                    format!("{} pushes through the ssh alias {}", name, host),
                                    format!("{} → {} (ssh -G). {}", r.remote_url, h.hostname, key_note),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(out)
}

/// The identity-related keys git may hold in a repository's own config.
const LOCAL_IDENTITY_KEYS: &[&str] = &["user.name", "user.email", "user.signingkey", "core.sshcommand", "commit.gpgsign", "gpg.format"];

/// `(key, value)` for every identity-related key set in the repository's own
/// config — `--local`, so a worktree or submodule is read correctly too.
async fn local_overrides(repo: &str) -> Vec<(String, String)> {
    let out = crate::git_exec::GitCmd::at(repo)
        .args(["config", "--local", "-z", "--get-regexp", r"^(user\.(name|email|signingkey)|core\.sshcommand|commit\.gpgsign|gpg\.format)$"])
        .run()
        .await;
    let Ok(out) = out else { return Vec::new() };
    if !out.ok() {
        return Vec::new(); // exit 1: nothing matched
    }
    out.text()
        .split('\0')
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let (k, v) = entry.split_once('\n').unwrap_or((entry, ""));
            (k.to_ascii_lowercase(), v.to_string())
        })
        .collect()
}

/// What the owning profile would set for `key` — None when it sets nothing.
fn expected_local_value(p: &profiles::Profile, key: &str) -> Option<String> {
    match key {
        "user.name" => Some(p.git_name.clone()),
        "user.email" => Some(p.git_email.clone()),
        "core.sshcommand" => p.ssh_key_path.as_deref().map(crate::git_config::ssh_command_for),
        "user.signingkey" if p.signing_enabled => p.ssh_key_path.as_ref().map(|k| format!("{}.pub", crate::paths::norm(k))),
        "commit.gpgsign" if p.signing_enabled => Some("true".into()),
        "gpg.format" if p.signing_enabled => Some("ssh".into()),
        _ => None,
    }
}

/// The repair: drop one identity key from the repository's own config so the
/// profile's rule applies again. Only the keys above may be touched.
pub async fn unset_local(repo_path: &str, key: &str) -> Result<String, AppError> {
    let key_lc = key.to_ascii_lowercase();
    if !LOCAL_IDENTITY_KEYS.contains(&key_lc.as_str()) {
        return Err(AppError::Config(format!("{} is not an identity setting GitSwitch manages.", key)));
    }
    let out = crate::git_exec::GitCmd::at(repo_path)
        .args(["config", "--local", "--unset-all", &key_lc])
        .run()
        .await?;
    // 5 = the key was not set, which is the state we wanted anyway.
    if !out.ok() && out.code != 5 {
        return Err(AppError::Command(format!("git config --unset-all {} failed: {}", key_lc, out.stderr)));
    }
    Ok(format!("Removed {} from {}'s own config; the profile applies again.", key_lc, crate::paths::base_name(repo_path)))
}

/// "2.50.1" from `git --version` ("git version 2.50.1 (Apple Git-155)").
async fn git_version() -> Option<String> {
    let out = crate::spawn::async_command("git").arg("--version").output().await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().nth(2).map(|v| v.to_string())
}

/// The first `git` on PATH, resolved the way a shell would.
fn git_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "git.exe" } else { "git" });
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
async fn sudo_would_not_ask() -> bool {
    // `-n` never prompts: success means a live timestamp or a NOPASSWD rule.
    let run = crate::spawn::async_command("sudo").args(["-n", "true"]).output();
    matches!(tokio::time::timeout(std::time::Duration::from_secs(5), run).await, Ok(Ok(o)) if o.status.success())
}

#[cfg(not(unix))]
async fn sudo_would_not_ask() -> bool {
    false
}

/// The push lock: its admin-owned layers, its helper, and every locked
/// repository. Fix ids all start with `lock-` and run through `push_lock::fix`.
pub async fn check_push_locks() -> Result<Vec<Finding>, AppError> {
    use crate::push_lock;
    let mut out = Vec::new();
    let status = push_lock::helper_status().await;
    if !status.supported {
        return Ok(out);
    }
    let has_locks = !status.locks.is_empty();
    match status.registry.as_str() {
        "missing" => {
            if status.installed {
                out.push(
                    Finding::new(
                        "push-lock-registry-missing",
                        "warning",
                        "The lock helper is installed but its registry is gone",
                        "Without the registry no repository is locked. Rebuild it, or remove the helper from Settings if you no longer use push locks.",
                    )
                    .with_fix("lock-repair-system", "Rebuild (administrator)"),
                );
            }
            return Ok(out);
        }
        "untrusted" => out.push(
            Finding::new(
                "push-lock-registry-untrusted",
                "error",
                "The push-lock registry is not administrator-owned",
                "Someone or something replaced the registry directory or file with one this account can write. Until it is repaired, no lock can be trusted.",
            )
            .with_fix("lock-repair-system", "Repair (administrator)"),
        ),
        "unreadable" => out.push(
            Finding::new(
                "push-lock-registry-unreadable",
                "error",
                "The push-lock registry cannot be read",
                "It may have been written by a newer GitSwitch, or damaged. Repairing rewrites it from the helper's own records.",
            )
            .with_fix("lock-repair-system", "Repair (administrator)"),
        ),
        _ => {}
    }

    match status.helper.as_str() {
        "missing" if has_locks => out.push(
            Finding::new(
                "push-lock-helper-missing",
                "error",
                "The lock helper is missing",
                format!(
                    "{} repositories are locked, but the program that applies and removes locks is gone from {}. Locks still hold; they cannot be changed until it is reinstalled.",
                    status.locks.len(),
                    status.helper_path.clone().unwrap_or_default()
                ),
            )
            .with_fix("lock-bootstrap", "Reinstall the helper (administrator)"),
        ),
        "untrusted" => out.push(
            Finding::new(
                "push-lock-helper-untrusted",
                "error",
                "The installed lock helper is not the one the registry vouches for",
                "Its checksum or ownership no longer matches. Reinstalling replaces it with the copy shipped inside this GitSwitch.",
            )
            .with_fix("lock-upgrade-helper", "Reinstall the helper (administrator)"),
        ),
        "outdated" => out.push(
            Finding::new(
                "push-lock-helper-outdated",
                "info",
                "A newer lock helper ships with this GitSwitch",
                format!(
                    "Installed: {}. Bundled: {}. Upgrading needs the administrator prompt once.",
                    status.installed_version.clone().unwrap_or_else(|| "unknown".into()),
                    status.bundled_version.clone().unwrap_or_else(|| "unknown".into())
                ),
            )
            .with_fix("lock-upgrade-helper", "Upgrade (administrator)"),
        ),
        _ => {}
    }

    if has_locks {
        match status.remote_helper.as_str() {
            "missing" | "foreign" => out.push(
                Finding::new(
                    "push-lock-remote-helper-missing",
                    "info",
                    "A refused push shows git's generic error instead of GitSwitch's explanation",
                    "The small program named git-remote-gitswitch-push-blocked is missing or is not GitSwitch's. Enforcement does not depend on it; the explanation does.",
                )
                .with_fix("lock-install-remote-helper", "Install it (administrator)"),
            ),
            "unprotected-dir" => out.push(Finding::new(
                "push-lock-remote-helper-unprotected-dir",
                "info",
                "The refused-push explanation lives in a folder this account can write",
                "Usually a Homebrew-owned /usr/local/bin. Someone could replace the explanation; the lock itself is not affected.",
            )),
            _ => {}
        }
    }

    for l in &status.locks {
        let name = crate::paths::base_name(&l.repo);
        if !l.exists {
            out.push(
                Finding::new(
                    &format!("push-lock-repo-missing:{}", l.repo),
                    "warning",
                    format!("{} is locked but no longer exists", name),
                    format!("{} was moved or deleted. A repository moved elsewhere is not locked at its new place — lock it again there. Forgetting removes the stale entry.", l.repo),
                )
                .with_fix(format!("lock-forget:{}", l.repo), "Forget this lock (administrator)"),
            );
            continue;
        }
        // The submodules the lock does not reach: the code lives there.
        let repo_dir = std::path::Path::new(&l.repo);
        let unlocked: Vec<String> = crate::push_guard::populated_submodules(repo_dir)
            .await
            .into_iter()
            .filter(|rel| !status.locks.iter().any(|x| x.repo == push_lock::canonical(&repo_dir.join(rel))))
            .collect();
        if !unlocked.is_empty() {
            out.push(Finding::new(
                &format!("push-lock-submodules-uncovered:{}", l.repo),
                "warning",
                format!("{} is locked, but {} of its submodules {} not", name, unlocked.len(), if unlocked.len() == 1 { "is" } else { "are" }),
                format!(
                    "A push from inside {} is not stopped by the lock. Open {} on the Repository page and use Repair (administrator) under Push access to lock them too.",
                    unlocked.join(", "),
                    name
                ),
            ));
        }
        let state = match crate::git_status::push_state_for(&l.repo).await {
            Ok(s) => s,
            Err(e) => {
                out.push(Finding::new(
                    &format!("push-lock-repo-unreadable:{}", l.repo),
                    "warning",
                    format!("{} is locked but could not be read", name),
                    format!("{}", e),
                ));
                continue;
            }
        };
        let lk = &state.lock;
        if lk.needs_elevation {
            let codes: Vec<&String> = lk
                .drift
                .iter()
                .filter(|d| !d.starts_with("mirror-") && !d.starts_with("explicit-pushurl") && *d != "hook-redirected")
                .collect();
            out.push(
                Finding::new(
                    &format!("push-lock-system-drift:{}", l.repo),
                    "error",
                    format!("{}'s lock is not fully in place", name),
                    format!(
                        "The administrator-owned rules for {} have drifted ({}). Until repaired, a push may not be refused.",
                        l.repo,
                        codes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                )
                .with_fix(format!("lock-repair:{}", l.repo), "Repair (administrator)"),
            );
        }
        if lk.mirrors == "unfixable" {
            out.push(Finding::new(
                &format!("push-lock-hook-redirected:{}", l.repo),
                "warning",
                format!("{}: core.hooksPath sends hooks elsewhere", name),
                format!(
                    "GitSwitch will not write its pre-push hook into {}, so a remote with an explicit push URL is stopped by nothing. The system-scope rewrite still applies to every other remote.",
                    state.hooks_path_overridden.clone().unwrap_or_default()
                ),
            ));
        }
        if lk.mirrors == "healed" {
            out.push(Finding::new(
                &format!("push-lock-mirrors-healed:{}", l.repo),
                "info",
                format!("{}: the user-level mirrors were re-applied just now", name),
                "Something removed the repository's own flag, rewrite or hook since the last look. The lock's administrator-owned rules were unaffected; see the card's recent events.",
            ));
        }
        for r in state.remotes.iter().filter(|r| r.has_explicit_pushurl) {
            out.push(
                Finding::new(
                    &format!("push-lock-explicit-pushurl:{}", l.repo),
                    "warning",
                    format!("{}: remote '{}' has an explicit push URL", name, r.name),
                    "remote.<name>.pushurl is exempt from pushInsteadOf, so only the pre-push hook stops a push to it — and `--no-verify` skips hooks. Neutralising stores the push URL under gitswitch.savedpushurl.<remote> and removes it; unlocking puts it back.",
                )
                .with_fix(format!("lock-neutralise-pushurl:{}", l.repo), "Store and neutralise pushurl"),
            );
        }
    }

    if has_locks {
        let system_git = push_lock::system_git();
        if let Some(on_path) = git_on_path() {
            let same = match (std::fs::canonicalize(&on_path), std::fs::canonicalize(&system_git)) {
                (Ok(a), Ok(b)) => a == b,
                _ => on_path == system_git,
            };
            if !same && system_git.is_absolute() {
                out.push(Finding::new(
                    "push-lock-other-git",
                    "info",
                    "A different git comes first on your PATH",
                    format!(
                        "{} is what your terminal runs; the lock lives in the system config of {}. That other git reads its own system file and is not covered.",
                        on_path.display(),
                        system_git.display()
                    ),
                ));
            }
        }
        if let Some(uac) = &status.uac {
            if !uac.enabled || uac.admin_behavior == 0 {
                out.push(Finding::new(
                    "push-lock-uac-off",
                    "warning",
                    "User Account Control is turned off, so unlocking never asks",
                    "With UAC disabled (or set to \"never notify\"), any program on this account can run the lock helper elevated without a prompt. Turn UAC back on in Windows settings for the lock to mean anything.",
                ));
            } else if matches!(uac.admin_behavior, 2 | 5) {
                out.push(Finding::new(
                    "push-lock-uac-consent",
                    "info",
                    "On this administrator account the unlock prompt is a click, not a password",
                    "Windows asks administrators for consent (\"Yes\"), which anyone at the keyboard — or software that can click for you — can give. For a real password prompt, work from a standard account and keep a separate administrator account.",
                ));
            }
        }
        if sudo_would_not_ask().await {
            out.push(Finding::new(
                "push-lock-sudo-cached",
                "info",
                "sudo would not ask this terminal for a password right now",
                "A live sudo timestamp or a NOPASSWD rule means the unlock helper could be run from a terminal without a new password until it expires. GitSwitch's own prompt is unaffected.",
            ));
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Fixes
// ---------------------------------------------------------------------------

/// Comment out IdentityFile/IdentitiesOnly lines in ssh config blocks that
/// apply to github.com. Backs the file up first and never deletes anything.
pub fn fix_ssh_config() -> Result<String, AppError> {
    let path = home().join(".ssh").join("config");
    let content = fs::read_to_string(&path)
        .map_err(|e| AppError::Config(format!("Can't read ~/.ssh/config: {}", e)))?;

    let offenders = ssh_config_github_overrides(&content);
    if offenders.is_empty() {
        return Ok("Nothing to change — no github.com block pins a key.".into());
    }

    let backup = path.with_extension(format!(
        "gitswitch-backup.{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    fs::copy(&path, &backup)?;

    let mut out = String::new();
    let mut in_github_block = false;
    let mut commented = 0;

    for raw in content.lines() {
        let trimmed = raw.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("host ") {
            in_github_block = rest
                .split_whitespace()
                .any(|p| pattern_matches(p, "github.com"));
        } else if in_github_block
            && !trimmed.starts_with('#')
            && (lower.starts_with("identityfile ") || lower.starts_with("identitiesonly "))
        {
            out.push_str(&format!("# disabled by GitSwitch: {}\n", trimmed));
            commented += 1;
            continue;
        }
        out.push_str(raw);
        out.push('\n');
    }

    fs::write(&path, out)?;
    Ok(format!(
        "Commented out {} line(s) in ~/.ssh/config. Backup: {}",
        commented,
        backup.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_handed_to_gh_as_gh_spells_it() {
        let accounts = vec!["Kshitij-Example".to_string(), "other".to_string()];
        assert_eq!(gh_spelling(&accounts, "kshitij-example").as_deref(), Some("Kshitij-Example"));
        assert_eq!(gh_spelling(&accounts, "nobody"), None);
    }

    #[test]
    fn glob_matches_ssh_host_patterns() {
        assert!(pattern_matches("github.com", "github.com"));
        assert!(pattern_matches("*", "github.com"));
        assert!(pattern_matches("github.*", "github.com"));
        assert!(pattern_matches("*.com", "github.com"));
        assert!(!pattern_matches("github.com-work", "github.com"));
        assert!(!pattern_matches("gitlab.com", "github.com"));
    }

    #[test]
    fn detects_only_blocks_that_pin_a_key_for_github() {
        let cfg = "\
Host github.com\n    IdentityFile ~/.ssh/id_personal\n    IdentitiesOnly yes\n\n\
Host github.com-work\n    IdentityFile ~/.ssh/id_work\n\n\
Host other\n    User bob\n";
        let found = ssh_config_github_overrides(cfg);
        // Only the exact github.com block counts; the alias is opt-in per remote.
        assert_eq!(found, vec!["github.com".to_string()]);
    }

    #[test]
    fn wildcard_host_with_identity_is_flagged() {
        let cfg = "Host *\n    IdentityFile ~/.ssh/id_rsa\n";
        assert_eq!(ssh_config_github_overrides(cfg), vec!["*".to_string()]);
    }

    #[test]
    fn block_without_identityfile_is_not_flagged() {
        let cfg = "Host github.com\n    HostName github.com\n    User git\n";
        assert!(ssh_config_github_overrides(cfg).is_empty());
    }

    #[test]
    fn commented_lines_are_ignored() {
        let cfg = "Host github.com\n    # IdentityFile ~/.ssh/old\n";
        assert!(ssh_config_github_overrides(cfg).is_empty());
    }

    #[test]
    fn finds_only_unmanaged_includeif_rules() {
        let gc = "\
[includeIf \"gitdir:/a/\"]\n\tpath = /x\n\
# >>> GitSwitch managed (DO NOT EDIT) >>>\n\
[includeIf \"gitdir:/b/\"]\n\tpath = /y\n\
# <<< GitSwitch managed (DO NOT EDIT) <<<\n\
[includeIf \"gitdir:/c/\"]\n\tpath = /z\n";
        let manual = manual_includeif_rules(gc);
        assert_eq!(manual.len(), 2);
        assert!(manual[0].contains("/a/"));
        assert!(manual[1].contains("/c/"));
    }
}

#[cfg(test)]
mod fix_tests {
    use super::*;

    #[test]
    fn a_fix_id_names_its_repair_and_keeps_a_drive_letter() {
        assert_eq!(
            parse_fix("drop-folder:0f8a1c2e-1111-2222-3333-444455556666:C:/Users/me/work"),
            Some(DoctorFix::DropFolder { profile_id: "0f8a1c2e-1111-2222-3333-444455556666".into(), dir: "C:/Users/me/work".into() })
        );
        assert_eq!(parse_fix("register-signing:octo:abc"), Some(DoctorFix::RegisterSigning { login: "octo".into(), profile_id: "abc".into() }));
        assert_eq!(parse_fix("register-key:abc"), Some(DoctorFix::RegisterKey { profile_id: "abc".into() }));
        assert_eq!(parse_fix("nav:/ssh"), None);
        assert_eq!(parse_fix("drop-folder:"), None);
    }
}
