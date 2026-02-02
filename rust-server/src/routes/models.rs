use axum::{extract::State, response::IntoResponse, Json};

use crate::{
    auth_flow::ensure_copilot_token,
    errors::ApiResult,
    services::{copilot::get_models, openai, azure},
    state::{AppState, Model},
};

pub async fn list(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let provider = std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string());
    if provider == "openai" {
        let models = openai::list_models(&state.client).await?;
        return Ok(Json(models));
    }

    if provider == "azure" {
        if let Some(cfg) = azure::load_azure_config("azure:") {
            let model_id = format!("azure:{}", cfg.deployment);
            return Ok(Json(serde_json::json!({
                "object": "list",
                "data": [
                    {
                        "id": model_id,
                        "object": "model",
                        "type": "model",
                        "created": 0,
                        "created_at": "1970-01-01T00:00:00Z",
                        "owned_by": "azure",
                        "display_name": "Azure OpenAI Deployment",
                    }
                ],
                "has_more": false
            })));
        }
    }

    let token = ensure_copilot_token(&state).await?;

    let models = {
        let config = state.config.read().await;
        if let Some(models) = &config.models {
            models.clone()
        } else {
            drop(config);
            let config_snapshot = state.config.read().await.clone();
            let models = get_models(&state.client, &config_snapshot, &token).await?;
            state.config.write().await.models = Some(models.clone());
            models
        }
    };

    let mut data: Vec<serde_json::Value> = models
        .data
        .iter()
        .map(|model| model_to_openai(model))
        .collect();

    for synth in synthetic_models() {
        if !data.iter().any(|m| m.get("id") == Some(&serde_json::Value::String(synth.id.clone()))) {
            data.push(model_to_openai(&synth));
        }
    }

    if std::env::var("COPILOT_EXPOSE_MODEL_ALIASES").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false) {
        for alias in alias_models() {
            if !data.iter().any(|m| m.get("id") == Some(&alias["id"])) {
                data.push(alias);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "object": "list",
        "data": data,
        "has_more": false,
    })))
}

fn model_to_openai(model: &Model) -> serde_json::Value {
    serde_json::json!({
        "id": model.id,
        "object": "model",
        "type": "model",
        "created": 0,
        "created_at": "1970-01-01T00:00:00Z",
        "owned_by": model.vendor,
        "display_name": model.name,
    })
}

fn synthetic_models() -> Vec<Model> {
    vec![
        Model {
            id: "gpt-5.2-codex".to_string(),
            name: "GPT-5.2 Codex".to_string(),
            vendor: "openai".to_string(),
            ..default_model()
        },
        Model {
            id: "o3".to_string(),
            name: "OpenAI O3".to_string(),
            vendor: "openai".to_string(),
            ..default_model()
        },
        Model {
            id: "o3-mini".to_string(),
            name: "OpenAI O3 Mini".to_string(),
            vendor: "openai".to_string(),
            ..default_model()
        },
    ]
}

fn alias_models() -> Vec<serde_json::Value> {
    vec![
        alias("gpt-5.2-codex", "gpt-4o"),
        alias("codex-5.2", "gpt-4o"),
        alias("o3", "gpt-4o"),
        alias("o3-mini", "gpt-4o-mini"),
        alias("o1", "o1-preview"),
        alias("claude-sonnet-4", "claude-3.5-sonnet"),
        alias("claude-4-sonnet", "claude-3.5-sonnet"),
    ]
}

fn alias(id: &str, target: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "model",
        "type": "model",
        "created": 0,
        "created_at": "1970-01-01T00:00:00Z",
        "owned_by": "alias",
        "display_name": format!("{} (alias of {})", id, target),
    })
}

#[cfg(test)]
mod tests {
    use super::{alias_models, alias};

    #[test]
    fn alias_model_display_name() {
        let model = alias("o3", "gpt-4o");
        assert_eq!(model.get("id").and_then(|v| v.as_str()), Some("o3"));
        assert!(model.get("display_name").and_then(|v| v.as_str()).unwrap_or("").contains("alias"));
    }

    #[test]
    fn alias_models_contains_expected() {
        let aliases = alias_models();
        assert!(aliases.iter().any(|m| m.get("id") == Some(&serde_json::Value::String("o3".to_string()))));
        assert!(aliases.iter().any(|m| m.get("id") == Some(&serde_json::Value::String("claude-4-sonnet".to_string()))));
    }
}

fn default_model() -> Model {
    Model {
        capabilities: crate::state::ModelCapabilities {
            family: "".to_string(),
            limits: Default::default(),
            object: "model_capabilities".to_string(),
            supports: Default::default(),
            tokenizer: "".to_string(),
            r#type: "model".to_string(),
        },
        id: "".to_string(),
        model_picker_enabled: true,
        name: "".to_string(),
        object: "model".to_string(),
        preview: false,
        vendor: "".to_string(),
        version: "".to_string(),
        policy: None,
    }
}
