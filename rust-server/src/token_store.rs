use crate::{errors::{ApiError, ApiResult}, paths::ensure_paths};

pub async fn read_github_token() -> ApiResult<Option<String>> {
    let paths = ensure_paths().await?;
    let content = tokio::fs::read_to_string(paths.github_token_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to read token: {e}")))?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

pub async fn write_github_token(token: &str) -> ApiResult<()> {
    let paths = ensure_paths().await?;
    tokio::fs::write(paths.github_token_path, token)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to write token: {e}")))?;
    Ok(())
}
