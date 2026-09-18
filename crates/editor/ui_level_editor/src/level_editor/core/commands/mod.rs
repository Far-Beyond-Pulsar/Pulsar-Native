/// Scene command system - single execution path for all scene mutations.
///
/// `SceneCommand` is a self-contained description of one editor operation.
/// `execute_command()` applies it through `SceneDatabase`, which in turn
/// writes to **both** `SceneDb` (the canonical store) and the Helio renderer
/// (immediate viewport update) in one call.
///
/// Both user GPUI action handlers and AI tool implementations call
/// `execute_command()`, giving a single auditable code path that is ready for
/// undo / redo to be layered on top.

mod executor;
mod types;

pub use executor::execute_command;
pub use types::{CommandResult, SceneCommand};

#[cfg(test)]
mod tests;
