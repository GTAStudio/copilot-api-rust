use crate::{
    errors::{ApiError, ApiResult},
    services::github::get_copilot_token,
    state::AppState,
    token_store::{GitHubCredential, read_github_credential, write_github_credential},
};

pub async fn ensure_github_token(state: &AppState) -> ApiResult<String> {
    let refresh_lock = state.config.read().await.github_refresh_lock.clone();
    let _refresh = refresh_lock.lock().await;
    let config = state.config.read().await.clone();
    let credential = if let Some(token) = config.github_token.clone() {
        Some(GitHubCredential {
            access_token: token,
            refresh_token: config.github_refresh_token.clone(),
            expires_at: config.github_token_expires_at,
        })
    } else {
        read_github_credential().await?
    };
    let mut credential = credential.ok_or_else(|| {
        ApiError::Unauthorized("GitHub token not found. Run device auth first.".to_string())
    })?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if credential
        .expires_at
        .is_some_and(|expires| expires <= now.saturating_add(60))
    {
        let refresh_token = credential.refresh_token.as_deref().ok_or_else(|| {
            ApiError::Unauthorized(
                "GitHub authorization expired. Run device auth again.".to_string(),
            )
        })?;
        credential =
            crate::services::github::refresh_access_token(&state.client, &config, refresh_token)
                .await?;
        write_github_credential(&credential).await?;
    }
    let mut config = state.config.write().await;
    config.github_token = Some(credential.access_token.clone());
    config.github_refresh_token = credential.refresh_token;
    config.github_token_expires_at = credential.expires_at;
    Ok(credential.access_token)
}

pub async fn ensure_copilot_token(state: &AppState) -> ApiResult<String> {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    };
    let config = state.config.read().await;
    if let Some(token) = cached_token(&config, now()) {
        return Ok(token);
    }
    let refresh_lock = config.token_refresh_lock.clone();
    drop(config);
    let _refresh = refresh_lock.lock().await;
    if let Some(token) = cached_token(&*state.config.read().await, now()) {
        return Ok(token);
    }
    let github_token = ensure_github_token(state).await?;
    let config_snapshot = state.config.read().await.clone();
    let response = get_copilot_token(&state.client, &config_snapshot, &github_token).await?;
    if response.token.trim().is_empty() || response.expires_at <= now() {
        return Err(ApiError::Upstream(
            "Received an empty or expired Copilot token".to_string(),
        ));
    }
    let endpoint = response
        .endpoints
        .and_then(|endpoints| endpoints.api)
        .map(|endpoint| validated_copilot_endpoint(&endpoint))
        .transpose()?;
    {
        let mut config = state.config.write().await;
        config.copilot_token = Some(response.token.clone());
        config.copilot_token_expires_at = Some(response.expires_at);
        if config.copilot_base_url != endpoint {
            config.models = None;
        }
        config.copilot_base_url = endpoint;
    }
    tracing::debug!(expires_at = response.expires_at, "Copilot token refreshed");
    Ok(response.token)
}

fn cached_token(config: &crate::state::AppConfig, now: u64) -> Option<String> {
    config
        .copilot_token_expires_at
        .filter(|expires| *expires > now.saturating_add(60))?;
    config
        .copilot_token
        .clone()
        .filter(|token| !token.trim().is_empty())
}

fn validated_copilot_endpoint(endpoint: &str) -> ApiResult<String> {
    let invalid = || ApiError::Upstream("Invalid Copilot API endpoint".to_string());
    let url = url::Url::parse(endpoint).map_err(|_| invalid())?;
    let trusted_host = url.host_str().is_some_and(|host| {
        host == "api.githubcopilot.com" || host.ends_with(".githubcopilot.com")
    });
    if url.scheme() != "https"
        || !trusted_host
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cache_requires_expiration_and_refreshes_early() {
        let mut config = crate::state::AppConfig {
            copilot_token: Some("unit-test-token".to_string()),
            ..Default::default()
        };
        assert!(cached_token(&config, 1000).is_none());
        for expires in [0, 1000, 1020, 1060] {
            config.copilot_token_expires_at = Some(expires);
            assert!(cached_token(&config, 1000).is_none());
        }
        config.copilot_token_expires_at = Some(1061);
        assert_eq!(
            cached_token(&config, 1000).as_deref(),
            Some("unit-test-token")
        );
    }

    #[test]
    fn token_endpoint_only_trusts_copilot_https_origins() {
        for endpoint in [
            "https://api.githubcopilot.com",
            "https://api.enterprise.githubcopilot.com/",
            "https://api.individual.githubcopilot.com",
        ] {
            assert!(validated_copilot_endpoint(endpoint).is_ok());
        }
        for endpoint in [
            "http://api.githubcopilot.com",
            "https://api.githubcopilot.com.evil.example",
            "https://githubcopilot.com@evil.example",
            "https://api.githubcopilot.com:444",
            "https://api.githubcopilot.com/path",
            "https://api.githubcopilot.com?token=value",
            "https://127.0.0.1",
        ] {
            assert!(validated_copilot_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }
}
