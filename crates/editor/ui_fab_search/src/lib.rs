//! The Fab asset search window (`FabSearchWindow`): authentication, result parsing, image
//! loading and the search index.

pub mod auth;
pub mod image_loader;
pub mod parser;
mod search_index;

pub mod components;
mod handlers;
mod screen;
mod utils;

pub(crate) use screen::FabSearchWindow;
pub(crate) use utils::actions::DownloadState;
