use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::errors::{ApiError, ApiResult};

const REPOSITORY_API: &str = "https://api.github.com/repos/affaan-m/everything-claude-code";
const RAW_BASE: &str = "https://raw.githubusercontent.com/affaan-m/everything-claude-code";

#[derive(Debug, Deserialize)]
struct TreeResponse {
    tree: Vec<TreeItem>,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct TreeItem {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    item_type: String,
}

pub async fn sync_skills() -> ApiResult<()> {
    let client = crate::utils::http_client()?;
    let root = resolve_project_skills_dir()?;
    sync_skills_from(&client, &root, REPOSITORY_API, RAW_BASE).await
}

async fn sync_skills_from(
    client: &reqwest::Client,
    target_root: &Path,
    repository_api: &str,
    raw_base: &str,
) -> ApiResult<()> {
    let commit = client
        .get(format!("{repository_api}/commits/main"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| ApiError::Upstream("Failed to resolve skills revision".to_string()))?;
    let commit: serde_json::Value = crate::errors::check_upstream(commit)
        .await?
        .json()
        .await
        .map_err(|_| ApiError::Upstream("Invalid skills revision response".to_string()))?;
    let revision = commit
        .get("sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ApiError::Upstream("Invalid skills revision".to_string()))?;
    let response = client
        .get(format!("{repository_api}/git/trees/{revision}?recursive=1"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch skills tree: {e}")))?;
    let tree = crate::errors::check_upstream(response)
        .await?
        .json::<TreeResponse>()
        .await
        .map_err(|e| ApiError::Internal(format!("Invalid tree response: {e}")))?;

    if tree.truncated {
        return Err(ApiError::Internal(
            "Git tree is truncated; cannot sync skills".to_string(),
        ));
    }

    reject_symlink_ancestors(target_root)?;
    tokio::fs::create_dir_all(target_root)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create skills dir: {e}")))?;
    ensure_notice_file(target_root)?;

    for item in tree.tree {
        if item.item_type != "blob" || !matches!(item.mode.as_str(), "100644" | "100755") {
            continue;
        }
        if !item.path.starts_with("skills/") {
            continue;
        }
        let target = skill_target(target_root, &item.path)?;
        reject_symlink_ancestors(&target)?;
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to create dir: {e}")))?;
        }
        let mut url = url::Url::parse(raw_base)
            .map_err(|_| ApiError::Internal("Invalid skill source URL".to_string()))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::Internal("Invalid skill source URL".to_string()))?
            .pop_if_empty()
            .push(revision)
            .extend(item.path.split('/'));
        let response = client
            .get(url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to download skill: {e}")))?;
        let mut response = crate::errors::check_upstream(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ApiError::Upstream("Skill download failed".to_string()))?
        {
            if bytes.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
                return Err(ApiError::Upstream(
                    "Skill file exceeds size limit".to_string(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let temporary = target.with_extension(format!("{}.download", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, bytes)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to write skill file: {e}")))?;
        let installation = tokio::fs::hard_link(&temporary, &target).await;
        tokio::fs::remove_file(&temporary).await.map_err(|_| {
            ApiError::Internal("Failed to clean temporary skill download".to_string())
        })?;
        match installation {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(ApiError::Internal(
                    "Failed to install skill file without overwriting".to_string(),
                ));
            }
        }
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
            break;
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

fn skill_target(root: &Path, source: &str) -> ApiResult<PathBuf> {
    let invalid = || ApiError::BadRequest("Unsafe skill download path".to_string());
    let relative = source.strip_prefix("skills/").ok_or_else(invalid)?;
    for component in relative.split('/') {
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|character| character.is_control() || "\\:*?\"<>|".contains(character))
            || [
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9",
            ]
            .contains(&stem.as_str())
        {
            return Err(invalid());
        }
    }
    Ok(root.join(relative))
}

fn reject_symlink_ancestors(path: &Path) -> ApiResult<()> {
    for ancestor in path.ancestors() {
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
            let mut linked = metadata.file_type().is_symlink();
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                linked |= metadata.file_attributes() & 0x400 != 0;
            }
            if linked {
                return Err(ApiError::BadRequest(
                    "Skill installation refuses symlink or junction paths".to_string(),
                ));
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skills_sync_downloads_fixed_revision_and_never_overwrites_local_edits() {
        for scenario in [
            "download",
            "created-during-download",
            "truncated-tree",
            "bad-tree-path",
            "http-error",
        ] {
            let directory = tempfile::tempdir().expect("skill target directory");
            let root = directory.path().join("skills");
            tokio::fs::create_dir_all(root.join("existing"))
                .await
                .expect("existing skill directory");
            tokio::fs::write(root.join("existing/SKILL.md"), "local content")
                .await
                .expect("existing skill");
            let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
            let captured = requests.clone();
            let target = root.clone();
            let revision = "1111111111111111111111111111111111111111";
            let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
                let captured = captured.clone();
                let target = target.clone();
                async move {
                    use axum::response::IntoResponse;
                    let path = request.uri().path().to_string();
                    captured.lock().await.push(path.clone());
                    if scenario == "http-error" { return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(); }
                    if path == "/repo/commits/main" { return axum::Json(serde_json::json!({"sha": revision})).into_response(); }
                    if path == format!("/repo/git/trees/{revision}") {
                        return axum::Json(serde_json::json!({"truncated": scenario == "truncated-tree", "tree": [
                            {"path": "skills/existing/SKILL.md", "type": "blob", "mode": "100644"},
                            {"path": if scenario == "bad-tree-path" { "skills/../../escape" } else { "skills/new/SKILL.md" }, "type": "blob", "mode": "100644"},
                            {"path": "skills/link", "type": "blob", "mode": "120000"},
                            {"path": "README.md", "type": "blob", "mode": "100644"}
                        ]})).into_response();
                    }
                    assert_eq!(path, format!("/raw/{revision}/skills/new/SKILL.md"));
                    if scenario == "created-during-download" {
                        tokio::fs::write(target.join("new/SKILL.md"), "concurrent local edit").await.expect("concurrent edit");
                    }
                    "# Downloaded Fixture\n".into_response()
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("download fixture listener");
            let base = format!(
                "http://{}",
                listener.local_addr().expect("download fixture address")
            );
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("download fixture server");
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("fixture client");
            let result = sync_skills_from(
                &client,
                &root,
                &format!("{base}/repo"),
                &format!("{base}/raw"),
            )
            .await;
            task.abort();
            let _ = task.await;
            assert_eq!(
                tokio::fs::read_to_string(root.join("existing/SKILL.md"))
                    .await
                    .expect("preserved skill"),
                "local content"
            );
            if scenario == "download" {
                result.expect("download success");
                assert_eq!(
                    tokio::fs::read_to_string(root.join("new/SKILL.md"))
                        .await
                        .expect("downloaded skill"),
                    "# Downloaded Fixture\n"
                );
                assert!(root.join("THIRD_PARTY_NOTICES.txt").is_file());
                assert!(
                    requests
                        .lock()
                        .await
                        .iter()
                        .all(|path| !path.contains("existing/SKILL.md"))
                );
            } else if scenario == "created-during-download" {
                assert_eq!(
                    tokio::fs::read_to_string(root.join("new/SKILL.md"))
                        .await
                        .expect("concurrent local edit"),
                    "concurrent local edit"
                );
                assert_eq!(
                    tokio::fs::read_dir(root.join("new"))
                        .await
                        .expect("target entries")
                        .next_entry()
                        .await
                        .expect("entry")
                        .expect("existing file")
                        .file_name(),
                    "SKILL.md"
                );
            } else {
                assert!(result.is_err(), "{scenario}");
            }
            assert!(!directory.path().join("escape").exists());
        }
    }

    #[test]
    fn skill_download_paths_cannot_escape_project_root() {
        let root = Path::new("fixture-skills");
        assert_eq!(
            skill_target(root, "skills/example/SKILL.md").expect("safe path"),
            root.join("example").join("SKILL.md")
        );
        for path in [
            "skills/../../escape",
            "skills/..\\escape",
            "skills/C:/escape",
            "skills//absolute",
            "skills/example/CON",
            "skills/example/file.md:stream",
            "not-skills/example",
        ] {
            assert!(skill_target(root, path).is_err(), "{path}");
        }
    }
}
