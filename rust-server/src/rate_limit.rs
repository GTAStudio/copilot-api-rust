use crate::{errors::{ApiError, ApiResult}, state::AppState};

pub async fn check_rate_limit(state: &AppState) -> ApiResult<()> {
    let mut config = state.config.write().await;

    let limit = match config.rate_limit_seconds {
        Some(v) => v,
        None => return Ok(()),
    };

    let now = std::time::Instant::now();

    if let Some(last) = config.last_request_timestamp {
        let elapsed = now.duration_since(last).as_secs_f64();
        if elapsed < limit as f64 {
            let wait_secs = (limit as f64 - elapsed).ceil() as u64;
            if !config.rate_limit_wait {
                return Err(ApiError::BadRequest(format!(
                    "Rate limit exceeded. Wait {wait_secs} seconds.",
                )));
            }
            drop(config);
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
            let mut config = state.config.write().await;
            config.last_request_timestamp = Some(std::time::Instant::now());
            return Ok(());
        }
    }

    config.last_request_timestamp = Some(now);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_rate_limit;
    use crate::state::{AppConfig, AppState};

    #[tokio::test]
    async fn rate_limit_blocks_when_wait_false() {
        let config = AppConfig {
            rate_limit_seconds: Some(10),
            rate_limit_wait: false,
            last_request_timestamp: Some(std::time::Instant::now()),
            ..AppConfig::default()
        };

        let state = AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client: reqwest::Client::new(),
            hooks: None,
        };

        let result = check_rate_limit(&state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rate_limit_allows_when_unset() {
        let config = AppConfig {
            rate_limit_seconds: None,
            ..AppConfig::default()
        };

        let state = AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client: reqwest::Client::new(),
            hooks: None,
        };

        let result = check_rate_limit(&state).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rate_limit_allows_after_window() {
        let config = AppConfig {
            rate_limit_seconds: Some(1),
            rate_limit_wait: false,
            last_request_timestamp: Some(std::time::Instant::now() - std::time::Duration::from_secs(2)),
            ..AppConfig::default()
        };

        let state = AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client: reqwest::Client::new(),
            hooks: None,
        };

        let result = check_rate_limit(&state).await;
        assert!(result.is_ok());
    }
}
