//! Engine context step: global typed state.

use crate::init::{InitContext, InitError};
use crate::uri;
use engine_state::EngineContext;

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let engine_context = EngineContext::new();

    // Handle URI project path if present
    if let Some(uri::UriCommand::OpenProject { path }) = &ctx.launch_args.uri_command {
        tracing::debug!("Launching project from URI: {}", path.display());
        engine_context
            .store
            .get_or_init::<engine_state::LaunchContext>()
            .update(|l| l.uri_project_path = Some(path.clone()));
    }

    ctx.engine_context = Some(engine_context);
    Ok(())
}
