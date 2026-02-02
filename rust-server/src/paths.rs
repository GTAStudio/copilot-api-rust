use std::path::PathBuf;

use crate::errors::{ApiError, ApiResult};

pub struct AppPaths {
    pub app_dir: PathBuf,
    pub github_token_path: PathBuf,
}

pub fn get_paths() -> ApiResult<AppPaths> {
    let base_dirs = directories::BaseDirs::new()
        .ok_or_else(|| ApiError::Internal("Failed to resolve data directory".to_string()))?;
    let base = base_dirs.data_local_dir();

    let app_dir = base.join("copilot-api");
    let github_token_path = app_dir.join("github_token");

    Ok(AppPaths {
        app_dir,
        github_token_path,
    })
}

pub async fn ensure_paths() -> ApiResult<AppPaths> {
    let paths = get_paths()?;
    tokio::fs::create_dir_all(&paths.app_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create app dir: {e}")))?;

    if !paths.github_token_path.exists() {
        tokio::fs::write(&paths.github_token_path, "")
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to create token file: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(&paths.github_token_path, std::fs::Permissions::from_mode(0o600)).await;
        }
    }

    Ok(paths)
}
