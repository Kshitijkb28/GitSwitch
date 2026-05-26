use crate::error::AppError;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

    let output = Command::new("ssh-keygen")
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

pub fn test_ssh_connection() -> Result<String, AppError> {
    let output = Command::new("ssh")
        .args(["-T", "git@github.com", "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10"])
        .output()
        .map_err(|e| AppError::Command(format!("Failed to run ssh: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let combined = format!("{}{}", stdout, stderr);

    if combined.contains("successfully authenticated") {
        Ok(combined)
    } else if combined.contains("Permission denied") {
        Err(AppError::Ssh("Permission denied - check your SSH key".into()))
    } else {
        Ok(combined)
    }
}

pub fn test_ssh_connection_with_key(key_path: &str) -> Result<String, AppError> {
    let output = Command::new("ssh")
        .args([
            "-T", "git@github.com",
            "-i", key_path,
            "-o", "IdentitiesOnly=yes",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ConnectTimeout=10",
        ])
        .output()
        .map_err(|e| AppError::Command(format!("Failed to run ssh: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);

    if combined.contains("successfully authenticated") {
        Ok(combined)
    } else if combined.contains("Permission denied") {
        Err(AppError::Ssh(format!("Permission denied for key {}", key_path)))
    } else {
        Ok(combined)
    }
}

pub fn list_ssh_keys() -> Result<Vec<String>, AppError> {
    let ssh_dir = get_ssh_dir()?;
    let mut keys = Vec::new();

    if let Ok(entries) = fs::read_dir(&ssh_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("id_") && !name.ends_with(".pub") {
                    keys.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    keys.sort();
    Ok(keys)
}
