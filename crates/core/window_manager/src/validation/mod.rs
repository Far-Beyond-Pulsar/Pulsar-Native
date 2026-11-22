//! Window validation: the `WindowError` / `HookError` types and the rules (such as
//! `MaxWindowsRule`) a `WindowValidator` applies.

pub mod errors;
pub mod validators;

pub use errors::{HookError, HookResult, WindowError, WindowResult};
pub use validators::{MaxWindowsRule, ValidationRule, WindowValidator};
