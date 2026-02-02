pub async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

pub async fn get_vscode_version() -> String {
    const FALLBACK: &str = "1.104.3";

    let client = reqwest::Client::new();
    let request = client
        .get("https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h=visual-studio-code-bin")
        .timeout(std::time::Duration::from_secs(5));

    match request.send().await {
        Ok(resp) => match resp.text().await {
            Ok(body) => {
                let re = regex::Regex::new(r"pkgver=([0-9.]+)").ok();
                if let Some(re) = re {
                    if let Some(caps) = re.captures(&body) {
                        if let Some(m) = caps.get(1) {
                            return m.as_str().to_string();
                        }
                    }
                }
                FALLBACK.to_string()
            }
            Err(_) => FALLBACK.to_string(),
        },
        Err(_) => FALLBACK.to_string(),
    }
}

pub fn estimate_tokens_from_json(value: &serde_json::Value) -> u64 {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    ((serialized.len() as f64) / 4.0).ceil() as u64
}

// intentionally left without env helpers to keep runtime dependency surface minimal
