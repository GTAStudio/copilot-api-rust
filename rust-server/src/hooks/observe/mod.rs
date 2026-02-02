use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::errors::ApiResult;
use crate::hooks::claude_paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationEvent {
    pub timestamp: String,
    pub event: String,
    pub session: Option<String>,
    pub tool: Option<String>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ObservationHub {
    pub sender: broadcast::Sender<ObservationEvent>,
}

impl ObservationHub {
    pub fn emit(&self, event: ObservationEvent) {
        let _ = self.sender.send(event);
    }
}

pub async fn start_observer() -> ApiResult<ObservationHub> {
    let (sender, mut receiver) = broadcast::channel(128);
    let path = claude_paths::observations_file()?;
    tokio::spawn(async move {
        let mut file = match tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
            Ok(f) => f,
            Err(_) => return,
        };
        while let Ok(event) = receiver.recv().await {
            if let Ok(line) = serde_json::to_string(&event) {
                let _ = tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await;
                let _ = tokio::io::AsyncWriteExt::write_all(&mut file, b"\n").await;
            }
        }
    });
    Ok(ObservationHub { sender })
}

pub fn build_event(
    event: &str,
    input: &crate::hooks::types::HookInput,
) -> ObservationEvent {
    ObservationEvent {
        timestamp: Utc::now().to_rfc3339(),
        event: event.to_string(),
        session: input.resolved_session_id(),
        tool: input.tool.clone(),
        input: input.tool_input.clone(),
        output: input.tool_output.clone(),
    }
}
