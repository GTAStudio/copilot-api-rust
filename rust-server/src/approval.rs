use crate::{errors::{ApiError, ApiResult}, state::AppState};
use dialoguer::Confirm;

pub async fn check_manual_approval(state: &AppState) -> ApiResult<()> {
    let config = state.config.read().await;
    if !config.manual_approve {
        return Ok(());
    }

    drop(config);

    let approved = Confirm::new()
        .with_prompt("Accept incoming request?")
        .default(false)
        .interact()
        .unwrap_or(false);

    if approved {
        Ok(())
    } else {
        Err(ApiError::Unauthorized("Request rejected".to_string()))
    }
}
