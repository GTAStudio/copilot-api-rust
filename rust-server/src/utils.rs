pub async fn get_vscode_version(client: &reqwest::Client) -> String {
    const FALLBACK: &str = "1.104.3";
    if let Ok(version) = std::env::var("COPILOT_EDITOR_VERSION")
        && valid_version(&version)
    {
        return version;
    }
    if let Ok(response) = client
        .get("https://update.code.visualstudio.com/api/releases/stable")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        && let Ok(versions) = response.json::<Vec<String>>().await
        && let Some(version) = versions.into_iter().find(|version| valid_version(version))
    {
        return version;
    }
    FALLBACK.to_string()
}

pub fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
}

pub fn required_api_key(name: &str) -> crate::errors::ApiResult<String> {
    let key = std::env::var(name)
        .map_err(|_| crate::errors::ApiError::BadRequest(format!("Missing {name}")))?;
    if key.is_empty() || key.len() > 8192 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(crate::errors::ApiError::BadRequest(format!(
            "Invalid {name}"
        )));
    }
    Ok(key)
}

pub fn http_client() -> crate::errors::ApiResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("copilot-api-rs/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(300))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(20)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if std::env::var("COPILOT_DISABLE_PROXY").as_deref() != Ok("1") {
        for name in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
            let value = std::env::var(name).or_else(|_| std::env::var(name.to_ascii_lowercase()));
            if let Ok(value) = value {
                if value.trim().is_empty() {
                    continue;
                }
                let proxy = match name {
                    "HTTP_PROXY" => reqwest::Proxy::http(value),
                    "HTTPS_PROXY" => reqwest::Proxy::https(value),
                    _ => reqwest::Proxy::all(value),
                }
                .map_err(|_| {
                    crate::errors::ApiError::BadRequest("Invalid proxy configuration".to_string())
                })?;
                builder = builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_env()));
            }
        }
    }
    builder.build().map_err(|_| {
        crate::errors::ApiError::Internal("Failed to initialize HTTP client".to_string())
    })
}

pub fn api_url(base: &str, endpoint: &str) -> crate::errors::ApiResult<url::Url> {
    let invalid =
        || crate::errors::ApiError::BadRequest("Invalid upstream API base URL".to_string());
    let mut url = url::Url::parse(base).map_err(|_| invalid())?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let base_path = url.path().trim_end_matches('/');
    let versioned = if base_path.ends_with("/v1") {
        base_path.to_string()
    } else {
        format!("{base_path}/v1")
    };
    url.set_path(&format!("{versioned}/{endpoint}"));
    Ok(url)
}

pub fn estimate_tokens_from_json(value: &serde_json::Value) -> u64 {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    ((serialized.len() as f64) / 4.0).ceil() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_urls_are_normalized_and_reject_credential_leaks() {
        for base in [
            "https://api.example.com",
            "https://api.example.com/",
            "https://api.example.com/v1",
            "https://api.example.com/v1/",
        ] {
            assert_eq!(
                api_url(base, "messages").expect("URL").as_str(),
                "https://api.example.com/v1/messages"
            );
        }
        assert!(api_url("http://127.0.0.1:4141", "messages").is_ok());
        for base in [
            "http://remote.example",
            "https://user:password@api.example.com",
            "https://api.example.com?secret=value",
            "file:///tmp/secret",
        ] {
            assert!(api_url(base, "messages").is_err(), "{base}");
        }
    }

    #[test]
    fn version_header_values_are_bounded() {
        for value in ["1.115.0", "0.39.0"] {
            assert!(valid_version(value));
        }
        for value in ["", "1.0\r\nInjected: yes", "not-a-version"] {
            assert!(!valid_version(value));
        }
    }
}
