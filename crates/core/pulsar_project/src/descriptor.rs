//! `Pulsar.toml`, the project descriptor the user owns: the project's
//! identity (`[project]`) and its plugins (`[plugins]`,
//! `pulsar_plugins::ProjectPlugins`). Read through the filesystem library.

use std::path::Path;

use engine_fs::virtual_fs;

use crate::error::SyncError;
use crate::paths;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectDescriptor {
    /// `[project] name`: what the game is called (its executable is named
    /// after it). Without one, the project's folder name.
    pub name: Option<String>,
    /// `[project] version`, a Cargo version; `0.1.0` without one.
    pub version: Option<String>,
    pub plugins: pulsar_plugins::ProjectPlugins,
}

impl ProjectDescriptor {
    pub fn read(project_root: &Path) -> Result<Self, SyncError> {
        let path = paths::join(project_root, paths::DESCRIPTOR);
        if !virtual_fs::is_file(&path) {
            return Err(SyncError::NotAProject(project_root.to_path_buf()));
        }
        let text = virtual_fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, SyncError> {
        let doc: toml::Table = toml::from_str(text).map_err(|e| SyncError::Descriptor(e.to_string()))?;
        let project = match doc.get("project") {
            None => None,
            Some(toml::Value::Table(t)) => Some(t),
            Some(_) => return Err(SyncError::Descriptor("`project` must be a table".into())),
        };
        let string = |key: &str| -> Result<Option<String>, SyncError> {
            match project.and_then(|p| p.get(key)) {
                None => Ok(None),
                Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_owned())),
                Some(_) => Err(SyncError::Descriptor(format!(
                    "`project.{key}` must be a non-empty string"
                ))),
            }
        };
        Ok(Self {
            name: string("name")?,
            version: string("version")?,
            plugins: pulsar_plugins::ProjectPlugins::parse(text).map_err(SyncError::Descriptor)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_plugins_are_read() {
        let d = ProjectDescriptor::parse(
            "[project]\nname = \"Star Fall\"\nversion = \"1.2.0\"\n\n[plugins]\n\"pulsar.voxel\" = false\n",
        )
        .unwrap();
        assert_eq!(d.name.as_deref(), Some("Star Fall"));
        assert_eq!(d.version.as_deref(), Some("1.2.0"));
        assert_eq!(d.plugins.listed.get("pulsar.voxel"), Some(&false));
        assert_eq!(ProjectDescriptor::parse("").unwrap(), ProjectDescriptor::default());
        assert!(ProjectDescriptor::parse("[project]\nname = 3\n").is_err());
    }
}
