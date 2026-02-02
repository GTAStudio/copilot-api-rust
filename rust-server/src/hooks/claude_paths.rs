use std::path::PathBuf;

use crate::errors::{ApiError, ApiResult};

pub fn claude_root_dir() -> ApiResult<PathBuf> {
    let base_dirs = directories::BaseDirs::new()
        .ok_or_else(|| ApiError::Internal("Failed to resolve home directory".to_string()))?;
    Ok(base_dirs.home_dir().join(".claude"))
}

pub fn sessions_dir() -> ApiResult<PathBuf> {
    Ok(claude_root_dir()?.join("sessions"))
}

pub fn learned_skills_dir() -> ApiResult<PathBuf> {
    Ok(claude_root_dir()?.join("skills").join("learned"))
}

pub fn hooks_dir() -> ApiResult<PathBuf> {
    Ok(claude_root_dir()?.join("hooks"))
}

pub fn observations_file() -> ApiResult<PathBuf> {
    Ok(claude_root_dir()?.join("observations.jsonl"))
}
