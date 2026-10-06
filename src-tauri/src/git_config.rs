use crate::error::AppError;
use crate::paths::{config_subsection, config_value, norm, shell_quote};
use crate::profiles::{load_profiles, Profile};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;

/// The directory git itself calls `~`: `$HOME` first on every platform; on
/// Windows, when HOME is unset, `HOMEDRIVE`+`HOMEPATH`, then `USERPROFILE`.
/// `dirs::home_dir()` answers USERPROFILE regardless, so for a Git Bash or
/// MSYS user with HOME set the app would write a config where git never looks.
pub(crate) fn git_home() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    {
        if let (Some(drive), Some(path)) = (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
            let mut s = drive;
            s.push(&path);
            return PathBuf::from(s);
        }
        if let Some(u) = std::env::var_os("USERPROFILE").filter(|u| !u.is_empty()) {
            return PathBuf::from(u);
        }
    }
    dirs::home_dir().unwrap_or_default()
}

fn get_gitconfig_path() -> PathBuf {
    git_home().join(".gitconfig")
}

const MAX_BACKUPS: usize = 5;

fn backup_file(path: &PathBuf) -> Result<(), AppError> {
    if path.exists() {
        let timestamp = Utc::now().format("%Y%m%d%H%M%S");
        let backup_path = path.with_extension(format!("backup.{}", timestamp));
        fs::copy(path, &backup_path)?;
        prune_backups(path);
    }
    Ok(())
}

/// Keep only the newest MAX_BACKUPS `<file>.backup.<timestamp>` siblings.
/// Timestamps are fixed-width, so lexicographic order == chronological order.
fn prune_backups(path: &std::path::Path) {
    let (Some(parent), Some(file_name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return;
    };
    let prefix = format!("{}.backup.", file_name);

    let Ok(entries) = fs::read_dir(parent) else { return };
    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();

    backups.sort();
    if backups.len() > MAX_BACKUPS {
        for old in &backups[..backups.len() - MAX_BACKUPS] {
            let _ = fs::remove_file(old);
        }
    }
}

/// Build every includeIf block across all profiles, ordered by path length so
/// the MOST SPECIFIC directory comes LAST. git resolves overlapping includes
/// with last-match-wins, so this makes a nested repo folder correctly override
/// its parent folder's profile (and matches the UI's longest-prefix logic).
fn build_includeif_blocks(profiles: &[Profile]) -> String {
    let mut blocks = String::new();

    // Remote rules FIRST, so a folder rule below — the more deliberate
    // statement about one place on disk — wins where both match (git applies
    // the last matching include).
    let mut remote_entries: Vec<(String, String)> = Vec::new();
    for profile in profiles {
        for pattern in &profile.remote_patterns {
            for glob in remote_globs(pattern) {
                remote_entries.push((glob, get_profile_gitconfig_path(profile)));
            }
        }
    }
    remote_entries.sort();
    for (glob, path) in remote_entries {
        blocks.push_str(&format!(
            "[includeIf \"hasconfig:remote.*.url:{}\"]\n\tpath = {}\n\n",
            config_subsection(&glob),
            config_value(&path)
        ));
    }

    let mut entries: Vec<(String, String)> = Vec::new();
    for profile in profiles {
        for dir in &profile.directories {
            entries.push((
                crate::paths::gitdir_pattern(dir),
                get_profile_gitconfig_path(profile),
            ));
        }
    }
    entries.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));

    for (dir, path) in entries {
        blocks.push_str(&format!(
            "[includeIf \"gitdir:{}\"]\n\tpath = {}\n\n",
            config_subsection(&dir),
            config_value(&path)
        ));
    }
    blocks
}

/// GitHub owner and repository names are case-insensitive; git's `hasconfig`
/// match is not. Each letter becomes a two-member character class so
/// `github.com/Acme/Thing` and `github.com/acme/thing` both match.
fn case_insensitive_glob(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 4);
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            out.push('[');
            out.push(c.to_ascii_uppercase());
            out.push(c.to_ascii_lowercase());
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out
}

/// The URL globs one remote rule stands for. `hasconfig:remote.*.url` is
/// matched against each remote's URL as written in the repository's config
/// (no `insteadOf` rewriting), so every spelling GitHub hands out is listed:
/// HTTPS, scp-like SSH, and `ssh://` with and without the port. An `owner/`
/// rule covers every repository of that owner; `owner/repo` covers that one
/// repository with or without `.git`. A full glob is used as it is.
pub fn remote_globs(pattern: &str) -> Vec<String> {
    const GITHUB: &[&str] = &["https://github.com/", "git@github.com:", "ssh://git@github.com/", "ssh://git@github.com:22/"];
    if pattern.starts_with("https://") || pattern.starts_with("http://") || pattern.starts_with("ssh://") || pattern.starts_with("git@") {
        return vec![pattern.to_string()];
    }
    let trimmed = pattern.trim_end_matches('/');
    let (owner, repo) = match trimmed.split_once('/') {
        Some((o, r)) => (o, Some(r)),
        None => (trimmed, None),
    };
    if owner.is_empty() {
        return Vec::new();
    }
    let owner = case_insensitive_glob(owner);
    let mut out = Vec::new();
    match repo.filter(|r| !r.is_empty()) {
        None => {
            for prefix in GITHUB {
                out.push(format!("{}{}/**", prefix, owner));
            }
        }
        Some(r) => {
            let r = case_insensitive_glob(r);
            for prefix in GITHUB {
                out.push(format!("{}{}/{}", prefix, owner, r));
                out.push(format!("{}{}/{}.git", prefix, owner, r));
            }
        }
    }
    out
}

/// The `[user]` section a profile commits as. Quoted: a `#` in a name or a
/// `;` in an email would otherwise be read as the start of a comment.
fn user_section(profile: &Profile) -> String {
    format!(
        "[user]\n\tname = {}\n\temail = {}\n",
        config_value(&profile.git_name),
        config_value(&profile.git_email)
    )
}

/// The command git runs to reach a remote over SSH with exactly this key.
/// git hands it to `sh -c`, so the path is shell-quoted: a space in it would
/// otherwise split into a key called `My` and a host called `Keys/id`.
/// Shared with the clone path, which passes the same string on the command
/// line (`-c core.sshCommand=…`), where it needs no config-file quoting.
pub(crate) fn ssh_command_for(key: &str) -> String {
    format!("ssh -i {} -o IdentitiesOnly=yes", shell_quote(&norm(key)))
}

/// NOTE: every path written into a git config VALUE must use forward slashes.
/// git treats `\` as an escape character, so a Windows path like
/// `C:\Users\me\...` produces `fatal: bad config line` and makes the entire
/// ~/.gitconfig unreadable — not just for GitSwitch, for every git command.
fn get_profile_gitconfig_path(profile: &Profile) -> String {
    let data_dir = dirs::data_dir().unwrap_or_default();
    let app_dir = data_dir.join("com.gitswitch.app").join("gitconfigs");
    crate::paths::norm(&format!("{}/{}.gitconfig", app_dir.display(), profile.id))
}

/// Config block that makes `git push` fail locally with a recognizable error
/// ("remote helper 'gitswitch-push-blocked' aborted") while leaving fetch/pull
/// untouched — pushes get their URL rewritten to a scheme git can't handle.
///
/// pushInsteadOf is a literal prefix match, so cover every common github.com
/// remote form. Known limits (documented in the UI): custom ssh-config host
/// aliases and remotes with an explicit remote.<name>.pushurl are not caught,
/// a LONGER rewrite in any scope wins over these (git picks the longest
/// match), and `GIT_CONFIG_NOSYSTEM` / another git binary skip config
/// altogether. This section is only ever written into per-folder include
/// files, never the main ~/.gitconfig: a block there would apply to every
/// folder, and undoing it per folder would need a longer rewrite for each URL
/// form. The push lock (push_lock.rs) is the variant that needs the
/// administrator password to remove.
fn push_block_section() -> &'static str {
    "[url \"gitswitch-push-blocked://\"]\n\
     \tpushInsteadOf = git@github.com:\n\
     \tpushInsteadOf = git@github.com/\n\
     \tpushInsteadOf = ssh://git@github.com/\n\
     \tpushInsteadOf = ssh://git@github.com:22/\n\
     \tpushInsteadOf = ssh://git@ssh.github.com/\n\
     \tpushInsteadOf = ssh://git@ssh.github.com:443/\n\
     \tpushInsteadOf = https://github.com/\n\
     \tpushInsteadOf = http://github.com/\n\
     \tpushInsteadOf = github.com:\n"
}

/// Everything one folder profile's include file says. Pure, so a test can
/// hand it to `git config -f` and check git reads back what was meant.
fn profile_gitconfig_content(profile: &Profile) -> String {
    let mut content = user_section(profile);

    if let Some(ref key_path) = profile.ssh_key_path {
        content.push_str(&format!(
            "[core]\n\tsshCommand = {}\n",
            config_value(&ssh_command_for(key_path))
        ));
    }

    if !profile.allow_push {
        content.push_str(push_block_section());
    }

    if let Some(block) = crate::signing::signing_config_block(profile) {
        content.push_str(&block);
    }
    content
}

fn write_profile_gitconfig(profile: &Profile) -> Result<(), AppError> {
    let data_dir = dirs::data_dir().ok_or(AppError::Config("Cannot find data dir".into()))?;
    let config_dir = data_dir.join("com.gitswitch.app").join("gitconfigs");
    fs::create_dir_all(&config_dir)?;

    let path = config_dir.join(format!("{}.gitconfig", profile.id));
    fs::write(&path, profile_gitconfig_content(profile))?;
    Ok(())
}

pub fn apply_git_config() -> Result<(), AppError> {
    let store = load_profiles()?;
    let gitconfig_path = get_gitconfig_path();

    backup_file(&gitconfig_path)?;

    let existing_content = if gitconfig_path.exists() {
        fs::read_to_string(&gitconfig_path)?
    } else {
        String::new()
    };

    let cleaned = remove_gitswitch_sections(&existing_content);

    let mut new_content = cleaned;
    if !new_content.ends_with('\n') && !new_content.is_empty() {
        new_content.push('\n');
    }

    new_content.push_str("\n# >>> GitSwitch managed (DO NOT EDIT) >>>\n");

    if let Some(default_profile) = store.profiles.iter().find(|p| p.is_default) {
        new_content.push_str(&user_section(default_profile));
        new_content.push('\n');

        if let Some(ref key_path) = default_profile.ssh_key_path {
            new_content.push_str(&format!(
                "[core]\n\tsshCommand = {}\n\n",
                config_value(&ssh_command_for(key_path))
            ));
        }

        // Intentionally NO push-block here: a block in the main config would
        // apply to every folder, and a folder profile could only undo it with a
        // longer rewrite per URL form. profiles.rs enforces that the default
        // profile always allows pushes.
    }

    for profile in &store.profiles {
        if !profile.directories.is_empty() || !profile.remote_patterns.is_empty() {
            write_profile_gitconfig(profile)?;
        }
    }
    new_content.push_str(&build_includeif_blocks(&store.profiles));

    new_content.push_str("# <<< GitSwitch managed (DO NOT EDIT) <<<\n");

    fs::write(&gitconfig_path, new_content)?;

    // Keep ~/.ssh/allowed_signers in step with the profiles that sign, so local
    // signature verification works without any manual bookkeeping.
    let _ = crate::signing::write_allowed_signers(&store.profiles);

    // Remove per-profile config files that no longer belong to a live profile
    // (e.g. after a profile is deleted), so the app-managed dir never collects orphans.
    cleanup_orphan_gitconfigs(&store);

    Ok(())
}

fn cleanup_orphan_gitconfigs(store: &crate::profiles::ProfileStore) {
    let Some(data_dir) = dirs::data_dir() else { return };
    let config_dir = data_dir.join("com.gitswitch.app").join("gitconfigs");
    let Ok(entries) = fs::read_dir(&config_dir) else { return };

    let live: Vec<String> = store
        .profiles
        .iter()
        .map(|p| format!("{}.gitconfig", p.id))
        .collect();

    for entry in entries.flatten() {
        let path = entry.path();
        let keep = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| live.iter().any(|l| l == n));
        if !keep {
            let _ = fs::remove_file(&path);
        }
    }
}

fn remove_gitswitch_sections(content: &str) -> String {
    let mut result = String::new();
    let mut in_managed_section = false;

    for line in content.lines() {
        if line.contains(">>> GitSwitch managed") {
            in_managed_section = true;
            continue;
        }
        if line.contains("<<< GitSwitch managed") {
            in_managed_section = false;
            continue;
        }
        if !in_managed_section {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

pub fn get_current_git_config() -> Result<String, AppError> {
    let gitconfig_path = get_gitconfig_path();
    if gitconfig_path.exists() {
        Ok(fs::read_to_string(&gitconfig_path)?)
    } else {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_managed_section_and_keeps_user_content() {
        let content = "\
[user]\n\tname = mine\n\n\
# >>> GitSwitch managed (DO NOT EDIT) >>>\n\
[user]\n\tname = managed\n\
# <<< GitSwitch managed (DO NOT EDIT) <<<\n\
[alias]\n\tco = checkout";
        let cleaned = remove_gitswitch_sections(content);
        assert!(cleaned.contains("name = mine"));
        assert!(cleaned.contains("co = checkout"));
        assert!(!cleaned.contains("managed"));
        assert!(!cleaned.contains(">>>"));
    }

    #[test]
    fn no_managed_section_is_a_noop() {
        let content = "[user]\n\tname = mine";
        assert_eq!(remove_gitswitch_sections(content), content);
    }

    fn mk_profile(id: &str, dirs: &[&str]) -> crate::profiles::Profile {
        use chrono::Utc;
        crate::profiles::Profile {
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
        }
    }

    #[test]
    fn includeif_blocks_append_trailing_slash_for_recursive_match() {
        let p = mk_profile("test-id", &["/a/work", "/b/personal/"]);
        let block = build_includeif_blocks(&[p]);
        assert!(block.contains("gitdir:/a/work/"));
        assert!(block.contains("gitdir:/b/personal/"));
        assert!(!block.contains("personal//"));
    }

    #[test]
    fn nested_dir_is_emitted_after_its_parent_so_it_wins() {
        // Profile order intentionally puts the SPECIFIC dir's owner first —
        // sorting must still emit the parent include before the nested one,
        // because git's last-match-wins means "later include overrides".
        let child_owner = mk_profile("work", &["/dev/oss-repo"]);
        let parent_owner = mk_profile("personal", &["/dev"]);
        let block = build_includeif_blocks(&[child_owner, parent_owner]);
        let parent_pos = block.find("gitdir:/dev/\"").expect("parent include missing");
        let child_pos = block.find("gitdir:/dev/oss-repo/").expect("child include missing");
        assert!(
            parent_pos < child_pos,
            "nested (more specific) include must come last so it wins"
        );
    }

    #[test]
    fn config_values_never_contain_backslashes() {
        // Regression guard: a Windows path written verbatim into a git config
        // value makes git fail with "bad config line" and renders the whole
        // ~/.gitconfig unreadable. Everything we emit must be forward-slashed.
        let mut p = mk_profile("win", &[r"C:\Users\me\Work"]);
        p.ssh_key_path = Some(r"C:\Users\me\.ssh\id_key".into());
        p.signing_enabled = true;

        let includes = build_includeif_blocks(&[p.clone()]);
        assert!(!includes.contains('\\'), "includeIf block: {includes}");
        assert!(includes.contains("gitdir:C:/Users/me/Work/"));

        let ssh_line = format!("sshCommand = {}", config_value(&ssh_command_for(p.ssh_key_path.as_ref().unwrap())));
        assert!(!ssh_line.contains('\\'), "{ssh_line}");
        assert!(ssh_line.contains("ssh -i C:/Users/me/.ssh/id_key -o"), "{ssh_line}");

        let signing = crate::signing::signing_config_block(&p).unwrap();
        assert!(!signing.contains('\\'), "signing block: {signing}");
    }

    /// Ask git itself: write a profile file with every character that is
    /// special somewhere in git's config syntax, read it back with
    /// `git config -f`, and expect the original strings.
    fn git_reads(file: &std::path::Path, key: &str) -> Option<String> {
        let out = crate::spawn::command("git").args(["config", "-f"]).arg(file).args(["--get", key]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
    }

    #[test]
    fn what_git_reads_back_is_exactly_what_the_profile_said() {
        if crate::spawn::command("git").arg("--version").output().is_err() {
            return; // no git here: nothing to prove against
        }
        let mut p = mk_profile("quoted", &["/tmp/w"]);
        p.git_name = r#"Ann #1 "The Hammer" O'Brien "#.into();
        p.git_email = "ann;tag@example.test".into();
        p.ssh_key_path = Some("/Users/me/My Keys/id_ed25519".into());
        p.signing_enabled = true;
        let dir = std::env::temp_dir().join(format!("gitswitch-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.gitconfig");
        std::fs::write(&file, profile_gitconfig_content(&p)).unwrap();

        assert_eq!(git_reads(&file, "user.name").as_deref(), Some(p.git_name.as_str()));
        assert_eq!(git_reads(&file, "user.email").as_deref(), Some(p.git_email.as_str()));
        assert_eq!(
            git_reads(&file, "core.sshCommand").as_deref(),
            Some("ssh -i '/Users/me/My Keys/id_ed25519' -o IdentitiesOnly=yes")
        );
        assert_eq!(git_reads(&file, "user.signingkey").as_deref(), Some("/Users/me/My Keys/id_ed25519.pub"));

        // The include block: a folder with a quote and a space in its name.
        let inc = build_includeif_blocks(&[mk_profile("inc", &[r#"/Users/me/Work "2024""#])]);
        std::fs::write(&file, &inc).unwrap();
        let listed = crate::spawn::command("git").args(["config", "-f"]).arg(&file).arg("--list").output().unwrap();
        assert!(listed.status.success(), "git could not read the include block:\n{}\n{}", inc, String::from_utf8_lossy(&listed.stderr));
        let text = String::from_utf8_lossy(&listed.stdout);
        assert!(text.contains(r#"includeif.gitdir:/Users/me/Work "2024"/.path="#), "{}", text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_remote_rule_covers_every_spelling_github_hands_out_in_either_case() {
        let globs = remote_globs("acme/");
        assert_eq!(globs.len(), 4, "{globs:?}");
        assert!(globs.contains(&"https://github.com/[Aa][Cc][Mm][Ee]/**".to_string()), "{globs:?}");
        assert!(globs.contains(&"git@github.com:[Aa][Cc][Mm][Ee]/**".to_string()));
        assert!(globs.contains(&"ssh://git@github.com/[Aa][Cc][Mm][Ee]/**".to_string()));
        let one = remote_globs("acme/thing");
        assert_eq!(one.len(), 8, "{one:?}");
        assert!(one.contains(&"https://github.com/[Aa][Cc][Mm][Ee]/[Tt][Hh][Ii][Nn][Gg].git".to_string()));
        assert!(one.contains(&"git@github.com:[Aa][Cc][Mm][Ee]/[Tt][Hh][Ii][Nn][Gg]".to_string()));
        // Digits and punctuation are left alone; a full glob is passed through.
        assert!(remote_globs("a-1.b/").iter().all(|g| g.contains("[Aa]-1.[Bb]/**")));
        assert_eq!(remote_globs("https://gitlab.example.com/team/**"), vec!["https://gitlab.example.com/team/**"]);
    }

    #[test]
    fn remote_rules_come_before_folder_rules_so_a_folder_wins_where_both_match() {
        let mut remote = mk_profile("remote", &[]);
        remote.remote_patterns = vec!["acme/".into()];
        let folder = mk_profile("folder", &["/dev/work"]);
        let block = build_includeif_blocks(&[folder, remote]);
        let r = block.find("hasconfig:remote.*.url:").expect("remote block");
        let d = block.find("gitdir:/dev/work/").expect("folder block");
        assert!(r < d, "remote rules must be emitted first:\n{block}");
        // And git can read the whole thing.
        if std::process::Command::new("git").arg("--version").output().is_ok() {
            let dir = std::env::temp_dir().join(format!("gitswitch-remote-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("inc.gitconfig");
            std::fs::write(&file, &block).unwrap();
            let listed = std::process::Command::new("git").args(["config", "-f"]).arg(&file).arg("--list").output().unwrap();
            assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
            assert!(String::from_utf8_lossy(&listed.stdout).contains("includeif.hasconfig:remote.*.url:https://github.com/[Aa][Cc][Mm][Ee]/**.path="), "{}", String::from_utf8_lossy(&listed.stdout));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn push_block_rewrites_only_push_urls() {
        let s = push_block_section();
        // Must rewrite pushes for every common GitHub remote style…
        assert!(s.contains("pushInsteadOf = git@github.com:"));
        assert!(s.contains("pushInsteadOf = git@github.com/"));
        assert!(s.contains("pushInsteadOf = https://github.com/"));
        assert!(s.contains("pushInsteadOf = http://github.com/"));
        assert!(s.contains("pushInsteadOf = ssh://git@github.com/"));
        assert!(s.contains("pushInsteadOf = ssh://git@github.com:22/"));
        assert!(s.contains("pushInsteadOf = ssh://git@ssh.github.com:443/"));
        // …but never fetches (a plain insteadOf would break pull/clone).
        assert!(!s.contains("\tinsteadOf"));
    }
}
