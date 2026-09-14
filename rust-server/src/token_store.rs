use crate::{
    errors::{ApiError, ApiResult},
    paths::ensure_paths,
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubCredential {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

pub async fn read_github_token() -> ApiResult<Option<String>> {
    Ok(read_github_credential()
        .await?
        .map(|credential| credential.access_token))
}

pub async fn read_github_credential() -> ApiResult<Option<GitHubCredential>> {
    let paths = ensure_paths().await?;
    let content = match tokio::fs::read_to_string(paths.github_token_path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ApiError::Internal("Failed to read credentials".to_string())),
    };
    parse_credential(&content)
}

fn parse_credential(content: &str) -> ApiResult<Option<GitHubCredential>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else if trimmed.starts_with('{') {
        let credential: GitHubCredential = serde_json::from_str(trimmed)
            .map_err(|_| ApiError::Internal("Invalid stored credentials".to_string()))?;
        if credential.access_token.trim().is_empty() {
            return Err(ApiError::Internal("Invalid stored credentials".to_string()));
        }
        Ok(Some(credential))
    } else {
        Ok(Some(GitHubCredential {
            access_token: trimmed.to_string(),
            refresh_token: None,
            expires_at: None,
        }))
    }
}

pub async fn write_github_credential(credential: &GitHubCredential) -> ApiResult<()> {
    if credential.access_token.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Cannot store an empty credential".to_string(),
        ));
    }
    let paths = ensure_paths().await?;
    let temporary = paths
        .app_dir
        .join(format!("github_token-{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(credential)
        .map_err(|_| ApiError::Internal("Failed to serialize credentials".to_string()))?;
    let result = async {
        use tokio::io::AsyncWriteExt;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &paths.github_token_path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result.map_err(|_| ApiError::Internal("Failed to store credentials".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_storage_roundtrip_uses_only_the_configured_directory() {
        let directory =
            tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .expect("isolated directory");
        for scenario in ["roundtrip", "relative-path", "blocked-directory"] {
            let root = directory.path().join(scenario);
            let selected = if scenario == "relative-path" {
                std::path::PathBuf::from("relative-credential-directory")
            } else {
                root.clone()
            };
            if scenario == "blocked-directory" {
                std::fs::write(&root, "existing-data").expect("blocking fixture");
            }
            let output =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "token_store::tests::credential_storage_fixture",
                        "--nocapture",
                    ])
                    .env("COPILOT_DATA_DIR", &selected)
                    .env("COPILOT_STORAGE_FIXTURE", scenario)
                    .env_remove("COPILOT_GITHUB_TOKEN")
                    .output()
                    .expect("storage fixture");
            assert!(
                output.status.success(),
                "scenario={scenario}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("CREDENTIAL_STORAGE_FIXTURE_PASSED")
            );
        }
    }

    #[tokio::test]
    async fn credential_storage_fixture() {
        let Ok(scenario) = std::env::var("COPILOT_STORAGE_FIXTURE") else {
            return;
        };
        let expected = std::path::PathBuf::from(
            std::env::var_os("COPILOT_DATA_DIR").expect("isolated data directory"),
        );
        if scenario == "relative-path" {
            assert!(crate::paths::get_paths().is_err());
        } else {
            let paths = crate::paths::get_paths().expect("paths");
            assert_eq!(
                paths.app_dir, expected,
                "test must never use the real credential directory"
            );
            if scenario == "blocked-directory" {
                assert!(read_github_credential().await.is_err());
                assert_eq!(
                    std::fs::read_to_string(&expected).expect("preserved file"),
                    "existing-data"
                );
            } else {
                assert!(
                    read_github_credential()
                        .await
                        .expect("empty store")
                        .is_none()
                );
                let credential = GitHubCredential {
                    access_token: "unit-test-token-one".to_string(),
                    refresh_token: Some("unit-test-refresh".to_string()),
                    expires_at: Some(123456),
                };
                write_github_credential(&credential)
                    .await
                    .expect("initial write");
                let stored = read_github_credential()
                    .await
                    .expect("read")
                    .expect("credential");
                assert_eq!(stored.access_token, credential.access_token);
                assert_eq!(stored.refresh_token, credential.refresh_token);
                assert_eq!(stored.expires_at, credential.expires_at);
                let replacement = GitHubCredential {
                    access_token: "unit-test-token-two".to_string(),
                    refresh_token: None,
                    expires_at: None,
                };
                write_github_credential(&replacement)
                    .await
                    .expect("atomic replacement");
                assert_eq!(
                    read_github_token().await.expect("updated token").as_deref(),
                    Some("unit-test-token-two")
                );
                assert_eq!(std::fs::read_dir(&expected).expect("directory").count(), 1);
                std::fs::write(&paths.github_token_path, "unit-test-legacy\n")
                    .expect("legacy fixture");
                assert_eq!(
                    read_github_token().await.expect("legacy read").as_deref(),
                    Some("unit-test-legacy")
                );
                std::fs::write(&paths.github_token_path, "{broken").expect("damaged fixture");
                assert!(read_github_credential().await.is_err());
                std::fs::remove_file(&paths.github_token_path).expect("remove owned fixture");
                std::fs::create_dir(&paths.github_token_path).expect("blocking destination");
                assert!(write_github_credential(&replacement).await.is_err());
                assert_eq!(
                    std::fs::read_dir(&expected)
                        .expect("no temporary residue")
                        .count(),
                    1
                );
                assert!(paths.github_token_path.is_dir());
            }
        }
        println!("CREDENTIAL_STORAGE_FIXTURE_PASSED");
    }

    #[test]
    fn reads_legacy_and_expiring_credentials() {
        assert!(parse_credential(" \n").expect("empty").is_none());
        assert_eq!(
            parse_credential("unit-test-legacy\n")
                .expect("legacy")
                .expect("credential")
                .access_token,
            "unit-test-legacy"
        );
        let credential = parse_credential(r#"{"access_token":"unit-test-access","refresh_token":"unit-test-refresh","expires_at":4600}"#).expect("json").expect("credential");
        assert_eq!(credential.expires_at, Some(4600));
        assert!(parse_credential("{broken").is_err());
        assert!(parse_credential(r#"{"access_token":""}"#).is_err());
    }
}
