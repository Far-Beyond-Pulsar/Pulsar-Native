//! URI registration step: custom URI scheme (pulsar://).

use crate::init::{InitContext, InitError};
use crate::uri;

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let rt = ctx
        .runtime
        .as_ref()
        .ok_or(InitError::MissingContext("Runtime not initialized"))?;

    rt.spawn(async {
        if let Err(e) = uri::ensure_uri_scheme_registered() {
            tracing::error!("Failed to register URI scheme: {}", e);
        }
    });
    Ok(())
}
