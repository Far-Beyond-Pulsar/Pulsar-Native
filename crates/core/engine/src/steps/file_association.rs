//! Project file association step: prompt to associate project files.

use crate::file_association;
use crate::init::{InitContext, InitError};

pub fn run(_ctx: &mut InitContext) -> Result<(), InitError> {
    file_association::maybe_prompt_project_file_association();
    Ok(())
}
