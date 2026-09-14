use std::path::PathBuf;

use crate::errors::{ApiError, ApiResult};

pub struct AppPaths {
    pub app_dir: PathBuf,
    pub github_token_path: PathBuf,
}

pub fn get_paths() -> ApiResult<AppPaths> {
    let app_dir = match std::env::var_os("COPILOT_DATA_DIR") {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(ApiError::BadRequest(
                    "COPILOT_DATA_DIR must be an absolute directory path".to_string(),
                ));
            }
            path
        }
        None => directories::BaseDirs::new()
            .ok_or_else(|| ApiError::Internal("Failed to resolve data directory".to_string()))?
            .data_local_dir()
            .join("copilot-api"),
    };
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

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&paths.app_dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| {
                ApiError::Internal("Failed to protect credential directory".to_string())
            })?;
    }

    Ok(paths)
}
