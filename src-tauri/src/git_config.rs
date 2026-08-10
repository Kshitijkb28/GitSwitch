use crate::error::AppError;
use crate::profiles::{load_profiles, Profile};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;

fn get_gitconfig_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gitconfig")
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
fn prune_backups(path: &PathBuf) {
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
    let mut entries: Vec<(String, String)> = Vec::new();
    for profile in profiles {
        for dir in &profile.directories {
            // Trailing slash makes the gitdir match recursive over every repo inside.
            let dir_path = if dir.ends_with('/') {
                dir.clone()
            } else {
                format!("{}/", dir)
            };
            entries.push((dir_path, get_profile_gitconfig_path(profile)));
        }
    }
    entries.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));

    let mut blocks = String::new();
    for (dir, path) in entries {
        blocks.push_str(&format!(
            "[includeIf \"gitdir:{}\"]\n\tpath = {}\n\n",
            dir, path
        ));
    }
    blocks
}

fn get_profile_gitconfig_path(profile: &Profile) -> String {
    let data_dir = dirs::data_dir().unwrap_or_default();
    let app_dir = data_dir.join("com.gitswitch.app").join("gitconfigs");
    format!("{}/{}.gitconfig", app_dir.display(), profile.id)
}

/// Config block that makes `git push` fail locally with a recognizable error
/// ("remote helper 'gitswitch-push-blocked' aborted") while leaving fetch/pull
/// untouched — pushes get their URL rewritten to a scheme git can't handle.
///
/// pushInsteadOf is a literal prefix match, so cover every common github.com
/// remote form. Known limits (documented in the UI): custom ssh-config host
/// aliases and remotes with an explicit remote.<name>.pushurl are not caught.
/// This section must only ever be written into per-folder include files, never
/// the main ~/.gitconfig — rewrite rules ACCUMULATE across includes (they are
/// not last-match-wins), so a global block could never be overridden.
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

fn write_profile_gitconfig(profile: &Profile) -> Result<(), AppError> {
    let data_dir = dirs::data_dir().ok_or(AppError::Config("Cannot find data dir".into()))?;
    let config_dir = data_dir.join("com.gitswitch.app").join("gitconfigs");
    fs::create_dir_all(&config_dir)?;

    let path = config_dir.join(format!("{}.gitconfig", profile.id));

    let mut content = format!("[user]\n\tname = {}\n\temail = {}\n", profile.git_name, profile.git_email);

    if let Some(ref key_path) = profile.ssh_key_path {
        content.push_str(&format!("[core]\n\tsshCommand = ssh -i {} -o IdentitiesOnly=yes\n", key_path));
    }

    if !profile.allow_push {
        content.push_str(push_block_section());
    }

    fs::write(&path, content)?;
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
        new_content.push_str(&format!("[user]\n\tname = {}\n\temail = {}\n\n", default_profile.git_name, default_profile.git_email));

        if let Some(ref key_path) = default_profile.ssh_key_path {
            new_content.push_str(&format!("[core]\n\tsshCommand = ssh -i {} -o IdentitiesOnly=yes\n\n", key_path));
        }

        // Intentionally NO push-block here: url rewrite rules accumulate across
        // includes, so a block in the main config could never be overridden by
        // folder profiles that allow pushes. profiles.rs enforces that the
        // default profile always allows pushes.
    }

    for profile in &store.profiles {
        if !profile.directories.is_empty() {
            write_profile_gitconfig(profile)?;
        }
    }
    new_content.push_str(&build_includeif_blocks(&store.profiles));

    new_content.push_str("# <<< GitSwitch managed (DO NOT EDIT) <<<\n");

    fs::write(&gitconfig_path, new_content)?;

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
