use crate::error::AppError;
use std::fs;
use std::path::PathBuf;

fn get_ssh_dir() -> Result<PathBuf, AppError> {
    let home = dirs::home_dir().ok_or(AppError::Ssh("Cannot find home directory".into()))?;
    let ssh_dir = home.join(".ssh");
    fs::create_dir_all(&ssh_dir)?;
    Ok(ssh_dir)
}

pub fn generate_ssh_key(profile_name: &str, email: &str) -> Result<(String, String), AppError> {
    let ssh_dir = get_ssh_dir()?;
    let sanitized_name = profile_name.to_lowercase().replace(' ', "_");
    let key_name = format!("id_ed25519_gitswitch_{}", sanitized_name);
    let key_path = ssh_dir.join(&key_name);

    if key_path.exists() {
        return Err(AppError::Ssh(format!(
            "Key already exists at {}",
            key_path.display()
        )));
    }

    let output = crate::spawn::command("ssh-keygen")
        .args([
            "-t", "ed25519",
            "-C", email,
            "-f", &key_path.to_string_lossy(),
            "-N", "",
        ])
        .output()
        .map_err(|e| AppError::Command(format!("Failed to run ssh-keygen: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Ssh(format!("ssh-keygen failed: {}", stderr)));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
    }

    let pub_key_path = key_path.with_extension("pub");
    let public_key = fs::read_to_string(&pub_key_path)?;

    Ok((key_path.to_string_lossy().to_string(), public_key.trim().to_string()))
}

pub fn get_public_key(private_key_path: &str) -> Result<String, AppError> {
    let pub_path = format!("{}.pub", private_key_path);
    let pub_path = PathBuf::from(&pub_path);

    if pub_path.exists() {
        Ok(fs::read_to_string(&pub_path)?.trim().to_string())
    } else {
        Err(AppError::Ssh(format!("Public key not found at {}", pub_path.display())))
    }
}

pub async fn test_ssh_connection() -> Result<String, AppError> {
    let output = crate::spawn::async_command("ssh")
        .args(["-T", "git@github.com", "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10"])
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run ssh: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let combined = format!("{}{}", stdout, stderr);

    if combined.contains("successfully authenticated") {
        Ok(combined)
    } else if combined.contains("Permission denied") {
        Err(AppError::Ssh("Permission denied - check your SSH key".into()))
    } else {
        Err(unreachable(&combined))
    }
}

/// ssh neither authenticated nor was refused: the host was not reached. Its
/// own last line (a DNS failure, a timeout, a proxy error) is the useful part.
fn unreachable(output: &str) -> AppError {
    let said = output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ssh printed nothing").trim();
    AppError::Ssh(format!("Could not reach github.com over SSH: {}", said))
}

pub async fn test_ssh_connection_with_key(key_path: &str) -> Result<String, AppError> {
    let output = crate::spawn::async_command("ssh")
        .args([
            "-T", "git@github.com",
            "-i", key_path,
            "-o", "IdentitiesOnly=yes",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ConnectTimeout=10",
        ])
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run ssh: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);

    if combined.contains("successfully authenticated") {
        Ok(combined)
    } else if combined.contains("Permission denied") {
        Err(AppError::Ssh(format!("Permission denied for key {}", key_path)))
    } else {
        Err(unreachable(&combined))
    }
}

/// This computer's name, as people know it: `Ann-PC`, not `Ann-PC.local`.
pub fn computer_name() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length, and gethostname writes
        // a NUL-terminated name into it (truncated if it does not fit).
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).to_string();
        Some(name).filter(|n| !n.trim().is_empty())
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").ok().filter(|n| !n.trim().is_empty())
    }
}

/// The title a key gets on GitHub: which computer it came from.
pub fn key_title(computer: Option<&str>) -> String {
    match computer.map(|c| c.trim().trim_end_matches(".local")).filter(|c| !c.is_empty()) {
        Some(c) => format!("GitSwitch ({})", c),
        None => "GitSwitch".into(),
    }
}

/// Is this private key protected by a passphrase?
/// Asking ssh-keygen for the public half with an empty passphrase succeeds only
/// when the key is unencrypted. `-P ""` guarantees it can never prompt.
/// Only a key ssh-keygen says needs a passphrase counts: one it cannot read
/// for another reason (permissions, a format it does not know) is not
/// "passphrase-protected", and must not be reported as such.
pub fn key_has_passphrase(key_path: &str) -> bool {
    crate::spawn::command("ssh-keygen")
        .args(["-y", "-P", "", "-f", key_path])
        .output()
        .map(|o| !o.status.success() && String::from_utf8_lossy(&o.stderr).to_lowercase().contains("passphrase"))
        .unwrap_or(false)
}

/// Whether ssh-agent holds this key: `ssh-add -L` lists its public half.
/// None when that cannot be told (no agent, no .pub, ssh-add missing).
pub async fn agent_holds_key(key_path: &str) -> Option<bool> {
    let material = crate::signing::pubkey_material(key_path).ok()?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::spawn::async_command("ssh-add").arg("-L").output(),
    )
    .await
    .ok()?
    .ok()?;
    match out.status.code() {
        Some(0) => Some(String::from_utf8_lossy(&out.stdout).lines().any(|l| {
            let mut w = l.split_whitespace();
            matches!((w.next(), w.next()), (Some(t), Some(k)) if format!("{} {}", t, k) == material)
        })),
        // "The agent has no identities."
        Some(1) => Some(false),
        _ => None,
    }
}

/// github.com refused a key that needs a passphrase, which the probe could
/// not give it (it runs with BatchMode and no ssh config, so the keychain the
/// config names cannot unlock it): that says nothing about the key's account.
pub(crate) fn rejected_because_locked(has_passphrase: bool, in_agent: Option<bool>) -> bool {
    has_passphrase && in_agent != Some(true)
}

async fn probe_key_account(key_path: &str) -> Result<Option<String>, AppError> {
    // Bypass the user's ssh config with the platform's null device, so we test
    // the key itself rather than whatever identity the config forces.
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = crate::spawn::async_command("ssh")
        .args([
            "-F", null_config,
            "-i", key_path,
            "-o", "IdentitiesOnly=yes",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            "-T", "git@github.com",
        ])
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run ssh: {}", e)))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    interpret_ssh_probe(&combined).ok_or_else(|| {
        AppError::Ssh(format!(
            "Could not determine the key's GitHub account: {}",
            combined
                .trim()
                .lines()
                .next()
                .unwrap_or("no response from github.com")
        ))
    })
}

/// Interpret `ssh -T git@github.com` output.
/// `Some(Some(login))` authenticated · `Some(None)` definitively rejected ·
/// `None` indeterminate (timeout, throttling, DNS…) — must NOT be read as
/// "not registered", or a transient blip condemns a working key.
pub(crate) fn interpret_ssh_probe(combined: &str) -> Option<Option<String>> {
    if let Some(idx) = combined.find("Hi ") {
        let rest = &combined[idx + 3..];
        if let Some(end) = rest.find('!') {
            return Some(Some(rest[..end].trim().to_string()));
        }
    }
    if combined.contains("Permission denied") {
        return Some(None);
    }
    None
}

/// Which GitHub account does this key actually authenticate as?
/// `Ok(Some(login))` = authenticated, `Ok(None)` = definitively rejected,
/// `Err` = couldn't tell (transient failure — never treat as "not registered").
pub async fn resolve_key_account(key_path: &str) -> Result<Option<String>, AppError> {
    match probe_key_account(key_path).await {
        Err(_) => {
            // One retry: a single dropped/throttled connection shouldn't surface
            // as a scary diagnostic.
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            probe_key_account(key_path).await
        }
        result => result,
    }
}

pub fn delete_ssh_key(key_path: &str) -> Result<(), AppError> {
    let ssh_dir = get_ssh_dir()?;
    let path = PathBuf::from(key_path);

    // Safety: only allow deleting keys that live directly inside ~/.ssh
    if path.parent() != Some(ssh_dir.as_path()) {
        return Err(AppError::Ssh(
            "Refusing to delete a key outside of ~/.ssh".into(),
        ));
    }

    if path.exists() {
        fs::remove_file(&path)?;
    }
    let pub_path = PathBuf::from(format!("{}.pub", key_path));
    if pub_path.exists() {
        fs::remove_file(&pub_path)?;
    }
    Ok(())
}

pub fn list_ssh_keys() -> Result<Vec<String>, AppError> {
    let ssh_dir = get_ssh_dir()?;
    let mut keys = Vec::new();

    if let Ok(entries) = fs::read_dir(&ssh_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(".pub") {
                continue;
            }
            // Skip well-known non-key files.
            if matches!(
                name,
                "config" | "known_hosts" | "known_hosts.old" | "authorized_keys" | "environment"
            ) || name.contains("backup")
            {
                continue;
            }
            // A private key: conventional id_* name, or any file with a .pub sibling.
            let has_pub_sibling = ssh_dir.join(format!("{}.pub", name)).exists();
            if name.starts_with("id_") || has_pub_sibling {
                keys.push(path.to_string_lossy().to_string());
            }
        }
    }

    keys.sort();
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_greeting_yields_login() {
        let out = "Hi octo-work! You've successfully authenticated, but GitHub does not provide shell access.\n";
        assert_eq!(
            interpret_ssh_probe(out),
            Some(Some("octo-work".to_string()))
        );
    }

    #[test]
    fn permission_denied_is_definitively_unregistered() {
        let out = "git@github.com: Permission denied (publickey).\n";
        assert_eq!(interpret_ssh_probe(out), Some(None));
    }

    #[test]
    fn transient_failures_are_indeterminate_not_unregistered() {
        // These must NOT be reported as "key isn't on any GitHub account".
        for out in [
            "ssh: connect to host github.com port 22: Operation timed out\n",
            "ssh: Could not resolve hostname github.com: nodename nor servname provided\n",
            "kex_exchange_identification: Connection closed by remote host\n",
            "",
        ] {
            assert_eq!(interpret_ssh_probe(out), None, "should be indeterminate: {out:?}");
        }
    }
}

#[cfg(test)]
mod locked_key_tests {
    use super::*;

    #[test]
    fn a_locked_key_is_not_called_unregistered() {
        assert!(rejected_because_locked(true, Some(false)));
        assert!(rejected_because_locked(true, None));
        assert!(!rejected_because_locked(true, Some(true)));
        assert!(!rejected_because_locked(false, None));
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn a_key_is_titled_after_the_computer_it_came_from() {
        assert_eq!(key_title(Some("Ann-PC.local")), "GitSwitch (Ann-PC)");
        assert_eq!(key_title(Some("build-box")), "GitSwitch (build-box)");
        assert_eq!(key_title(None), "GitSwitch");
        assert_eq!(key_title(Some("  ")), "GitSwitch");
    }
}
