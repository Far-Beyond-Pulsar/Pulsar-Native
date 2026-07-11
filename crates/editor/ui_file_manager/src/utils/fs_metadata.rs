use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const METADATA_FILE: &str = ".pulsar_fs_meta";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorOverride {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl From<gpui::Hsla> for ColorOverride {
    fn from(hsla: gpui::Hsla) -> Self {
        let rgba = gpui::Rgba::from(hsla);
        Self {
            r: (rgba.r * 255.0) as u8,
            g: (rgba.g * 255.0) as u8,
            b: (rgba.b * 255.0) as u8,
            a: (rgba.a * 255.0) as u8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Metadata {
    colors: HashMap<String, Option<ColorOverride>>,
    #[serde(default)]
    pinned: Vec<String>,
}

pub struct FsMetadataManager {
    metadata: Metadata,
    path: Option<PathBuf>,
}

impl FsMetadataManager {
    pub fn new() -> Self {
        Self {
            metadata: Metadata::default(),
            path: None,
        }
    }

    pub fn load_from_project_root(&mut self, project_path: &Path) {
        let meta_path = project_path.join(METADATA_FILE);
        if let Ok(content) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<Metadata>(&content) {
                self.metadata = meta;
                self.path = Some(project_path.to_path_buf());
                return;
            }
        }
        self.path = Some(project_path.to_path_buf());
    }

    fn load_from_parents(&mut self, start_path: &Path) {
        let mut current = Some(start_path);
        while let Some(dir) = current {
            let meta_path = dir.join(METADATA_FILE);
            if let Ok(content) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<Metadata>(&content) {
                    self.metadata = meta;
                    self.path = Some(dir.to_path_buf());
                    return;
                }
            }
            current = dir.parent();
        }
    }

    fn save(&self) {
        if let Some(ref path) = self.path {
            let meta_path = path.join(METADATA_FILE);
            if let Ok(content) = serde_json::to_string_pretty(&self.metadata) {
                let _ = std::fs::write(&meta_path, content);
            }
        }
    }

    pub fn get_color_override(&mut self, file_path: &Path) -> Option<gpui::Hsla> {
        if self.path.is_none() {
            if let Some(parent) = file_path.parent() {
                self.load_from_parents(parent);
            }
        }
        let key = file_path.to_string_lossy().to_string();
        self.metadata
            .colors
            .get(&key)
            .and_then(|c| c.as_ref())
            .map(|c| {
                gpui::Hsla::from(gpui::Rgba {
                    r: c.r as f32 / 255.0,
                    g: c.g as f32 / 255.0,
                    b: c.b as f32 / 255.0,
                    a: c.a as f32 / 255.0,
                })
            })
    }

    pub fn set_color_override(
        &mut self,
        file_path: &Path,
        color: Option<gpui::Hsla>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let key = file_path.to_string_lossy().to_string();
        self.metadata
            .colors
            .insert(key, color.map(ColorOverride::from));
        self.save();
        Ok(())
    }

    pub fn rename_file(
        &mut self,
        old_path: &Path,
        new_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let old_key = old_path.to_string_lossy().to_string();
        let new_key = new_path.to_string_lossy().to_string();
        if let Some(color) = self.metadata.colors.remove(&old_key) {
            self.metadata.colors.insert(new_key, color);
        }
        self.save();
        Ok(())
    }
}
