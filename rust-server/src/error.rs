use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unexpected response: {0}")]
    Unexpected(String),
}

pub type AppResult<T> = Result<T, AppError>;
