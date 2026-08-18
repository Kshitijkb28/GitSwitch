use crate::error::AppError;
use crate::profiles::Profile;
use std::fs;
use std::path::PathBuf;

const BEGIN: &str = "# >>> GitSwitch managed signers (DO NOT EDIT) >>>";
const END: &str = "# <<< GitSwitch managed signers (DO NOT EDIT) <<<";

pub fn allowed_signers_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".ssh")
        .join("allowed_signers")
}

/// The public key of an SSH keypair, as `<type> <base64>` with the trailing
/// comment stripped — that is the exact shape an allowed_signers entry wants.
pub fn pubkey_material(private_key_path: &str) -> Result<String, AppError> {
    let pub_path = format!("{}.pub", private_key_path);
    let raw = fs::read_to_string(&pub_path).map_err(|_| {
        AppError::Ssh(format!(
            "Public key not found at {} — signing needs the .pub half of the key.",
            pub_path
        ))
    })?;
    let mut parts = raw.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(t), Some(k)) => Ok(format!("{} {}", t, k)),
        _ => Err(AppError::Ssh(format!("{} is not a valid public key", pub_path))),
    }
}

/// Strip the block GitSwitch owns, leaving any hand-written entries intact.
fn strip_managed(content: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in content.lines() {
        if line.trim() == BEGIN {
            inside = true;
            continue;
        }
        if line.trim() == END {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Build the managed block: one `email <type> <key>` line per signing profile.
/// Git needs this file to verify signatures locally (`git log --show-signature`);
/// getting the email↔key pairing right by hand is the step people trip on.
pub fn build_allowed_signers(profiles: &[Profile], existing: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for p in profiles.iter().filter(|p| p.signing_enabled) {
        let Some(key) = p.ssh_key_path.as_ref() else { continue };
        let Ok(material) = pubkey_material(key) else { continue };
        let entry = format!("{} {}", p.git_email.trim(), material);
        if !lines.contains(&entry) {
            lines.push(entry);
        }
    }

    let mut out = strip_managed(existing);
    if !out.is_empty() {
        out.push('\n');
    }
    if lines.is_empty() {
        return out.trim_end().to_string();
    }
    out.push_str(BEGIN);
    out.push('\n');
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// Rewrite ~/.ssh/allowed_signers. Returns how many signers are managed.
pub fn write_allowed_signers(profiles: &[Profile]) -> Result<usize, AppError> {
    let path = allowed_signers_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let content = build_allowed_signers(profiles, &existing);
    let count = profiles
        .iter()
        .filter(|p| p.signing_enabled && p.ssh_key_path.is_some())
        .count();
    fs::write(&path, content)?;
    Ok(count)
}

/// The gitconfig stanza that turns on SSH commit signing for one profile.
/// `user.signingkey` points at the PUBLIC key — that's what gpg.format=ssh wants.
pub fn signing_config_block(profile: &Profile) -> Option<String> {
    if !profile.signing_enabled {
        return None;
    }
    let key = profile.ssh_key_path.as_ref()?;
    Some(format!(
        "[gpg]\n\tformat = ssh\n[gpg \"ssh\"]\n\tallowedSignersFile = {}\n[user]\n\tsigningkey = {}.pub\n[commit]\n\tgpgsign = true\n[tag]\n\tgpgsign = true\n",
        allowed_signers_path().display(),
        key
    ))
}

/// Register a key on GitHub as a SIGNING key. This is a different key type from
/// an authentication key — a key registered only for auth will NOT produce the
/// "Verified" badge.
pub async fn register_signing_key(
    token: &str,
    title: &str,
    public_key: &str,
) -> Result<String, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Command(format!("Failed to build HTTP client: {}", e)))?;

    let resp = client
        .post("https://api.github.com/user/ssh_signing_keys")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .json(&serde_json::json!({ "title": title, "key": public_key }))
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();

    match status {
        200..=299 => Ok("Signing key registered on GitHub".into()),
        _ if body.contains("already in use") => {
            Ok("That key is already registered as a signing key".into())
        }
        // GitHub answers 404 (not 403) when the token lacks the signing-key scope.
        403 | 404 => Err(AppError::Command(
            "Your gh token can't manage signing keys. Grant the scope once with:\n\
             gh auth refresh -h github.com -s write:ssh_signing_key\n\
             then try again."
                .into(),
        )),
        _ => Err(AppError::Command(format!(
            "GitHub returned {}: {}",
            status, body
        ))),
    }
}

/// Is this key already registered as a signing key on the account?
pub async fn signing_key_registered(token: &str, public_key: &str) -> Result<bool, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Command(format!("Failed to build HTTP client: {}", e)))?;

    let resp = client
        .get("https://api.github.com/user/ssh_signing_keys")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "GitSwitch/0.1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::Command(format!("HTTP request failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Command(
            "Couldn't list signing keys (the gh token likely lacks the write:ssh_signing_key scope)."
                .into(),
        ));
    }
    let keys: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| AppError::Command(format!("Failed to parse response: {}", e)))?;

    // Compare only <type> <base64>; titles and comments differ freely.
    let want = public_key.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    Ok(keys.iter().any(|k| {
        k.get("key")
            .and_then(|v| v.as_str())
            .map(|s| s.split_whitespace().take(2).collect::<Vec<_>>().join(" ") == want)
            .unwrap_or(false)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn prof(email: &str, key: Option<&str>, signing: bool) -> Profile {
        Profile {
            id: email.into(),
            name: email.into(),
            git_name: "n".into(),
            git_email: email.into(),
            ssh_key_path: key.map(|s| s.to_string()),
            is_default: false,
            allow_push: true,
            signing_enabled: signing,
            directories: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn signing_block_only_for_enabled_profiles_with_a_key() {
        assert!(signing_config_block(&prof("a@b", Some("/k"), false)).is_none());
        assert!(signing_config_block(&prof("a@b", None, true)).is_none());
        let block = signing_config_block(&prof("a@b", Some("/k"), true)).unwrap();
        assert!(block.contains("format = ssh"));
        // SSH signing points signingkey at the PUBLIC half.
        assert!(block.contains("signingkey = /k.pub"));
        assert!(block.contains("gpgsign = true"));
        assert!(block.contains("allowedSignersFile ="));
    }

    #[test]
    fn allowed_signers_preserves_handwritten_entries() {
        let existing = "someone@else.com ssh-ed25519 AAAAKEEPME\n";
        let out = build_allowed_signers(&[], existing);
        assert!(out.contains("someone@else.com ssh-ed25519 AAAAKEEPME"));
        assert!(!out.contains(BEGIN), "no managed block when nothing signs");
    }

    #[test]
    fn managed_block_is_replaced_not_appended() {
        let stale = format!("keep@me.com ssh-ed25519 AAAAKEEP\n{}\nold@x.com ssh-ed25519 AAAAOLD\n{}\n", BEGIN, END);
        let out = build_allowed_signers(&[], &stale);
        assert!(out.contains("keep@me.com"));
        assert!(!out.contains("old@x.com"), "stale managed entry must be dropped");
        assert_eq!(out.matches(BEGIN).count(), 0);
    }

    #[test]
    fn pubkey_material_strips_the_comment() {
        let dir = std::env::temp_dir().join(format!("gs_sign_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("id_test");
        std::fs::write(
            format!("{}.pub", key.display()),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI user@host\n",
        )
        .unwrap();
        assert_eq!(
            pubkey_material(&key.to_string_lossy()).unwrap(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
