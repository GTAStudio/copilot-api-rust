use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::errors::{ApiError, ApiResult};

const TREE_URL: &str = "https://api.github.com/repos/affaan-m/everything-claude-code/git/trees/main?recursive=1";
const RAW_BASE: &str = "https://raw.githubusercontent.com/affaan-m/everything-claude-code/main/";

#[derive(Debug, Deserialize)]
struct TreeResponse {
    tree: Vec<TreeItem>,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct TreeItem {
    path: String,
    #[serde(rename = "type")]
    item_type: String,
}

pub async fn sync_skills() -> ApiResult<()> {
    let client = reqwest::Client::builder()
        .user_agent("copilot-api-rs")
        .build()
        .map_err(|e| ApiError::Internal(format!("Failed to build client: {e}")))?;

    let tree = client
        .get(TREE_URL)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch skills tree: {e}")))?
        .json::<TreeResponse>()
        .await
        .map_err(|e| ApiError::Internal(format!("Invalid tree response: {e}")))?;

    if tree.truncated {
        return Err(ApiError::Internal("Git tree is truncated; cannot sync skills".to_string()));
    }

    let target_root = resolve_project_skills_dir()?;
    tokio::fs::create_dir_all(&target_root)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create skills dir: {e}")))?;
    let _ = ensure_notice_file(&target_root);

    for item in tree.tree {
        if item.item_type != "blob" {
            continue;
        }
        if !item.path.starts_with("skills/") {
            continue;
        }
        let rel = item.path.trim_start_matches("skills/");
        let target = target_root.join(rel);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to create dir: {e}")))?;
        }
        let url = format!("{}{}", RAW_BASE, item.path);
        let bytes = client
            .get(url)
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to download skill: {e}")))?
            .bytes()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to read skill bytes: {e}")))?;
        tokio::fs::write(&target, bytes)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to write skill file: {e}")))?;
    }

    Ok(())
}

fn resolve_project_skills_dir() -> ApiResult<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|e| ApiError::Internal(format!("Failed to read cwd: {e}")))?;
    let mut current = cwd.as_path();
    let mut last_found: Option<PathBuf> = None;
    for _ in 0..8 {
        let candidate = current.join(".claude");
        if candidate.exists() {
            last_found = Some(candidate);
        }
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }
    if let Some(found) = last_found {
        return Ok(found.join("skills"));
    }
    Ok(cwd.join(".claude").join("skills"))
}

#[allow(dead_code)]
fn ensure_notice_file(root: &Path) -> ApiResult<()> {
    let notice = root.join("THIRD_PARTY_NOTICES.txt");
    if notice.exists() {
        return Ok(());
    }
    let content = "MIT License\n\nCopyright (c) 2026 Affaan Mustafa\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in\nall copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n";
    std::fs::write(notice, content)
        .map_err(|e| ApiError::Internal(format!("Failed to write notice: {e}")))?;
    Ok(())
}
