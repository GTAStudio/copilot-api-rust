use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header, uri::Authority},
    middleware::Next,
    response::Response,
};
use std::net::{IpAddr, SocketAddr};
use subtle::ConstantTimeEq;

use crate::{
    errors::{ApiError, ApiResult},
    state::AppState,
};

pub fn validate_bind_address(address: &str, api_key: Option<&str>) -> ApiResult<SocketAddr> {
    if let Some(key) = api_key
        && (!(16..=512).contains(&key.len()) || !key.bytes().all(|byte| byte.is_ascii_graphic()))
    {
        return Err(ApiError::BadRequest(
            "COPILOT_API_KEY must contain 16 to 512 printable ASCII characters without spaces"
                .to_string(),
        ));
    }
    let address = address
        .strip_prefix("localhost:")
        .map(|port| format!("127.0.0.1:{port}"))
        .unwrap_or_else(|| address.to_string());
    let address: SocketAddr = address
        .parse()
        .map_err(|_| ApiError::BadRequest("Invalid listening IP address or port".to_string()))?;
    if !address.ip().is_loopback() && api_key.is_none() {
        return Err(ApiError::BadRequest(
            "COPILOT_API_KEY is required for non-loopback listening addresses".to_string(),
        ));
    }
    Ok(address)
}

fn authenticated(headers: &HeaderMap, expected: &str) -> bool {
    let mut supplied = false;
    for name in [header::AUTHORIZATION.as_str(), "x-api-key"] {
        let values = headers.get_all(name);
        if values.iter().count() > 1 {
            return false;
        }
        if let Some(value) = values.iter().next() {
            let Ok(value) = value.to_str() else {
                return false;
            };
            let candidate = if name == "authorization" {
                let Some((scheme, token)) = value.split_once(' ') else {
                    return false;
                };
                if !scheme.eq_ignore_ascii_case("bearer") {
                    return false;
                }
                token
            } else {
                value
            };
            if !bool::from(expected.as_bytes().ct_eq(candidate.as_bytes())) {
                return false;
            }
            supplied = true;
        }
    }
    supplied
}

pub async fn authorize(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    if request.headers().contains_key(header::ORIGIN)
        || request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return Err(ApiError::Forbidden(
            "Browser-origin requests are disabled".to_string(),
        ));
    }
    let api_key = state.config.read().await.api_key.clone();
    if let Some(key) = api_key {
        if !authenticated(request.headers(), &key) {
            return Err(ApiError::Unauthorized(
                "A valid local API key is required".to_string(),
            ));
        }
    } else {
        let authority = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<Authority>().ok())
            .or_else(|| request.uri().authority().cloned());
        let local = authority.is_some_and(|authority| {
            let host = authority.host().trim_matches(['[', ']']);
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if !local {
            return Err(ApiError::Forbidden(
                "A loopback Host header is required without an API key".to_string(),
            ));
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_loopback_and_authenticated_remote_bindings() {
        for address in ["127.0.0.1:4141", "[::1]:4141", "localhost:4141"] {
            assert!(validate_bind_address(address, None).is_ok());
        }
        for address in ["0.0.0.0:4141", "[::]:4141", "192.0.2.1:4141"] {
            assert!(validate_bind_address(address, None).is_err());
            assert!(validate_bind_address(address, Some("unit-test-local-api-key")).is_ok());
        }
        for key in ["", "short", "unit-test key with spaces"] {
            assert!(validate_bind_address("127.0.0.1:4141", Some(key)).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_and_conflicting_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_static("unit-test-local-api-key"),
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer invalid"),
        );
        assert!(!authenticated(&headers, "unit-test-local-api-key"));
        headers.remove(header::AUTHORIZATION);
        headers.append(
            "x-api-key",
            HeaderValue::from_static("unit-test-local-api-key"),
        );
        assert!(!authenticated(&headers, "unit-test-local-api-key"));
    }
}
