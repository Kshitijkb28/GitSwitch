use crate::error::AppError;

/// List the GitHub accounts the local `gh` CLI is authenticated with.
pub async fn gh_list_accounts() -> Result<Vec<String>, AppError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await
        .map_err(|e| {
            AppError::Command(format!(
                "Failed to run gh (is the GitHub CLI installed?): {}",
                e
            ))
        })?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Lines look like: "  ✓ Logged in to github.com account <name> (keyring)"
    let mut accounts = Vec::new();
    for line in combined.lines() {
        if let Some(idx) = line.find("account ") {
            let rest = &line[idx + "account ".len()..];
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if !name.is_empty() && !accounts.contains(&name) {
                accounts.push(name);
            }
        }
    }

    if accounts.is_empty() {
        return Err(AppError::Command(
            "No gh accounts found. Run `gh auth login` first.".into(),
        ));
    }

    Ok(accounts)
}

/// Log a specific account out of the local `gh` CLI (removes its stored token).
pub async fn gh_logout(account: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "logout", "--user", account])
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Command(format!(
            "gh logout failed for {}: {}",
            account, stderr
        )));
    }
    Ok(format!("Removed {} from the GitHub CLI", account))
}

/// Get an OAuth token for a specific gh account without switching the active one.
pub async fn gh_get_token(account: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "token", "--user", account])
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Command(format!(
            "gh auth token failed for {}: {}",
            account, stderr
        )));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AppError::Command("gh returned an empty token".into()));
    }
    Ok(token)
}
