use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ApiError {
    #[error("Request body exceeds the size limit")]
    PayloadTooLarge,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("Rate limit exceeded. Retry after {0} seconds.")]
    RateLimited(u64),
    #[error("{message}")]
    Provider {
        status: StatusCode,
        error_type: &'static str,
        message: &'static str,
    },
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    r#type: &'static str,
    error: ErrorMessage,
}

#[derive(Debug, Serialize)]
struct ErrorMessage {
    r#type: &'static str,
    message: String,
}

impl ApiError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Provider { status, .. } => *status,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_type = match &self {
            Self::PayloadTooLarge => "request_too_large",
            Self::BadRequest(_) => "invalid_request_error",
            Self::Unauthorized(_) => "authentication_error",
            Self::Forbidden(_) => "permission_error",
            Self::RateLimited(_) => "rate_limit_error",
            Self::Provider { error_type, .. } => error_type,
            Self::NotFound(_) => "not_found_error",
            Self::Upstream(_) | Self::Internal(_) => "api_error",
        };
        let message = match &self {
            Self::Upstream(_) => "The upstream service could not complete the request.".to_string(),
            Self::Internal(_) => "An internal server error occurred.".to_string(),
            _ => self.to_string(),
        };
        let body = ErrorBody {
            r#type: "error",
            error: ErrorMessage {
                r#type: error_type,
                message,
            },
        };
        let mut response = (status, Json(body)).into_response();
        if let Self::RateLimited(seconds) = self
            && let Ok(value) = seconds.to_string().parse()
        {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, value);
        }
        response
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

pub async fn check_upstream(mut response: reqwest::Response) -> ApiResult<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|header| header.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1)
            .clamp(1, 86_400);
        return Err(ApiError::RateLimited(retry));
    }
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len().saturating_add(chunk.len()) > 65_536 {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    Err(provider_error(status, &error))
}

fn provider_error(status: StatusCode, error: &serde_json::Value) -> ApiError {
    let error_type = match status.as_u16() {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        529 => "overloaded_error",
        _ => "api_error",
    };
    let detail = error
        .pointer("/error/message")
        .or_else(|| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let message = if status == StatusCode::BAD_REQUEST
        && (detail.contains("prompt is too long")
            || detail.contains("prompt too long")
            || detail.contains("context_length_exceeded")
            || detail.contains("context window"))
    {
        "capability_rejected: prompt_too_long"
    } else if status == StatusCode::BAD_REQUEST
        && detail.contains("thinking")
        && detail.contains("signature")
    {
        "capability_rejected: thinking_signature"
    } else if status == StatusCode::BAD_REQUEST && detail.contains("thinking") {
        "capability_rejected: thinking"
    } else if status == StatusCode::BAD_REQUEST && detail.contains("cache_control") {
        "capability_rejected: cache_control"
    } else if status.is_client_error() {
        "The upstream service rejected the request. Check the model, credentials and request parameters."
    } else {
        "The upstream service is temporarily unavailable."
    };
    ApiError::Provider {
        status: if status.is_client_error() || status.is_server_error() {
            status
        } else {
            StatusCode::BAD_GATEWAY
        },
        error_type,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_and_internal_errors_do_not_expose_details() {
        for error in [
            ApiError::Upstream("sensitive-provider-diagnostic".to_string()),
            ApiError::Internal("sensitive-local-path".to_string()),
        ] {
            let response = error.into_response();
            assert!(response.status().is_server_error());
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .expect("body");
            let text = std::str::from_utf8(&body).expect("utf8");
            assert!(!text.contains("sensitive"));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["type"], "error");
            assert_eq!(json["error"]["type"], "api_error");
        }
    }

    #[tokio::test]
    async fn request_errors_include_protocol_error_types() {
        let response = ApiError::Unauthorized("API key required".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["error"]["type"], "authentication_error");
        assert_eq!(json["error"]["message"], "API key required");
    }
}
