use thiserror::Error;

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Plugin load error: {0}")]
    PluginLoad(String),
    #[error("Runtime error: {0}")]
    Runtime(String),
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

#[macro_export]
macro_rules! unwrap_or_warn {
    ($expr:expr) => {{
        match $expr {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("unwrap_or_warn failed: {}", e);
                Default::default()
            }
        }
    }};
}

#[macro_export]
macro_rules! expect_or_log {
    ($expr:expr, $msg:expr) => {{
        match $expr {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("{}: {}", $msg, e);
                Default::default()
            }
        }
    }};
}
