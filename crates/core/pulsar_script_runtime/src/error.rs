//! The runtime's error type and its breakdown for an editor's problems list.

use std::path::PathBuf;

use pulsar_script_vm::{ErrorSite, LibraryError, LinkError, ScriptError, SourceLoc};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("no script class `{0}` is loaded")]
    UnknownClass(String),
    #[error("script class `{0}` is already loaded")]
    ClassLoaded(String),
    #[error("no script instance `{0}`")]
    UnknownInstance(String),
    #[error("script instance `{0}` already exists")]
    DuplicateInstance(String),
    #[error("`{class}::{name}` must be `{expected}` to be a lifecycle event")]
    BadEntryPoint {
        class: String,
        name: &'static str,
        expected: &'static str,
    },
    #[error("`{class}` has no exported function `{name}`")]
    UnknownEvent { class: String, name: String },
    #[error("variable `{name}`: {reason}")]
    BadVariable { name: String, reason: String },
    #[error("script class `{class}`: {source}{}", site_suffix(.site.as_ref()))]
    Link {
        class: String,
        source: LinkError,
        /// Where in the class the error is, when the module can tell
        /// (with debug info: the graph node).
        site: Option<ErrorSite>,
    },
    #[error("script instance `{object_id}` of `{class}`: {source}")]
    Script {
        object_id: String,
        class: String,
        source: ScriptError,
    },
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error("could not read `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("`{path}` is not a script module: {source}")]
    Parse {
        path: PathBuf,
        source: pulsar_script_vm::ModuleDecodeError,
    },
    #[error("script class `{class}` declares event `{event}`: {reason}")]
    EventDeclaration {
        class: String,
        event: String,
        reason: String,
    },
    #[error("migrating `{object_id}` of `{class}` failed, so the reload was refused: {source}")]
    Migration {
        class: String,
        object_id: String,
        source: ScriptError,
    },
    #[error("restoring saved state of `{object_id}`: {reason}")]
    State { object_id: String, reason: String },
}

fn site_suffix(site: Option<&ErrorSite>) -> String {
    site.map(|site| format!(" (at {site})")).unwrap_or_default()
}

/// A script error broken down for an editor's problems list:
/// which class, instance and function, and the source location debug info
/// gives (the Blueprint node).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorDetails {
    pub class: Option<String>,
    pub object_id: Option<String>,
    pub function: Option<String>,
    pub pc: Option<usize>,
    pub location: Option<SourceLoc>,
    /// The error itself, without the location.
    pub message: String,
}

impl RuntimeError {
    /// This error's class, instance, function and source location, where
    /// it has them.
    pub fn details(&self) -> ErrorDetails {
        match self {
            Self::Script {
                object_id,
                class,
                source,
            } => ErrorDetails {
                class: Some(class.clone()),
                object_id: Some(object_id.clone()),
                function: source.function().map(|(f, _)| f.to_owned()),
                pc: source.function().map(|(_, pc)| pc),
                location: source.location().cloned(),
                message: source.kind.to_string(),
            },
            Self::Link {
                class,
                source,
                site,
            } => ErrorDetails {
                class: Some(class.clone()),
                object_id: None,
                function: site.as_ref().map(|s| s.function.clone()),
                pc: site.as_ref().and_then(|s| s.pc),
                location: site.as_ref().and_then(|s| s.location.clone()),
                message: source.to_string(),
            },
            Self::UnknownClass(class) | Self::ClassLoaded(class) => ErrorDetails {
                class: Some(class.clone()),
                message: self.to_string(),
                ..Default::default()
            },
            Self::BadEntryPoint { class, name, .. } => ErrorDetails {
                class: Some(class.clone()),
                function: Some((*name).to_owned()),
                message: self.to_string(),
                ..Default::default()
            },
            Self::UnknownEvent { class, .. } | Self::EventDeclaration { class, .. } => {
                ErrorDetails {
                    class: Some(class.clone()),
                    message: self.to_string(),
                    ..Default::default()
                }
            }
            Self::UnknownInstance(id) | Self::DuplicateInstance(id) => ErrorDetails {
                object_id: Some(id.clone()),
                message: self.to_string(),
                ..Default::default()
            },
            _ => ErrorDetails {
                message: self.to_string(),
                ..Default::default()
            },
        }
    }
}
