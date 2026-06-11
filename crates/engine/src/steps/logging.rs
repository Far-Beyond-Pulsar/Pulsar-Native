//! Logging step: Tracy/tracing setup.

use crate::consts;
use crate::init::{InitContext, InitError};

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let log_guard = crate::logging::init(ctx.launch_args.verbose);

    tracing::debug!("{}", consts::ENGINE_NAME);
    tracing::debug!("Version: {}", consts::ENGINE_VERSION);
    tracing::debug!("Authors: {}", consts::ENGINE_AUTHORS);
    tracing::debug!("Description: {}", consts::ENGINE_DESCRIPTION);
    tracing::debug!("🚀 Starting Pulsar Engine with Winit + GPUI Zero-Copy Composition");
    tracing::debug!(
        "Command-line arguments: {:?}",
        std::env::args().collect::<Vec<_>>()
    );

    ctx.log_guard = Some(log_guard);
    Ok(())
}
