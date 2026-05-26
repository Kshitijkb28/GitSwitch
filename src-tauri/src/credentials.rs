use crate::error::AppError;
use keyring::Entry;

const SERVICE_NAME: &str = "com.gitswitch.app";

pub fn store_token(profile_id: &str, token: &str) -> Result<(), AppError> {
    let entry = Entry::new(SERVICE_NAME, profile_id)
        .map_err(|e| AppError::Keyring(format!("Failed to create keyring entry: {}", e)))?;
    entry
        .set_password(token)
        .map_err(|e| AppError::Keyring(format!("Failed to store token: {}", e)))?;
    Ok(())
}

pub fn get_token(profile_id: &str) -> Result<Option<String>, AppError> {
    let entry = Entry::new(SERVICE_NAME, profile_id)
        .map_err(|e| AppError::Keyring(format!("Failed to create keyring entry: {}", e)))?;

    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Keyring(format!("Failed to retrieve token: {}", e))),
    }
}

pub fn delete_token(profile_id: &str) -> Result<(), AppError> {
    let entry = Entry::new(SERVICE_NAME, profile_id)
        .map_err(|e| AppError::Keyring(format!("Failed to create keyring entry: {}", e)))?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Keyring(format!("Failed to delete token: {}", e))),
    }
}
