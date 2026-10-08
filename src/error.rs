#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("network error: {0}")]
    Network(String),
    /// Session cookies are missing, expired or rejected (HTTP 401/403).
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("unexpected response: {0}")]
    Parse(String),
}
