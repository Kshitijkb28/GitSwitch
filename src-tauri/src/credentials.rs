//! GitHub tokens GitSwitch keeps itself — a "Sign in with GitHub" — in the
//! system's own credential store: the macOS Keychain, Windows Credential
//! Manager, the Secret Service on Linux. GitSwitch's files hold only which
//! logins are signed in, never a token.

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE_NAME: &str = "com.gitswitch.app";

/// The store's name for a sign-in. Logins ignore case, and so does this.
fn signin_key(login: &str) -> String {
    format!("github:{}", login.trim().to_ascii_lowercase())
}

/// Where tokens go: the system's store — or, for the verify suites, which
/// must never reach the person's real keychain, files in the folder
/// GITSWITCH_TOKEN_DIR names (debug builds only); in unit tests, a temporary
/// folder.
enum Store {
    System,
    Files(PathBuf),
}

fn store() -> Store {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("GITSWITCH_TOKEN_DIR").filter(|d| !d.is_empty()) {
        return Store::Files(PathBuf::from(dir));
    }
    if cfg!(test) {
        return Store::Files(std::env::temp_dir().join(format!("gitswitch-tokens-{}", std::process::id())));
    }
    Store::System
}

fn file_for(dir: &Path, key: &str) -> PathBuf {
    dir.join(key.replace(':', "_"))
}

fn refused(e: keyring::Error) -> AppError {
    AppError::Keyring(format!("The system's credential store refused it: {}", e))
}

/// Keep the token a sign-in gave, for that login.
pub fn keep_signin_token(login: &str, token: &str) -> Result<(), AppError> {
    let key = signin_key(login);
    match store() {
        Store::Files(dir) => {
            fs::create_dir_all(&dir)?;
            let path = file_for(&dir, &key);
            fs::write(&path, token)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        }
        Store::System => keyring::Entry::new(SERVICE_NAME, &key).and_then(|e| e.set_password(token)).map_err(refused),
    }
}

/// The token kept for a login, if one is.
pub fn signin_token(login: &str) -> Result<Option<String>, AppError> {
    let key = signin_key(login);
    match store() {
        Store::Files(dir) => match fs::read_to_string(file_for(&dir, &key)) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        },
        Store::System => match keyring::Entry::new(SERVICE_NAME, &key).and_then(|e| e.get_password()) {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(refused(e)),
        },
    }
}

/// Remove the token kept for a login; none kept is not an error.
pub fn forget_signin_token(login: &str) -> Result<(), AppError> {
    let key = signin_key(login);
    match store() {
        Store::Files(dir) => match fs::remove_file(file_for(&dir, &key)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        },
        Store::System => match keyring::Entry::new(SERVICE_NAME, &key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(refused(e)),
        },
    }
}

/// A login signed in here, as GitSwitch's own files remember it.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct KeptSignIn {
    pub login: String,
    pub id: u64,
}

/// The list of logins: beside the profiles, or beside the test tokens.
fn index_path() -> Option<PathBuf> {
    match store() {
        Store::Files(dir) => Some(dir.join("github-signins.json")),
        Store::System => dirs::data_dir().map(|d| d.join("com.gitswitch.app").join("github-signins.json")),
    }
}

/// Newest first. Reading it never reads a token: listing the accounts must
/// not make the system ask for the keychain.
pub fn kept_signins() -> Vec<KeptSignIn> {
    index_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Vec<KeptSignIn>>(&t).ok())
        .unwrap_or_default()
}

fn write_index(list: &[KeptSignIn]) -> Result<(), AppError> {
    let path = index_path().ok_or_else(|| AppError::Config("Cannot find the data folder".into()))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".into()))?;
    Ok(())
}

pub fn remember_signin(login: &str, id: u64) -> Result<(), AppError> {
    let mut list: Vec<KeptSignIn> = kept_signins().into_iter().filter(|s| !s.login.eq_ignore_ascii_case(login)).collect();
    list.insert(0, KeptSignIn { login: login.to_string(), id });
    write_index(&list)
}

pub fn forget_signin(login: &str) -> Result<(), AppError> {
    let list: Vec<KeptSignIn> = kept_signins().into_iter().filter(|s| !s.login.eq_ignore_ascii_case(login)).collect();
    write_index(&list)
}

/// "signin:<login>" → the login, for the account pickers' values.
pub fn signin_login(account: &str) -> Option<&str> {
    account.strip_prefix("signin:").filter(|l| !l.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_sign_in_is_found_by_its_login_in_any_case_and_forgotten() {
        keep_signin_token("Octo-Keep", "tok-1").unwrap();
        assert_eq!(signin_token("octo-keep").unwrap().as_deref(), Some("tok-1"));
        remember_signin("Octo-Keep", 7).unwrap();
        remember_signin("OCTO-KEEP", 7).unwrap();
        assert_eq!(kept_signins().iter().filter(|s| s.login.eq_ignore_ascii_case("octo-keep")).count(), 1);
        forget_signin_token("OCTO-keep").unwrap();
        forget_signin("octo-KEEP").unwrap();
        assert_eq!(signin_token("Octo-Keep").unwrap(), None);
        assert!(!kept_signins().iter().any(|s| s.login.eq_ignore_ascii_case("octo-keep")));
        // Forgetting what is not kept is not an error.
        forget_signin_token("nobody-kept").unwrap();
    }

    #[test]
    fn a_picker_value_names_its_login() {
        assert_eq!(signin_login("signin:alice"), Some("alice"));
        assert_eq!(signin_login("signin:"), None);
        assert_eq!(signin_login("alice"), None);
    }
}
