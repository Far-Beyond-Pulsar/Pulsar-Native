use thiserror::Error;

#[derive(Error, Debug)]
pub enum TypeSystemError {
    #[error("Invalid type name: {0}. Names must match ^[A-Za-z_][A-Za-z0-9_]*$")]
    InvalidName(String),

    #[error("Type name collision: {0} already exists in {1}")]
    NameCollision(String, String),

    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),

    #[error("Invalid generic arity: {constructor} expects {expected} parameters, got {actual}")]
    InvalidArity {
        constructor: String,
        expected: usize,
        actual: usize,
    },

    #[error("Type not found: {0}")]
    TypeNotFound(String),

    #[error("Alias not found: {0}")]
    AliasNotFound(String),

    #[error("Invalid type reference: {0}")]
    InvalidTypeReference(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Code generation failed: {0}")]
    CodeGeneration(String),

    #[error("Validation failed: {0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, TypeSystemError>;
