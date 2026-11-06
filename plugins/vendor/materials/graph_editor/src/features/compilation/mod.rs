//! Compilation feature: compiling the shader document to WGSL.
//!
//! - [`compiler`]: PSGC compile with WGSL validation, status and history
//! - [`lowering`]: the document as PSGC reads it (texture bindings, input defaults)

pub mod compiler;
pub mod lowering;

/// Compilation status tracking for UI feedback
#[derive(Clone, Debug, PartialEq)]
pub enum CompilationState {
    Idle,
    Compiling,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct CompilationStatus {
    pub state: CompilationState,
    pub message: String,
    pub progress: f32, // 0.0 to 1.0
    pub is_compiling: bool,
}

impl Default for CompilationStatus {
    fn default() -> Self {
        Self {
            state: CompilationState::Idle,
            message: "Ready to compile".to_string(),
            progress: 0.0,
            is_compiling: false,
        }
    }
}
