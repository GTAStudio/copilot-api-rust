use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::errors::{ApiError, ApiResult};
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
    sender: mpsc::Sender<ObservationMessage>,
}

#[derive(Debug)]
enum ObservationMessage {
    Event(ObservationEvent),
    Flush(oneshot::Sender<ApiResult<()>>),
}

impl ObservationHub {
    pub fn emit(&self, mut event: ObservationEvent) {
        event.input = None;
        event.output = None;
        let _ = self.sender.try_send(ObservationMessage::Event(event));
    }

    pub async fn flush(&self) -> ApiResult<()> {
        let flush = async {
            let (sender, receiver) = oneshot::channel();
            self.sender
                .send(ObservationMessage::Flush(sender))
                .await
                .map_err(|_| ApiError::Internal("Observation writer stopped".to_string()))?;
            receiver
                .await
                .map_err(|_| ApiError::Internal("Observation writer stopped".to_string()))?
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), flush)
            .await
            .map_err(|_| ApiError::Internal("Observation flush timed out".to_string()))?
    }
}

pub async fn start_observer() -> ApiResult<ObservationHub> {
    let path = claude_paths::observations_file()?;
    start_observer_at(&path).await
}

pub(crate) async fn start_observer_at(path: &std::path::Path) -> ApiResult<ObservationHub> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::Internal("Invalid observation path".to_string()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| ApiError::Internal("Cannot create observation directory".to_string()))?;
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|_| ApiError::Internal("Cannot open observation log".to_string()))?;
    let mut size = file
        .metadata()
        .await
        .map_err(|_| ApiError::Internal("Cannot inspect observation log".to_string()))?
        .len();
    let (sender, mut receiver) = mpsc::channel(128);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut failed = false;
        while let Some(message) = receiver.recv().await {
            match message {
                ObservationMessage::Event(event) => {
                    if let Ok(mut line) = serde_json::to_vec(&event) {
                        line.push(b'\n');
                        if !failed && size.saturating_add(line.len() as u64) <= 8 * 1024 * 1024 {
                            failed = file.write_all(&line).await.is_err();
                            size = size.saturating_add(line.len() as u64);
                        }
                    }
                }
                ObservationMessage::Flush(acknowledgement) => {
                    let result = if failed {
                        Err(ApiError::Internal(
                            "Observation log write failed".to_string(),
                        ))
                    } else {
                        file.sync_data().await.map_err(|_| {
                            ApiError::Internal("Observation log flush failed".to_string())
                        })
                    };
                    let _ = acknowledgement.send(result);
                }
            }
        }
    });
    Ok(ObservationHub { sender })
}

pub fn build_event(event: &str, input: &crate::hooks::types::HookInput) -> ObservationEvent {
    ObservationEvent {
        timestamp: Utc::now().to_rfc3339(),
        event: event.to_string(),
        session: input.resolved_session_id(),
        tool: input.tool.clone(),
        input: None,
        output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn observer_creates_directory_flushes_events_and_protects_contents() {
        let directory = tempfile::tempdir().expect("observer directory");
        let path = directory.path().join("nested/observations.jsonl");
        let observer = start_observer_at(&path).await.expect("observer");
        let input = crate::hooks::types::HookInput {
            session_id: Some("fixture-session".to_string()),
            tool: Some("Read".to_string()),
            tool_input: Some(serde_json::json!({"secret": "unit-test-private-input"})),
            ..Default::default()
        };
        for event in ["PreToolUse", "PostToolUse", "SessionEnd"] {
            observer.emit(build_event(event, &input));
        }
        observer.flush().await.expect("flush acknowledgements");
        let text = tokio::fs::read_to_string(&path)
            .await
            .expect("observation contents");
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("event JSON"))
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2]["event"], "SessionEnd");
        assert!(!text.contains("unit-test-private-input"));
        drop(observer);
    }

    #[tokio::test]
    async fn observer_rejects_invalid_paths_and_limits_persistent_logs() {
        let directory = tempfile::tempdir().expect("observer directory");
        let block = directory.path().join("not-a-directory");
        tokio::fs::write(&block, "preserve")
            .await
            .expect("blocking file");
        assert!(
            start_observer_at(&block.join("observations.jsonl"))
                .await
                .is_err()
        );
        let path = directory.path().join("full.jsonl");
        let file = tokio::fs::File::create(&path).await.expect("log");
        file.set_len(8 * 1024 * 1024).await.expect("full log");
        drop(file);
        let observer = start_observer_at(&path).await.expect("existing log");
        observer.emit(build_event("PreToolUse", &Default::default()));
        observer.flush().await.expect("bounded flush");
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .expect("log metadata")
                .len(),
            8 * 1024 * 1024
        );
    }

    #[test]
    fn default_observation_does_not_record_prompt_or_tool_contents() {
        let input = crate::hooks::types::HookInput {
            tool: Some("Read".to_string()),
            tool_input: Some(serde_json::json!({"secret": "unit-test-secret"})),
            tool_output: Some(serde_json::json!({"content": "unit-test-private-content"})),
            ..Default::default()
        };
        let event = build_event("PostToolUse", &input);
        assert!(event.input.is_none());
        assert!(event.output.is_none());
        assert_eq!(event.tool.as_deref(), Some("Read"));
    }
}
