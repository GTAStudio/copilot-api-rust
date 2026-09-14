use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use tower::ServiceExt;

use crate::state::{AppConfig, AppState};

const TEST_API_KEY: &str = "unit-test-local-access-key";

fn state(api_key: Option<&str>) -> AppState {
    AppState {
        config: std::sync::Arc::new(tokio::sync::RwLock::new(AppConfig {
            api_key: api_key.map(str::to_string),
            github_token: Some("unit-test-github-secret".to_string()),
            copilot_token: Some("unit-test-copilot-secret".to_string()),
            copilot_token_expires_at: Some(u64::MAX),
            ..AppConfig::default()
        })),
        client: reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client"),
        hooks: None,
    }
}

async fn request(
    state: AppState,
    path: &str,
    headers: &[(&str, &str)],
    json: Option<serde_json::Value>,
) -> Response {
    let mut builder = Request::builder().uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if let Some(json) = json {
        builder = builder
            .method("POST")
            .header("content-type", "application/json");
        Body::from(serde_json::to_vec(&json).expect("serialize request"))
    } else {
        Body::empty()
    };
    crate::build_router(state)
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response")
}

fn token_count_payload() -> serde_json::Value {
    serde_json::json!({
        "model": "claude-sonnet-4-6",
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "hello"}]
    })
}

#[tokio::test]
async fn requires_api_key_before_processing_requests() {
    for headers in [
        vec![],
        vec![("x-api-key", "invalid")],
        vec![("authorization", "Bearer invalid")],
    ] {
        let response = request(
            state(Some(TEST_API_KEY)),
            "/v1/messages/count_tokens",
            &headers,
            Some(token_count_payload()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
    }
}

#[tokio::test]
async fn accepts_openai_and_anthropic_authentication_headers() {
    let bearer = format!("Bearer {TEST_API_KEY}");
    for header in [
        ("authorization", bearer.as_str()),
        ("x-api-key", TEST_API_KEY),
    ] {
        let response = request(
            state(Some(TEST_API_KEY)),
            "/v1/messages/count_tokens",
            &[header],
            Some(token_count_payload()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn rejects_cross_origin_and_dns_rebinding_requests() {
    for headers in [
        vec![
            ("host", "localhost:4141"),
            ("origin", "https://untrusted.example"),
        ],
        vec![("host", "untrusted.example:4141")],
        vec![("host", "localhost:4141"), ("origin", "null")],
    ] {
        let response = request(state(None), "/", &headers, None).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn allows_local_native_clients_without_configured_key() {
    for host in ["localhost:4141", "127.0.0.1:4141", "[::1]:4141"] {
        let response = request(
            state(None),
            "/v1/messages/count_tokens",
            &[("host", host)],
            Some(token_count_payload()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn raw_copilot_token_endpoint_is_not_exposed() {
    let response = request(state(None), "/token", &[("host", "localhost:4141")], None).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.expect("body");
    assert!(!String::from_utf8_lossy(&body).contains("unit-test-copilot-secret"));
    assert_eq!(status, StatusCode::NOT_FOUND);
}

struct MockUpstream {
    url: String,
    requests:
        std::sync::Arc<tokio::sync::Mutex<Vec<(String, axum::http::HeaderMap, serde_json::Value)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn mock_upstream(response: serde_json::Value) -> MockUpstream {
    mock_wire_response(
        StatusCode::OK,
        "application/json",
        serde_json::to_vec(&response).expect("mock JSON"),
    )
    .await
}

async fn mock_wire_response(
    status: StatusCode,
    content_type: &'static str,
    response: Vec<u8>,
) -> MockUpstream {
    let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let captured = captured.clone();
        let response = response.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = to_bytes(body, 32 * 1024 * 1024).await.expect("mock body");
            let json = if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("mock JSON")
            };
            captured
                .lock()
                .await
                .push((parts.uri.path().to_string(), parts.headers, json));
            let chunks = response
                .chunks(7)
                .map(|chunk| {
                    Ok::<_, std::convert::Infallible>(bytes::Bytes::copy_from_slice(chunk))
                })
                .collect::<Vec<_>>();
            let mut outgoing = Response::new(Body::from_stream(futures::stream::iter(chunks)));
            *outgoing.status_mut() = status;
            outgoing.headers_mut().insert(
                "content-type",
                axum::http::HeaderValue::from_static(content_type),
            );
            outgoing
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("7"));
            outgoing
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock listener");
    let url = format!("http://{}", listener.local_addr().expect("mock address"));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });
    MockUpstream {
        url,
        requests,
        task,
    }
}

async fn native_state(upstream: &MockUpstream) -> AppState {
    let state = state(Some(TEST_API_KEY));
    {
        let mut config = state.config.write().await;
        config.copilot_base_url = Some(upstream.url.clone());
        config.models = Some(
            serde_json::from_value(serde_json::json!({
                "object": "list",
                "data": [{
                    "id": "claude-sonnet-4.6",
                    "name": "Claude Sonnet 4.6",
                    "vendor": "Anthropic",
                    "supported_endpoints": ["/v1/messages", "/chat/completions"]
                }]
            }))
            .expect("model metadata"),
        );
    }
    state
}

async fn run_anthropic_fixture(upstream: &MockUpstream, scenario: &str) {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "integration_tests::anthropic_provider_fixture",
            "--nocapture",
        ])
        .env("COPILOT_TEST_ANTHROPIC_SCENARIO", scenario)
        .env("COPILOT_PROVIDER", "anthropic")
        .env("ANTHROPIC_BASE_URL", &upstream.url)
        .env("ANTHROPIC_API_KEY", "unit-test-anthropic-upstream-key")
        .env("ANTHROPIC_VERSION", "2023-06-01-fixture")
        .env_remove("COPILOT_GITHUB_TOKEN")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .expect("fixture deadline")
        .expect("fixture output");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("ANTHROPIC_PROVIDER_CONTRACT_PASSED"));
}

#[tokio::test]
async fn anthropic_provider_fixture() {
    let Ok(scenario) = std::env::var("COPILOT_TEST_ANTHROPIC_SCENARIO") else {
        return;
    };
    let response = request(
        state(Some(TEST_API_KEY)),
        "/v1/models",
        &[("x-api-key", TEST_API_KEY)],
        None,
    )
    .await;
    let expected = if scenario == "rate-limit" {
        StatusCode::TOO_MANY_REQUESTS
    } else {
        StatusCode::OK
    };
    assert_eq!(response.status(), expected);
    if scenario == "rate-limit" {
        assert_eq!(response.headers()["retry-after"], "7");
    }
    let body = to_bytes(response.into_body(), 8192)
        .await
        .expect("models body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("models JSON");
    if scenario == "version" {
        assert_eq!(json["data"][0]["id"], "claude-sonnet-4-6");
    }
    assert!(!String::from_utf8_lossy(&body).contains("unit-test-anthropic-upstream-key"));
    println!("ANTHROPIC_PROVIDER_CONTRACT_PASSED");
}

#[tokio::test]
async fn anthropic_model_discovery_honors_configured_version() {
    let upstream = mock_upstream(serde_json::json!({"data": [{"id": "claude-sonnet-4-6"}]})).await;
    run_anthropic_fixture(&upstream, "version").await;
    let requests = upstream.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let (path, headers, _) = &requests[0];
    assert_eq!(path, "/v1/models");
    assert_eq!(headers["anthropic-version"], "2023-06-01-fixture");
    assert_eq!(headers["x-api-key"], "unit-test-anthropic-upstream-key");
    assert!(!headers.contains_key("authorization"));
}

#[tokio::test]
async fn anthropic_model_discovery_preserves_rate_limit_status() {
    let upstream = mock_wire_response(
        StatusCode::TOO_MANY_REQUESTS,
        "application/json",
        b"{\"error\":{\"type\":\"rate_limit_error\",\"message\":\"fixture\"}}".to_vec(),
    )
    .await;
    run_anthropic_fixture(&upstream, "rate-limit").await;
}

async fn run_provider_case(upstream: &MockUpstream, case: &serde_json::Value) {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "integration_tests::provider_matrix_fixture",
            "--nocapture",
        ])
        .env("COPILOT_TEST_PROVIDER_CASE", case.to_string())
        .env("COPILOT_TEST_UPSTREAM_URL", &upstream.url)
        .env(
            "COPILOT_PROVIDER",
            case["provider"].as_str().expect("provider"),
        )
        .env("ANTHROPIC_BASE_URL", &upstream.url)
        .env("OPENAI_BASE_URL", &upstream.url)
        .env("AZURE_OPENAI_ENDPOINT", &upstream.url)
        .env("ANTHROPIC_API_KEY", "unit-test-provider-key")
        .env("OPENAI_API_KEY", "unit-test-provider-key")
        .env("AZURE_OPENAI_KEY", "unit-test-provider-key")
        .env("AZURE_OPENAI_DEPLOYMENT", "fixture-deployment")
        .env("AZURE_OPENAI_API_VERSION", "2024-10-21")
        .env("ANTHROPIC_VERSION", "2023-06-01")
        .env_remove("COPILOT_GITHUB_TOKEN")
        .env_remove("COPILOT_TEST_ANTHROPIC_SCENARIO")
        .kill_on_drop(true);
    if let Some(environment) = case["env"].as_object() {
        for (name, value) in environment {
            if let Some(value) = value.as_str() {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
        }
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .expect("provider deadline")
        .expect("provider process");
    assert!(
        output.status.success(),
        "case={}\n{}\n{}",
        case["label"],
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("PROVIDER_MATRIX_CASE_PASSED"));
}

#[tokio::test]
async fn provider_matrix_fixture() {
    let Ok(raw) = std::env::var("COPILOT_TEST_PROVIDER_CASE") else {
        return;
    };
    let case: serde_json::Value = serde_json::from_str(&raw).expect("case JSON");
    let mut state = state(Some(TEST_API_KEY));
    let directory = tempfile::tempdir().expect("isolated provider observations");
    let log = directory.path().join("observations.jsonl");
    let observer = crate::hooks::observe::start_observer_at(&log)
        .await
        .expect("provider observer");
    state.hooks = Some(std::sync::Arc::new(crate::hooks::executor::HookExecutor {
        config: Default::default(),
        observer: Some(observer.clone()),
    }));
    {
        let mut config = state.config.write().await;
        config.copilot_base_url =
            Some(std::env::var("COPILOT_TEST_UPSTREAM_URL").expect("mock URL"));
        config.manual_approve = false;
        config.rate_limit_seconds = None;
        config.models = Some(
            serde_json::from_value(serde_json::json!({"object": "list", "data": []}))
                .expect("catalogue"),
        );
    }
    let body = case.get("body").filter(|body| !body.is_null()).cloned();
    let mut headers = vec![
        ("x-api-key", TEST_API_KEY),
        ("anthropic-version", "2023-06-01"),
        ("anthropic-beta", "fixture-capability"),
    ];
    if case["omit_version"] == true {
        headers.retain(|(name, _)| *name != "anthropic-version");
    }
    let response = request(state, case["path"].as_str().expect("path"), &headers, body).await;
    assert_eq!(
        response.status().as_u16() as u64,
        case["expected_status"].as_u64().expect("expected status")
    );
    if let Some(expected) = case["content_type"].as_str() {
        assert_eq!(response.headers()["content-type"], expected);
    }
    if case["expected_status"] == 429 {
        assert_eq!(response.headers()["retry-after"], "7");
    }
    let bytes = to_bytes(response.into_body(), 65_536)
        .await
        .expect("provider response body");
    let text = std::str::from_utf8(&bytes).expect("provider UTF8");
    assert!(!text.contains("unit-test-provider-key"));
    if let Some(expected) = case.get("expected_json") {
        assert_eq!(
            &serde_json::from_slice::<serde_json::Value>(&bytes).expect("provider JSON"),
            expected
        );
    }
    if let Some(expected) = case["expected_text"].as_str() {
        assert_eq!(text, expected);
    }
    if case["expected_status"] == 200
        && matches!(
            case["path"].as_str(),
            Some("/v1/messages" | "/v1/chat/completions" | "/v1/responses")
        )
    {
        observer.flush().await.expect("provider observation flush");
        let events: Vec<serde_json::Value> = tokio::fs::read_to_string(&log)
            .await
            .expect("provider events")
            .lines()
            .map(|line| serde_json::from_str(line).expect("event JSON"))
            .collect();
        assert_eq!(
            events.len(),
            2,
            "provider requests must emit exactly one pre/post pair"
        );
        assert_eq!(events[0]["event"], "PreToolUse");
        assert_eq!(events[1]["event"], "PostToolUse");
    }
    println!("PROVIDER_MATRIX_CASE_PASSED");
}

fn provider_payload(provider: &str, operation: &str, stream: bool) -> serde_json::Value {
    let model = if provider == "anthropic" {
        "claude-sonnet-4-6"
    } else {
        "fixture-deployment"
    };
    match operation {
        "messages" => {
            serde_json::json!({"model": model, "max_tokens": 32, "messages": [{"role": "user", "content": "hello"}], "stream": stream, "thinking": {"type": "adaptive"}})
        }
        "messages/count_tokens" => {
            serde_json::json!({"model": model, "messages": [{"role": "user", "content": "hello"}]})
        }
        "chat/completions" => {
            serde_json::json!({"model": model, "messages": [{"role": "user", "content": "hello"}], "stream": stream, "max_completion_tokens": 32, "parallel_tool_calls": false})
        }
        "responses" => {
            serde_json::json!({"model": model, "input": "hello", "stream": stream, "reasoning": {"effort": "high"}, "store": false})
        }
        "embeddings" => {
            serde_json::json!({"model": model, "input": ["hello"], "dimensions": 256, "encoding_format": "float"})
        }
        _ => serde_json::Value::Null,
    }
}

fn provider_reply(operation: &str) -> serde_json::Value {
    match operation {
        "messages" => {
            serde_json::json!({"id": "msg_fixture", "type": "message", "role": "assistant", "content": [{"type": "text", "text": "hello"}], "stop_reason": "end_turn", "usage": {"input_tokens": 2, "output_tokens": 1}})
        }
        "messages/count_tokens" => serde_json::json!({"input_tokens": 17}),
        "chat/completions" => {
            serde_json::json!({"id": "chatcmpl_fixture", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}]})
        }
        "responses" => {
            serde_json::json!({"id": "resp_fixture", "status": "completed", "output": [{"type": "message", "content": [{"type": "output_text", "text": "hello"}]}]})
        }
        "embeddings" => {
            serde_json::json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [0.1, 0.2]}]})
        }
        _ => serde_json::json!({"data": [{"id": "fixture-model"}], "has_more": false}),
    }
}

#[tokio::test]
async fn provider_matrix_preserves_nonstreaming_requests_and_responses() {
    for provider in ["anthropic", "openai", "azure"] {
        let operations: &[&str] = if provider == "anthropic" {
            &["messages", "messages/count_tokens", "models"]
        } else {
            &["chat/completions", "responses", "embeddings"]
        };
        for operation in operations {
            let reply = provider_reply(operation);
            let upstream = mock_upstream(reply.clone()).await;
            let body = provider_payload(provider, operation, false);
            let case = serde_json::json!({"label": format!("{provider}/{operation}"), "provider": provider, "path": format!("/v1/{operation}"), "body": body, "expected_status": 200, "expected_json": reply});
            run_provider_case(&upstream, &case).await;
            let requests = upstream.requests.lock().await;
            assert_eq!(requests.len(), 1, "{provider}/{operation}");
            let (path, headers, forwarded) = &requests[0];
            let expected_path = if provider == "azure" {
                if *operation == "responses" {
                    "/openai/v1/responses".to_string()
                } else {
                    format!("/openai/deployments/fixture-deployment/{operation}")
                }
            } else {
                format!("/v1/{operation}")
            };
            assert_eq!(*path, expected_path);
            let (credential_name, credential_value) = match provider {
                "anthropic" => ("x-api-key", "unit-test-provider-key"),
                "azure" => ("api-key", "unit-test-provider-key"),
                _ => ("authorization", "Bearer unit-test-provider-key"),
            };
            assert_eq!(headers[credential_name], credential_value);
            if provider == "anthropic" {
                assert_eq!(headers["anthropic-beta"], "fixture-capability");
            }
            if let Some(object) = body.as_object() {
                for (field, value) in object {
                    assert_eq!(&forwarded[field], value, "{provider}/{operation}/{field}");
                }
            }
        }
    }
}

#[tokio::test]
async fn provider_matrix_preserves_streaming_responses() {
    for provider in ["anthropic", "openai", "azure"] {
        let operations: &[&str] = if provider == "anthropic" {
            &["messages"]
        } else {
            &["chat/completions", "responses"]
        };
        for operation in operations {
            let stream = format!(
                "event: fixture\r\ndata:{}\r\n\r\n",
                provider_reply(operation)
            );
            let upstream = mock_wire_response(
                StatusCode::OK,
                "text/event-stream",
                stream.as_bytes().to_vec(),
            )
            .await;
            let case = serde_json::json!({"label": format!("{provider}/{operation}/stream"), "provider": provider, "path": format!("/v1/{operation}"), "body": provider_payload(provider, operation, true), "expected_status": 200, "expected_text": stream, "content_type": "text/event-stream"});
            run_provider_case(&upstream, &case).await;
            assert_eq!(upstream.requests.lock().await.len(), 1);
        }
    }
}

#[tokio::test]
async fn provider_matrix_propagates_errors_and_rejects_empty_credentials() {
    for provider in ["anthropic", "openai", "azure"] {
        let operation = if provider == "anthropic" {
            "messages"
        } else {
            "responses"
        };
        for (label, upstream_status, bytes, expected_status) in [
            (
                "rate-limit",
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"type":"rate_limit_error"}}"#.as_slice(),
                429,
            ),
            (
                "authentication",
                StatusCode::UNAUTHORIZED,
                br#"{"error":{"type":"authentication_error"}}"#.as_slice(),
                401,
            ),
            ("invalid-json", StatusCode::OK, b"not-json".as_slice(), 502),
            ("empty-key", StatusCode::OK, b"{}".as_slice(), 400),
        ] {
            let upstream =
                mock_wire_response(upstream_status, "application/json", bytes.to_vec()).await;
            let mut case = serde_json::json!({"label": format!("{provider}/{label}"), "provider": provider, "path": format!("/v1/{operation}"), "body": provider_payload(provider, operation, false), "expected_status": expected_status});
            if label == "empty-key" {
                let key_name = match provider {
                    "anthropic" => "ANTHROPIC_API_KEY",
                    "azure" => "AZURE_OPENAI_KEY",
                    _ => "OPENAI_API_KEY",
                };
                case["env"] = serde_json::json!({key_name: " "});
            }
            run_provider_case(&upstream, &case).await;
            assert_eq!(
                upstream.requests.lock().await.len(),
                usize::from(label != "empty-key")
            );
        }
    }
}

#[tokio::test]
async fn github_authentication_and_refresh_contracts() {
    for scenario in [
        "authorize",
        "github-refresh",
        "copilot-refresh",
        "copilot-invalid-endpoint",
        "denied",
    ] {
        let directory =
            tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .expect("credential isolation");
        let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let captured = captured.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = to_bytes(body, 8192).await.expect("GitHub mock body");
                let json: serde_json::Value = if body.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&body).expect("GitHub mock JSON") };
                let path = parts.uri.path().to_string();
                let response = match path.as_str() {
                    "/login/device/code" => serde_json::json!({"device_code": "unit-test-device", "user_code": "ABCD-EFGH", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 1}),
                    "/login/oauth/access_token" if scenario == "denied" => serde_json::json!({"error": "access_denied"}),
                    "/login/oauth/access_token" => serde_json::json!({"access_token": "unit-test-authorized", "refresh_token": "unit-test-new-refresh", "expires_in": 3600, "token_type": "bearer"}),
                    "/user" => serde_json::json!({"login": "fixture-user"}),
                    "/copilot_internal/v2/token" => serde_json::json!({
                        "token": "unit-test-fresh-copilot", "expires_at": u64::MAX, "refresh_in": 3600,
                        "endpoints": {"api": if scenario == "copilot-invalid-endpoint" { "https://untrusted.example" } else { "https://api.individual.githubcopilot.com" }}
                    }),
                    _ => panic!("unexpected GitHub request: {path}"),
                };
                captured.lock().await.push((path, parts.headers, json));
                axum::Json(response)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("GitHub mock listener");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("GitHub mock address")
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("GitHub mock server");
        });
        let upstream = MockUpstream {
            url,
            requests,
            task,
        };
        let mut command =
            tokio::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "integration_tests::github_authentication_fixture",
                "--nocapture",
            ])
            .env("COPILOT_TEST_GITHUB_SCENARIO", scenario)
            .env("COPILOT_TEST_GITHUB_URL", &upstream.url)
            .env("COPILOT_DATA_DIR", directory.path())
            .env_remove("COPILOT_GITHUB_TOKEN")
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .expect("GitHub fixture deadline")
            .expect("GitHub fixture process");
        assert!(
            output.status.success(),
            "{scenario}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("GITHUB_AUTHENTICATION_CONTRACT_PASSED")
        );
        let requests = upstream.requests.lock().await;
        let expected = if scenario == "authorize" { 3 } else { 1 };
        assert_eq!(requests.len(), expected, "single-flight {scenario}");
        if scenario == "github-refresh" {
            assert_eq!(requests[0].2["grant_type"], "refresh_token");
            assert_eq!(requests[0].2["refresh_token"], "unit-test-old-refresh");
        }
        if scenario.starts_with("copilot-") {
            assert_eq!(requests[0].0, "/copilot_internal/v2/token");
            assert_eq!(
                requests[0].1["authorization"],
                "Bearer unit-test-github-secret"
            );
        }
    }
}

#[tokio::test]
async fn github_authentication_fixture() {
    let Ok(scenario) = std::env::var("COPILOT_TEST_GITHUB_SCENARIO") else {
        return;
    };
    let root =
        std::path::PathBuf::from(std::env::var_os("COPILOT_DATA_DIR").expect("isolated directory"));
    assert_eq!(crate::paths::get_paths().expect("paths").app_dir, root);
    let mock_url = std::env::var("COPILOT_TEST_GITHUB_URL").expect("mock URL");
    let state = state(Some(TEST_API_KEY));
    {
        let mut config = state.config.write().await;
        config.github_endpoints = crate::services::github::GitHubEndpoints {
            web: mock_url.clone(),
            api: mock_url,
        };
        config.copilot_token = None;
        config.copilot_token_expires_at = None;
    }
    match scenario.as_str() {
        "authorize" | "denied" => {
            if scenario == "authorize" {
                let response = request(
                    state.clone(),
                    "/auth/device-code",
                    &[("x-api-key", TEST_API_KEY)],
                    None,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = to_bytes(response.into_body(), 8192)
                    .await
                    .expect("device body");
                let device: serde_json::Value =
                    serde_json::from_slice(&bytes).expect("device JSON");
                assert_eq!(device["user_code"], "ABCD-EFGH");
            }
            let response = request(
                state.clone(),
                "/auth/poll",
                &[("x-api-key", TEST_API_KEY)],
                Some(serde_json::json!({"device_code": "unit-test-device", "interval": 1})),
            )
            .await;
            assert_eq!(
                response.status(),
                if scenario == "denied" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::OK
                }
            );
            let bytes = to_bytes(response.into_body(), 8192)
                .await
                .expect("poll body");
            assert!(!String::from_utf8_lossy(&bytes).contains("unit-test-authorized"));
            if scenario == "authorize" {
                let stored = crate::token_store::read_github_credential()
                    .await
                    .expect("stored credential")
                    .expect("credential");
                assert_eq!(stored.access_token, "unit-test-authorized");
                assert_eq!(
                    state.config.read().await.github_token.as_deref(),
                    Some("unit-test-authorized")
                );
                assert!(state.config.read().await.copilot_token.is_none());
                let status = request(
                    state.clone(),
                    "/auth/status",
                    &[("x-api-key", TEST_API_KEY)],
                    None,
                )
                .await;
                let bytes = to_bytes(status.into_body(), 8192)
                    .await
                    .expect("status body");
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).expect("status JSON"),
                    serde_json::json!({"authenticated": true})
                );
            } else {
                assert!(
                    crate::token_store::read_github_credential()
                        .await
                        .expect("unchanged store")
                        .is_none()
                );
            }
        }
        "github-refresh" => {
            {
                let mut config = state.config.write().await;
                config.github_token_expires_at = Some(1);
                config.github_refresh_token = Some("unit-test-old-refresh".to_string());
            }
            let results = futures::future::join_all(
                (0..16).map(|_| crate::auth_flow::ensure_github_token(&state)),
            )
            .await;
            for result in results {
                assert_eq!(result.expect("refreshed token"), "unit-test-authorized");
            }
            let stored = crate::token_store::read_github_credential()
                .await
                .expect("read refreshed credential")
                .expect("stored credential");
            assert_eq!(
                stored.refresh_token.as_deref(),
                Some("unit-test-new-refresh")
            );
        }
        "copilot-refresh" => {
            let results = futures::future::join_all(
                (0..16).map(|_| crate::auth_flow::ensure_copilot_token(&state)),
            )
            .await;
            for result in results {
                assert_eq!(
                    result.expect("refreshed Copilot token"),
                    "unit-test-fresh-copilot"
                );
            }
            assert_eq!(
                state.config.read().await.copilot_base_url.as_deref(),
                Some("https://api.individual.githubcopilot.com")
            );
        }
        "copilot-invalid-endpoint" => {
            assert!(
                crate::auth_flow::ensure_copilot_token(&state)
                    .await
                    .is_err()
            );
            assert!(state.config.read().await.copilot_token.is_none());
        }
        _ => panic!("unexpected scenario"),
    }
    println!("GITHUB_AUTHENTICATION_CONTRACT_PASSED");
}

#[tokio::test]
async fn cli_commands_validate_inputs_and_propagate_failures() {
    for (arguments, expected_error, input) in [
        (vec!["debug", "--json"], false, ""),
        (vec!["debug"], false, ""),
        (vec!["check-usage"], true, ""),
        (vec!["start", "--host", "0.0.0.0", "--port", "0"], true, ""),
        (
            vec![
                "start",
                "--account-type",
                "untrusted.example",
                "--port",
                "0",
            ],
            true,
            "",
        ),
        (
            vec!["start", "--host", "127.0.0.1", "--port", "0"],
            false,
            "",
        ),
        (vec!["--addr", "127.0.0.1:0"], false, ""),
        (
            vec!["hook", "--event", "PreToolUse"],
            false,
            "{\"tool_name\":\"Read\"}",
        ),
        (vec!["hook"], true, "{broken"),
        (vec!["hook"], true, "{\"session_id\":\"../escape\"}"),
    ] {
        let directory =
            tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .expect("CLI directory");
        let mut command =
            tokio::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "integration_tests::cli_command_fixture",
                "--nocapture",
            ])
            .env(
                "COPILOT_CLI_FIXTURE",
                serde_json::json!({"arguments": arguments, "expected_error": expected_error})
                    .to_string(),
            )
            .env("COPILOT_DATA_DIR", directory.path().join("server"))
            .env("CLAUDE_CONFIG_DIR", directory.path().join("claude"))
            .env(
                "CLAUDE_HOOKS_PATH",
                directory.path().join("missing-hooks.json"),
            )
            .env("COPILOT_HOOKS_ENABLED", "0")
            .env("COPILOT_PROVIDER", "anthropic")
            .env("COPILOT_EDITOR_VERSION", "1.0.0")
            .env("COPILOT_DISABLE_PROXY", "1")
            .env_remove("COPILOT_GITHUB_TOKEN")
            .env_remove("COPILOT_API_KEY")
            .env_remove("COPILOT_OBSERVATIONS_ENABLED")
            .current_dir(directory.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().expect("CLI process");
        {
            use tokio::io::AsyncWriteExt;
            let mut stdin = child.stdin.take().expect("CLI stdin");
            stdin.write_all(input.as_bytes()).await.expect("CLI input");
        }
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output())
                .await
                .expect("CLI deadline")
                .expect("CLI output");
        assert!(
            output.status.success(),
            "{arguments:?}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("CLI_CONTRACT_PASSED"));
    }
}

#[tokio::test]
async fn cli_command_fixture() {
    let Ok(raw) = std::env::var("COPILOT_CLI_FIXTURE") else {
        return;
    };
    use clap::Parser;
    let case: serde_json::Value = serde_json::from_str(&raw).expect("CLI case");
    let mut arguments = vec!["copilot-api-server".to_string()];
    arguments.extend(
        case["arguments"]
            .as_array()
            .expect("arguments")
            .iter()
            .map(|value| value.as_str().expect("argument").to_string()),
    );
    let cli = crate::cli::Cli::try_parse_from(&arguments).expect("CLI parsing");
    let result = crate::run_cli(cli, std::future::ready(())).await;
    assert_eq!(
        result.is_err(),
        case["expected_error"].as_bool().expect("expected result"),
        "{arguments:?}: {result:?}"
    );
    println!("CLI_CONTRACT_PASSED");
}

#[tokio::test]
async fn native_messages_preserve_capabilities_and_replace_credentials() {
    let upstream_response = serde_json::json!({
        "id": "msg_fixture", "type": "message", "role": "assistant", "model": "claude-sonnet-4.6",
        "content": [{"type": "text", "text": "hello"}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 4, "output_tokens": 1, "cache_read_input_tokens": 100}
    });
    let upstream = mock_upstream(upstream_response.clone()).await;
    let mut payload = token_count_payload();
    payload["thinking"] = serde_json::json!({"type": "adaptive"});
    payload["output_config"] = serde_json::json!({"effort": "high"});
    payload["system"] = serde_json::json!([{"type": "text", "text": "context", "cache_control": {"type": "ephemeral"}}]);
    payload["context_management"] = serde_json::json!({"edits": []});
    payload["tools"] = serde_json::json!([{
        "name": "get_weather", "input_schema": {"type": "object"},
        "defer_loading": true, "strict": true
    }]);
    payload["messages"]
        .as_array_mut()
        .expect("messages")
        .push(serde_json::json!({
            "role": "system", "content": [{"type": "text", "text": "updated context"}],
            "cache_control": {"type": "ephemeral"}
        }));
    let response = request(
        native_state(&upstream).await,
        "/v1/messages?beta=true",
        &[
            ("x-api-key", TEST_API_KEY),
            ("anthropic-version", "2023-06-01"),
            (
                "anthropic-beta",
                "context-management-2025-06-27,tool-search-tool-2025-10-19",
            ),
            ("anthropic-future-capability", "preserved"),
        ],
        Some(payload.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.expect("body");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).expect("JSON"),
        upstream_response
    );
    let requests = upstream.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let (path, headers, forwarded) = &requests[0];
    assert_eq!(path, "/v1/messages");
    assert_eq!(headers["authorization"], "Bearer unit-test-copilot-secret");
    assert!(!headers.contains_key("x-api-key"));
    assert_eq!(
        headers["anthropic-beta"],
        "context-management-2025-06-27,tool-search-tool-2025-10-19"
    );
    assert_eq!(headers["anthropic-future-capability"], "preserved");
    payload["model"] = "claude-sonnet-4.6".into();
    assert_eq!(*forwarded, payload);
}

#[tokio::test]
async fn count_tokens_does_not_require_generation_parameters() {
    let mut payload = token_count_payload();
    payload
        .as_object_mut()
        .expect("request")
        .remove("max_tokens");
    let response = request(
        state(Some(TEST_API_KEY)),
        "/v1/messages/count_tokens",
        &[("x-api-key", TEST_API_KEY)],
        Some(payload),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert!(json["input_tokens"].as_u64().is_some_and(|count| count > 0));
}

#[tokio::test]
async fn native_cache_prewarming_preserves_zero_output_budget() {
    let upstream = mock_upstream(serde_json::json!({
        "id": "msg_cache", "type": "message", "role": "assistant", "content": [],
        "stop_reason": "max_tokens", "usage": {"input_tokens": 12, "output_tokens": 0}
    }))
    .await;
    for native in [true, false] {
        let state = native_state(&upstream).await;
        if !native {
            state
                .config
                .write()
                .await
                .models
                .as_mut()
                .expect("models")
                .data[0]
                .supported_endpoints = vec!["/chat/completions".to_string()];
        }
        let mut payload = token_count_payload();
        payload["max_tokens"] = 0.into();
        let response = request(
            state,
            "/v1/messages",
            &[("x-api-key", TEST_API_KEY)],
            Some(payload),
        )
        .await;
        assert_eq!(
            response.status(),
            if native {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            },
            "native={native}"
        );
    }
    let requests = upstream.requests.lock().await;
    assert_eq!(
        requests.len(),
        1,
        "cache-only requests require native support"
    );
    assert_eq!(requests[0].2["max_tokens"], 0);
}

#[tokio::test]
async fn rejects_invalid_message_shapes_before_upstream_access() {
    for change in [
        serde_json::json!({"messages": []}),
        serde_json::json!({"model": ""}),
        serde_json::json!({"messages": [{"role": "user", "content": 42}]}),
    ] {
        let mut payload = token_count_payload();
        payload
            .as_object_mut()
            .expect("request")
            .extend(change.as_object().expect("change").clone());
        let response = request(
            state(Some(TEST_API_KEY)),
            "/v1/messages/count_tokens",
            &[("x-api-key", TEST_API_KEY)],
            Some(payload),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn model_discovery_only_advertises_real_upstream_models() {
    let upstream = mock_upstream(serde_json::json!({})).await;
    let state = native_state(&upstream).await;
    {
        let mut config = state.config.write().await;
        let model = &mut config.models.as_mut().expect("catalogue").data[0];
        model.capabilities.limits.max_prompt_tokens = Some(200_000);
        model.capabilities.limits.max_output_tokens = Some(64_000);
    }
    let response = request(
        state,
        "/v1/models?limit=1000",
        &[("x-api-key", TEST_API_KEY)],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 8192).await.expect("body");
    let catalogue: serde_json::Value = serde_json::from_slice(&body).expect("catalogue");
    let models = catalogue["data"].as_array().expect("models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "claude-sonnet-4.6");
    assert_eq!(models[0]["anthropic_family_tier"], "sonnet");
    assert_eq!(models[0]["max_input_tokens"], 200_000);
    assert_eq!(models[0]["max_output_tokens"], 64_000);
    assert_eq!(catalogue["first_id"], "claude-sonnet-4.6");
    assert!(upstream.requests.lock().await.is_empty());
}

#[tokio::test]
async fn native_stream_preserves_thinking_signatures_and_pings() {
    let stream = concat!(
        "event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_fixture\"}}\r\n\r\n",
        "event: ping\r\ndata:{\"type\":\"ping\"}\r\n\r\n",
        "event: content_block_delta\r\ndata:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"fixture\"}}\r\n\r\n",
        "event: content_block_delta\r\ndata:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"opaque-fixture\"}}\r\n\r\n",
        "event: message_stop\r\ndata:{\"type\":\"message_stop\"}\r\n\r\n"
    );
    let upstream = mock_wire_response(
        StatusCode::OK,
        "text/event-stream",
        stream.as_bytes().to_vec(),
    )
    .await;
    let mut payload = token_count_payload();
    payload["stream"] = true.into();
    let response = request(
        native_state(&upstream).await,
        "/v1/messages",
        &[("x-api-key", TEST_API_KEY)],
        Some(payload),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = to_bytes(response.into_body(), 8192).await.expect("stream");
    assert_eq!(&body[..], stream.as_bytes());
}

#[tokio::test]
async fn upstream_rate_limit_retains_status_without_disclosing_details() {
    let error = serde_json::json!({"type": "error", "error": {"type": "rate_limit_error", "message": "sensitive-provider-diagnostic"}});
    let upstream = mock_wire_response(
        StatusCode::TOO_MANY_REQUESTS,
        "application/json",
        serde_json::to_vec(&error).expect("error"),
    )
    .await;
    let response = request(
        native_state(&upstream).await,
        "/v1/messages",
        &[("x-api-key", TEST_API_KEY)],
        Some(token_count_payload()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "7");
    let body = to_bytes(response.into_body(), 8192)
        .await
        .expect("error body");
    assert!(!String::from_utf8_lossy(&body).contains("sensitive-provider-diagnostic"));
    let error: serde_json::Value = serde_json::from_slice(&body).expect("error JSON");
    assert_eq!(error["error"]["type"], "rate_limit_error");
}

#[tokio::test]
async fn responses_stream_converts_text_and_function_arguments_separately() {
    let events = [
        serde_json::json!({"type": "response.output_text.delta", "delta": "hello"}),
        serde_json::json!({"type": "response.output_item.added", "output_index": 1, "item": {"type": "function_call", "call_id": "call_fixture", "name": "weather", "arguments": ""}}),
        serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": 1, "delta": "{\"city\":\"Paris\"}"}),
        serde_json::json!({"type": "response.completed", "response": {"usage": {"input_tokens": 3, "output_tokens": 4}}}),
    ];
    let stream = events
        .iter()
        .map(|event| format!("data:{event}\r\n\r\n"))
        .collect::<String>();
    let upstream =
        mock_wire_response(StatusCode::OK, "text/event-stream", stream.into_bytes()).await;
    let state = native_state(&upstream).await;
    {
        let mut config = state.config.write().await;
        let model = &mut config.models.as_mut().expect("catalogue").data[0];
        model.id = "gpt-5.4".to_string();
        model.supported_endpoints = vec!["/responses".to_string()];
    }
    let response = request(
        state,
        "/v1/chat/completions",
        &[("x-api-key", TEST_API_KEY)],
        Some(serde_json::json!({
            "model": "gpt-5.4", "stream": true, "messages": [{"role": "user", "content": "hello"}]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 16_384)
        .await
        .expect("chat stream");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF8");
    let chunks: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect();
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk["choices"][0]["delta"]["content"] == "hello")
    );
    assert!(chunks.iter().any(
        |chunk| chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            == "{\"city\":\"Paris\"}"
    ));
    assert_eq!(
        chunks.last().expect("final chunk")["choices"][0]["finish_reason"],
        "tool_calls"
    );
    assert!(text.ends_with("data: [DONE]\n\n"));
}

#[tokio::test]
async fn copilot_routes_complete_text_tools_embeddings_and_metadata_requests() {
    for operation in ["chat/completions", "responses", "embeddings", "models"] {
        let reply = if operation == "models" {
            serde_json::json!({"object": "list", "data": [{"id": "fixture-deployment", "name": "Fixture"}]})
        } else {
            provider_reply(operation)
        };
        let upstream = mock_upstream(reply.clone()).await;
        let state = native_state(&upstream).await;
        {
            let mut config = state.config.write().await;
            if operation == "models" {
                config.models = None;
            } else {
                let model = &mut config.models.as_mut().expect("models").data[0];
                model.id = "fixture-deployment".to_string();
                model.capabilities.limits.max_output_tokens = Some(64);
            }
        }
        let body = provider_payload("copilot", operation, false);
        let response = request(
            state,
            &format!("/v1/{operation}"),
            &[("x-api-key", TEST_API_KEY)],
            (!body.is_null()).then_some(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{operation}");
        let bytes = to_bytes(response.into_body(), 8192)
            .await
            .expect("Copilot response");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("Copilot JSON");
        if operation != "models" {
            assert_eq!(json, reply);
        } else {
            assert_eq!(json["data"][0]["id"], "fixture-deployment");
        }
        let requests = upstream.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, format!("/{operation}"));
        assert_eq!(
            requests[0].1["authorization"],
            "Bearer unit-test-copilot-secret"
        );
    }
}

#[tokio::test]
async fn usage_route_and_command_handle_quota_formats() {
    for usage in [
        serde_json::json!({"copilot_plan": "fixture", "quota_reset_date": "2026-10-01", "quota_snapshots": {"premium_interactions": {"entitlement": 300, "remaining": 100, "percent_remaining": 33.3}, "chat": {"entitlement": 0, "remaining": 0}}}),
        serde_json::json!({"copilot_plan": "fixture", "quota_snapshots": {}}),
    ] {
        let upstream = mock_upstream(usage.clone()).await;
        let state = native_state(&upstream).await;
        state.config.write().await.github_endpoints.api = upstream.url.clone();
        crate::commands::run_check_usage(&state)
            .await
            .expect("usage command");
        let response = request(state, "/usage", &[("x-api-key", TEST_API_KEY)], None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 8192)
            .await
            .expect("usage body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("usage JSON"),
            usage
        );
        assert_eq!(upstream.requests.lock().await.len(), 2);
    }
}

#[tokio::test]
async fn chat_responses_translation_preserves_named_choice_and_strict_tools() {
    let upstream = mock_upstream(serde_json::json!({
        "id": "resp_fixture", "model": "fixture-responses-model", "status": "completed",
        "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    })).await;
    let state = native_state(&upstream).await;
    state.config.write().await.models = Some(serde_json::from_value(serde_json::json!({
        "object": "list", "data": [{"id": "fixture-responses-model", "supported_endpoints": ["/responses"]}]
    })).expect("responses catalogue"));
    for choice in [
        serde_json::json!({"type": "function", "function": {"name": "lookup"}}),
        serde_json::json!("auto"),
        serde_json::json!("required"),
        serde_json::json!("none"),
    ] {
        let response = request(state.clone(), "/v1/chat/completions", &[("x-api-key", TEST_API_KEY)], Some(serde_json::json!({
            "model": "fixture-responses-model", "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "lookup", "strict": true, "parameters": {"type": "object", "properties": {}, "additionalProperties": false}}}],
            "tool_choice": choice
        }))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let requests = upstream.requests.lock().await;
        let (path, _, body) = requests.last().expect("upstream request");
        assert_eq!(path, "/responses");
        let expected = if choice.is_object() {
            serde_json::json!({"type": "function", "name": "lookup"})
        } else {
            choice
        };
        assert_eq!(body["tool_choice"], expected);
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["tools"][0]["name"], "lookup");
    }
}

#[tokio::test]
async fn native_and_translated_successes_emit_one_post_hook() {
    for (path, capability) in [
        ("/v1/messages", "/v1/messages"),
        ("/v1/messages", "/responses"),
        ("/v1/chat/completions", "/responses"),
    ] {
        for streaming in [false, true] {
            let output = serde_json::json!({
                "id": "fixture-response", "model": "fixture-model", "status": "completed",
                "type": "message", "role": "assistant", "content": [{"type": "text", "text": "ok"}],
                "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}],
                "usage": {"input_tokens": 1, "output_tokens": 1}, "stop_reason": "end_turn"
            });
            let wire = if streaming {
                if capability == "/v1/messages" {
                    b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_vec()
                } else {
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({"type": "response.completed", "response": output})
                    )
                    .into_bytes()
                }
            } else {
                serde_json::to_vec(&output).expect("upstream JSON")
            };
            let upstream = mock_wire_response(
                StatusCode::OK,
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
                wire,
            )
            .await;
            let mut state = native_state(&upstream).await;
            state.config.write().await.models = Some(serde_json::from_value(serde_json::json!({
                "object": "list", "data": [{"id": "fixture-model", "supported_endpoints": [capability]}]
            })).expect("fixture models"));
            let directory = tempfile::tempdir().expect("isolated observations");
            let log = directory.path().join("observations.jsonl");
            let observer = crate::hooks::observe::start_observer_at(&log)
                .await
                .expect("observer");
            state.hooks = Some(std::sync::Arc::new(crate::hooks::executor::HookExecutor {
                config: Default::default(),
                observer: Some(observer.clone()),
            }));
            let response = request(
                state,
                path,
                &[("x-api-key", TEST_API_KEY)],
                Some(serde_json::json!({
                    "model": "fixture-model", "max_tokens": 64, "stream": streaming,
                    "messages": [{"role": "user", "content": "unit-test-private-message"}]
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{path} {capability}");
            assert!(
                !to_bytes(response.into_body(), 8192)
                    .await
                    .expect("complete response")
                    .is_empty()
            );
            observer.flush().await.expect("flush hook events");
            let text = tokio::fs::read_to_string(&log).await.expect("hook events");
            let events: Vec<serde_json::Value> = text
                .lines()
                .map(|line| serde_json::from_str(line).expect("event JSON"))
                .collect();
            assert_eq!(events.len(), 2, "{path} {capability} streaming={streaming}");
            assert_eq!(events[0]["event"], "PreToolUse");
            assert_eq!(events[1]["event"], "PostToolUse");
            assert!(!text.contains("unit-test-private-message"));
        }
    }
}

#[tokio::test]
async fn messages_route_uses_catalogue_responses_capability() {
    let upstream = mock_upstream(serde_json::json!({
        "status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "hello"}]},
            {"type": "function_call", "call_id": "call_fixture", "name": "weather", "arguments": "{}"}
        ], "usage": {"input_tokens": 5, "output_tokens": 3}
    })).await;
    let state = native_state(&upstream).await;
    {
        let mut config = state.config.write().await;
        let model = &mut config.models.as_mut().expect("models").data[0];
        model.id = "fixture-responses-model".to_string();
        model.supported_endpoints = vec!["/responses".to_string()];
    }
    let mut body = token_count_payload();
    body["model"] = "fixture-responses-model".into();
    let response = request(
        state,
        "/v1/messages",
        &[("x-api-key", TEST_API_KEY)],
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8192)
        .await
        .expect("Messages response");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("Messages JSON");
    assert_eq!(json["content"][0]["text"], "hello");
    assert_eq!(json["content"][1]["type"], "tool_use");
    assert_eq!(json["usage"]["input_tokens"], 5);
    assert_eq!(json["stop_reason"], "tool_use");
    assert_eq!(upstream.requests.lock().await[0].0, "/responses");
}

fn translated_stream_events(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

#[tokio::test]
async fn translated_messages_stream_handles_tools_usage_and_failure() {
    for scenario in ["success", "incomplete", "invalid-json", "upstream-error"] {
        let mut chunks = vec![
            serde_json::json!({"model": "fixture-deployment", "choices": [{"delta": {"content": "hello"}}]}),
        ];
        chunks.push(serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_fixture", "function": {"name": "weather", "arguments": "{}"}}]}}]}));
        let mut stream = chunks
            .iter()
            .map(|chunk| format!("data:{chunk}\r\n\r\n"))
            .collect::<String>();
        match scenario {
            "success" => {
                stream.push_str(
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                );
                stream.push_str("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\n");
                stream.push_str("data: [DONE]\n\n");
            }
            "invalid-json" => stream.push_str("data:{invalid}\n\n"),
            "upstream-error" => stream
                .push_str("data:{\"error\":{\"message\":\"unit-test-private-diagnostic\"}}\n\n"),
            _ => {}
        }
        let upstream =
            mock_wire_response(StatusCode::OK, "text/event-stream", stream.into_bytes()).await;
        let state = native_state(&upstream).await;
        state
            .config
            .write()
            .await
            .models
            .as_mut()
            .expect("models")
            .data[0]
            .supported_endpoints = vec!["/chat/completions".to_string()];
        let mut body = token_count_payload();
        body["stream"] = true.into();
        let response = request(
            state,
            "/v1/messages",
            &[("x-api-key", TEST_API_KEY)],
            Some(body),
        )
        .await;
        let bytes = to_bytes(response.into_body(), 32768)
            .await
            .expect("translated stream");
        let text = String::from_utf8(bytes.to_vec()).expect("UTF8 stream");
        assert!(!text.contains("unit-test-private-diagnostic"));
        let events = translated_stream_events(&text);
        assert_eq!(events[0]["type"], "message_start");
        if scenario == "success" {
            assert_eq!(events.last().expect("terminal")["type"], "message_stop");
            let usage = &events[events.len() - 2]["usage"];
            assert_eq!(usage["input_tokens"], 10);
            assert_eq!(usage["output_tokens"], 4);
            assert_eq!(usage["cache_read_input_tokens"], 2);
            assert!(
                events
                    .iter()
                    .any(|event| event["delta"]["partial_json"] == "{}")
            );
        } else {
            assert_eq!(events.last().expect("failure")["type"], "error");
            assert!(!events.iter().any(|event| event["type"] == "message_stop"));
        }
    }
}
