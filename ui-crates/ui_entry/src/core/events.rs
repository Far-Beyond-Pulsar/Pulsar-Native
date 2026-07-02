use gpui::*;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ProjectSelected {
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct GitManagerRequested {
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SettingsRequested;

#[derive(Clone, Debug)]
pub struct FabSearchRequested;
