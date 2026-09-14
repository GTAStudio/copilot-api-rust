use crate::{
    errors::{ApiError, ApiResult},
    state::AppState,
};

pub async fn check_rate_limit(state: &AppState) -> ApiResult<()> {
    let mut config = state.config.write().await;

    let limit = match config.rate_limit_seconds {
        Some(0) | None => return Ok(()),
        Some(value) if value <= 86_400 => value,
        Some(_) => {
            return Err(ApiError::BadRequest(
                "Rate limit interval must not exceed 86400 seconds".to_string(),
            ));
        }
    };

    let now = std::time::Instant::now();

    let slot = config
        .last_request_timestamp
        .and_then(|last| last.checked_add(std::time::Duration::from_secs(limit)))
        .unwrap_or(now)
        .max(now);
    let wait = slot.saturating_duration_since(now);
    if !wait.is_zero() && (!config.rate_limit_wait || wait > std::time::Duration::from_secs(300)) {
        return Err(ApiError::RateLimited(
            wait.as_secs()
                .saturating_add(u64::from(wait.subsec_nanos() > 0)),
        ));
    }
    config.last_request_timestamp = Some(slot);
    drop(config);
    if !wait.is_zero() {
        tokio::time::sleep_until(tokio::time::Instant::from_std(slot)).await;
    }
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
        assert_eq!(
            result.expect_err("rate limited").status_code(),
            axum::http::StatusCode::TOO_MANY_REQUESTS
        );
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
            last_request_timestamp: Some(
                std::time::Instant::now() - std::time::Duration::from_secs(2),
            ),
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
    async fn waiting_requests_reserve_distinct_slots() {
        let state = AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(AppConfig {
                rate_limit_seconds: Some(1),
                rate_limit_wait: true,
                last_request_timestamp: Some(std::time::Instant::now()),
                ..Default::default()
            })),
            client: reqwest::Client::new(),
            hooks: None,
        };
        let first_state = state.clone();
        let first = tokio::spawn(async move {
            check_rate_limit(&first_state).await.expect("first slot");
            std::time::Instant::now()
        });
        let second = tokio::spawn(async move {
            check_rate_limit(&state).await.expect("second slot");
            std::time::Instant::now()
        });
        let first = first.await.expect("first");
        let second = second.await.expect("second");
        let gap = first.max(second).duration_since(first.min(second));
        assert!(
            gap >= std::time::Duration::from_millis(900),
            "requests overlapped: {gap:?}"
        );
    }
}
