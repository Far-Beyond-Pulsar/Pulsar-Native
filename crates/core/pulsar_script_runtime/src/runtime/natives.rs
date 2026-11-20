//! Registering natives and loading, reloading and unloading native libraries.

use std::path::Path;
use std::sync::Arc;

use pulsar_script_vm::{EventCatalog, LibraryError, LibraryId, NativeFn};

use super::ScriptRuntime;
use crate::class::Class;
use crate::reload::RelinkReport;
use crate::RuntimeError;

impl ScriptRuntime {
    /// Register an engine native, then relink every class.
    pub fn register_native(&mut self, native: NativeFn) -> Result<RelinkReport, RuntimeError> {
        self.natives
            .register(native)
            .map_err(|e| RuntimeError::Library(LibraryError::Duplicate(e)))?;
        Ok(self.relink_all())
    }

    pub fn load_library(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(LibraryId, RelinkReport), RuntimeError> {
        let id = self.libraries.load(path, &mut self.natives)?;
        Ok((id, self.relink_all()))
    }

    /// Load the current build of library `id` and relink every class
    /// against it. The previous build stays mapped for the rest of the
    /// process: values it created (objects in instance variables, event
    /// payloads) may outlive every program linked against it, see
    /// `pulsar_script_vm::library::ShadowLibrary`.
    pub fn reload_library(&mut self, id: LibraryId) -> Result<RelinkReport, RuntimeError> {
        self.libraries.reload(id, &mut self.natives)?;
        Ok(self.relink_all())
    }

    /// Remove library `id`'s natives and relink every class; classes that
    /// import them fail to relink. The library's code is not unmapped (as
    /// for [`reload_library`](Self::reload_library)).
    pub fn unload_library(&mut self, id: LibraryId) -> Result<RelinkReport, RuntimeError> {
        self.libraries.unload(id, &mut self.natives)?;
        Ok(self.relink_all())
    }

    pub(crate) fn relink_all(&mut self) -> RelinkReport {
        let mut report = RelinkReport::default();
        let catalog = self.events.as_deref().map(|e| e as &dyn EventCatalog);
        for (name, class) in &mut self.classes {
            match Class::link(
                Arc::clone(class.program.module()),
                &self.natives,
                catalog,
                &self.capabilities,
            ) {
                Ok(relinked) => {
                    *class = relinked;
                    report.relinked.push(name.clone());
                }
                Err(RuntimeError::Link { source, .. }) => {
                    tracing::error!("script class `{name}` failed to relink: {source}");
                    report.failed.push((name.clone(), source));
                }
                Err(other) => tracing::error!("script class `{name}` failed to relink: {other}"),
            }
        }
        report
    }
}
