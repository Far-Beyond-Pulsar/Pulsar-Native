//! Dev/source detection step: must run before `set_global`.

use crate::init::{InitContext, InitError};
use crate::Assets;

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let engine_context = ctx
        .engine_context
        .as_ref()
        .ok_or(InitError::MissingContext("Engine context not initialized"))?;

    let dev = engine_state::DevContext::detect();
    if dev.is_source_build {
        tracing::info!(
            "Source build detected — workspace root: {:?}",
            dev.source_path
        );
    } else {
        tracing::debug!("Running from installed/distributed binary");
    }
    engine_context
        .store
        .get_or_init::<engine_state::DevContext>()
        .set(dev);

    // Stash what a new project starts with (the embedded default level and
    // the meshes it references, under `assets/meshes/`) so the level editor
    // seeds new projects without depending on the engine crate directly.
    let level: Option<std::sync::Arc<[u8]>> =
        Assets::get("default.level").map(|file| std::sync::Arc::from(file.data.as_ref()));
    match &level {
        Some(level) => tracing::info!("Embedded default.level found ({} bytes)", level.len()),
        None => tracing::debug!("No embedded default.level — new projects start empty"),
    }
    let assets = <crate::assets::EngineAssets as rust_embed::RustEmbed>::iter()
        .filter(|path| path.starts_with("meshes/"))
        .filter_map(|path| {
            let file = Assets::get(&path)?;
            Some((path.into_owned(), std::sync::Arc::from(file.data.as_ref())))
        })
        .collect();
    engine_context
        .store
        .get_or_init::<engine_state::StarterContent>()
        .set(engine_state::StarterContent { level, assets });

    Ok(())
}
