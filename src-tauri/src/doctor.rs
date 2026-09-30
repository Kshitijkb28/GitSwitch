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
    tokio::process::Command::new(bin)
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
    if !command_exists("gh").await {
        out.push(Finding::new(
            "gh-missing",
            "info",
            "GitHub CLI (gh) not found",
            "Optional, but it powers one-click key registration, account sign-in, autofill and Auto Assign. Install from https://cli.github.com",
        ));
    } else if gh_cli::gh_list_accounts().await.is_err() {
        out.push(Finding::new(
            "gh-not-authenticated",
            "info",
            "GitHub CLI has no signed-in accounts",
            "Run `gh auth login` so GitSwitch can verify repo access and register keys.",
        ));
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
    let gitconfig_path = home().join(".gitconfig");
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
        out.push(Finding::new(
            "no-profiles",
            "info",
            "No profiles yet",
            "Create a profile and assign it a folder so git picks the right identity automatically.",
        ));
        return Ok(out);
    }

    if !store.profiles.iter().any(|p| p.is_default) {
        out.push(Finding::new(
            "no-default",
            "warning",
            "No default profile",
            "Folders that aren't covered by any profile will fall back to whatever is in your global git config.",
        ));
    }

    for p in &store.profiles {
        match &p.ssh_key_path {
            None => out.push(Finding::new(
                "profile-no-key",
                "warning",
                format!("Profile \"{}\" has no SSH key", p.name),
                "Without a key, repos in this profile's folders push using whatever key ssh picks — which is how commits end up on the wrong account.",
            )),
            Some(key) => {
                if !PathBuf::from(key).exists() {
                    out.push(Finding::new(
                        "key-missing",
                        "error",
                        format!("Profile \"{}\" points at a missing key", p.name),
                        format!("{} no longer exists on disk. Generate a new key or pick an existing one.", key),
                    ));
                } else if ssh_keys::key_has_passphrase(key) {
                    out.push(Finding::new(
                        "key-passphrase",
                        "warning",
                        format!("Profile \"{}\" uses a passphrase-protected key", p.name),
                        format!("{} needs a passphrase, so pushes prompt (or fail in scripts) unless the key is loaded in ssh-agent. Run `ssh-add {}` or use a passphrase-free key for automation.", key, key),
                    ));
                }
            }
        }

        if p.directories.is_empty() && !p.is_default {
            out.push(Finding::new(
                "profile-no-folders",
                "info",
                format!("Profile \"{}\" has no folders", p.name),
                "It will never apply to anything. Assign a folder, or make it the default profile.",
            ));
        }

        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                out.push(Finding::new(
                    "folder-missing",
                    "warning",
                    format!("Folder assigned to \"{}\" doesn't exist", p.name),
                    format!("{} is not a directory (moved or deleted?). The rule is inert until it exists.", dir),
                ));
            }
        }
    }

    // A folder claimed by two profiles means git's last-match-wins decides —
    // not the app. (The app dedupes on save; this catches hand-edited stores.)
    let mut seen: Vec<(String, String)> = Vec::new();
    for p in &store.profiles {
        for d in &p.directories {
            let norm = crate::paths::norm(d);
            if let Some((other, _)) = seen.iter().find(|(_, dd)| dd == &norm) {
                out.push(Finding::new(
                    "folder-conflict",
                    "error",
                    "A folder is claimed by two profiles",
                    format!("{} is listed in both \"{}\" and \"{}\". Remove it from one of them.", norm, other, p.name),
                ));
            }
            seen.push((p.name.clone(), norm));
        }
    }

    Ok(out)
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
                            Ok(false) => out.push(Finding::new(
                                "signing-key-not-registered",
                                "warning",
                                format!("Profile \"{}\" signs commits, but the key isn't a GitHub signing key", p.name),
                                "GitHub treats authentication keys and signing keys as different types. Until this key is registered as a SIGNING key, commits stay Unverified. Add it from the profile's Commit Signing section.",
                            )),
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
                    out.push(Finding::new(
                        "key-account-mismatch",
                        sev,
                        format!("Profile \"{}\" pushes as {}", p.name, login),
                        format!(
                            "Its key authenticates as GitHub user {}, but the profile's git username is \"{}\". Commits in these folders will be attributed to {}.",
                            login, p.git_name, login
                        ),
                    ));
                }
            }
            Ok(None) => out.push(Finding::new(
                "key-not-registered",
                "error",
                format!("Profile \"{}\"'s key isn't on any GitHub account", p.name),
                format!(
                    "{} was rejected by github.com. Register its .pub on the right account (SSH Keys → Register), or pushes from these folders will fail.",
                    key
                ),
            )),
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

                if r.remote_url.is_empty() {
                    continue; // no origin — nothing to push to yet
                }
                // scan_repos masks credentials as `***@`, so this detects an
                // embedded token without ever surfacing the secret.
                if r.remote_url.contains("***@") {
                    out.push(
                        Finding::new(
                            "remote-embedded-token",
                            "error",
                            format!("{} has a token embedded in its remote URL", name),
                            format!(
                                "{} stores a credential in plaintext in .git/config, and pushes bypass your profile's SSH key entirely. Converting to SSH removes the token.",
                                r.path
                            ),
                        )
                        .with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH"),
                    );
                } else if r.remote_url.starts_with("https://") || r.remote_url.starts_with("http://") {
                    out.push(
                        Finding::new(
                            "remote-https",
                            "warning",
                            format!("{} uses an HTTPS remote", name),
                            format!(
                                "{} → {}\nHTTPS pushes authenticate through a credential helper, not your profile's SSH key, so GitSwitch can't control which account they land on.",
                                r.path, r.remote_url
                            ),
                        )
                        .with_fix(format!("convert-ssh:{}", r.path), "Convert to SSH"),
                    );
                }
            }
        }
    }

    Ok(out)
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
    let run = tokio::process::Command::new("sudo").args(["-n", "true"]).output();
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
