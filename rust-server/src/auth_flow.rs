use crate::{
    errors::{ApiError, ApiResult},
    services::github::{get_copilot_token, get_github_user},
    state::AppState,
    token_store::read_github_token,
};

pub async fn ensure_github_token(state: &AppState) -> ApiResult<String> {
    if let Some(token) = state.config.read().await.github_token.clone() {
        return Ok(token);
    }

    if let Some(token) = read_github_token().await? {
        let mut config = state.config.write().await;
        config.github_token = Some(token.clone());
        return Ok(token);
    }

    Err(ApiError::Unauthorized(
        "GitHub token not found. Run device auth first.".to_string(),
    ))
}

pub async fn ensure_copilot_token(state: &AppState) -> ApiResult<String> {
    if let Some(token) = state.config.read().await.copilot_token.clone() {
        return Ok(token);
    }

    let github_token = ensure_github_token(state).await?;
    let config_snapshot = state.config.read().await.clone();

    let response = get_copilot_token(&state.client, &config_snapshot, &github_token).await?;
    {
        let mut config = state.config.write().await;
        config.copilot_token = Some(response.token.clone());
    }

    if state.config.read().await.show_token {
        tracing::info!("Copilot token: {}", response.token);
    }

    schedule_copilot_refresh(state.clone(), response.refresh_in);

    // Best-effort log user
    let _ = get_github_user(&state.client, &config_snapshot, &github_token).await;

    Ok(response.token)
}

fn schedule_copilot_refresh(state: AppState, refresh_in: u64) {
    tokio::spawn(async move {
        let mut next_refresh = refresh_in;
        loop {
            let wait_secs = next_refresh.saturating_sub(60);
            if wait_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
            }

            let github_token = match ensure_github_token(&state).await {
                Ok(token) => token,
                Err(err) => {
                    tracing::warn!("Failed to refresh Copilot token (no GitHub token): {}", err);
                    continue;
                }
            };

            let config_snapshot = state.config.read().await.clone();
            match get_copilot_token(&state.client, &config_snapshot, &github_token).await {
                Ok(response) => {
                    next_refresh = response.refresh_in;
                    let mut config = state.config.write().await;
                    config.copilot_token = Some(response.token.clone());
                    if config.show_token {
                        tracing::info!("Refreshed Copilot token: {}", response.token);
                    }
                }
                Err(err) => {
                    tracing::warn!("Failed to refresh Copilot token: {}", err);
                    // Backoff a bit before retry
                    next_refresh = 300;
                }
            }
        }
    });
}
