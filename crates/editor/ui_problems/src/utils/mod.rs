//! Diagnostic types, the listed items, dismissals, actions and filtering for
//! the Problems drawer.

pub mod actions;
pub mod dismissed;
pub mod filter;
pub mod items;
pub mod types;

pub use filter::compute_aligned_diff;
pub use types::{Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic, RunFindingAction};
