pub mod errors;
pub mod validators;

pub use errors::{HookError, HookResult, WindowError, WindowResult};
pub use validators::{MaxWindowsRule, ValidationRule, WindowValidator};
