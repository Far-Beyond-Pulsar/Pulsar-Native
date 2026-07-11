//! Runtime step: Tokio async runtime setup.

use crate::init::{InitContext, InitError};
use crate::runtime;

pub fn run(ctx: &mut InitContext) -> Result<(), InitError> {
    let rt = runtime::create_runtime();
    ctx.runtime = Some(rt);
    Ok(())
}
