use ui_types_common::window_types::WindowId;

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("Window {0} does not exist")]
    WindowNotFound(WindowId),

    #[error("Cannot close window {0}: {1}")]
    CannotClose(WindowId, String),

    #[error("Invalid window options: {0}")]
    InvalidOptions(String),

    #[error("Hook execution failed: {0}")]
    HookFailed(#[from] HookError),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("GPUI error: {0}")]
    GpuiError(String),

    #[error("Window manager not initialized")]
    NotInitialized,

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("Hook execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Blocking hook failed: {0}")]
    BlockingHookFailed(String),

    #[error("Hook timeout: {0}")]
    Timeout(String),

    #[error("Hook error: {0}")]
    Other(#[from] anyhow::Error),
}

pub type WindowResult<T> = Result<T, WindowError>;
pub type HookResult<T> = Result<T, HookError>;
