use crate::{
    errors::{AppError, AppResult},
    github::{get_copilot_token, get_device_code, get_github_user, poll_access_token},
    paths::Paths,
    state::{AppState, RuntimeState},
};
use tokio::fs;

pub async fn read_github_token(paths: &Paths) -> AppResult<Option<String>> {
    let content = fs::read_to_string(&paths.github_token_path).await.unwrap_or_default();
    let token = content.trim().to_string();
    if token.is_empty() {
        Ok(None)
    } else {
        Ok(Some(token))
    }
}

pub async fn write_github_token(paths: &Paths, token: &str) -> AppResult<()> {
    fs::write(&paths.github_token_path, token).await
        .map_err(|e| AppError::Internal(format!("Failed to write token: {}", e)))
}

pub async fn setup_github_token(state: &AppState, force: bool) -> AppResult<()> {
    let paths = &state.paths;
    let existing = read_github_token(paths).await?;

    if let Some(token) = existing {
        if !force {
            let mut inner = state.inner.write().await;
            inner.github_token = Some(token.clone());
            drop(inner);
            let user = get_github_user(&state.http, &state.inner.read().await)?;
            tracing::info!("Logged in as {}", user.login);
            return Ok(());
        }
    }

    let device = get_device_code(&state.http).await?;
    tracing::info!(
        "Enter code {} at {}",
        device.user_code,
        device.verification_uri
    );

    let token = poll_access_token(&state.http, &device).await?;
    write_github_token(paths, &token).await?;

    let mut inner = state.inner.write().await;
    inner.github_token = Some(token.clone());
    drop(inner);

    let user = get_github_user(&state.http, &state.inner.read().await)?;
    tracing::info!("Logged in as {}", user.login);
    Ok(())
}

pub async fn setup_copilot_token(state: &AppState) -> AppResult<()> {
    let token_resp = {
        let inner = state.inner.read().await;
        get_copilot_token(&state.http, &inner).await?
    };

    {
        let mut inner = state.inner.write().await;
        inner.copilot_token = Some(token_resp.token.clone());
    }

    let refresh_ms = (token_resp.refresh_in.saturating_sub(60)) * 1000;
    let app = state.clone_for_task();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(refresh_ms)).await;
            let token_resp = {
                let inner = app.inner.read().await;
                get_copilot_token(&app.http, &inner).await
            };
            if let Ok(token_resp) = token_resp {
                let mut inner = app.inner.write().await;
                inner.copilot_token = Some(token_resp.token);
                tracing::info!("Copilot token refreshed");
            }
        }
    });

    Ok(())
}

impl AppState {
    pub fn clone_for_task(&self) -> Self {
        Self {
            http: self.http.clone(),
            inner: tokio::sync::RwLock::new(self.inner.blocking_read().clone()),
            paths: self.paths.clone(),
        }
    }
}
