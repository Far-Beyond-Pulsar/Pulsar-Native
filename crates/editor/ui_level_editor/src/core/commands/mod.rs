/// Scene command system - single execution path for all scene mutations.
///
/// `SceneCommand` is a self-contained description of one editor operation.
/// `execute_command()` applies it through the `scene_edit` functions, which
/// write the `SceneDb` world directly; the renderer reads that same world,
/// so the viewport follows without a separate update.
///
/// Both user GPUI action handlers and AI tool implementations call
/// `execute_command()`, giving a single auditable code path that is ready for
/// undo / redo to be layered on top.
mod executor;
mod types;

pub use executor::execute_command;
pub use types::{CommandResult, ComponentData, SceneCommand, TypedComponent};

#[cfg(test)]
mod tests;
