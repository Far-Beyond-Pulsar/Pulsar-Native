//! Loading, querying and hot-reloading script classes.

use std::path::Path;
use std::sync::Arc;

use pulsar_scenedb::World;
use pulsar_script_vm::{Module, Type, Value};

use super::ScriptRuntime;
use crate::class::Class;
use crate::reload::{carry_state, Carried, DroppedCall, ReloadReport};
use crate::RuntimeError;

impl ScriptRuntime {
    /// Link and add a class, named by its module name.
    pub fn load_class(&mut self, module: Module) -> Result<(), RuntimeError> {
        if self.classes.contains_key(&module.name) {
            return Err(RuntimeError::ClassLoaded(module.name));
        }
        self.declare_events(&module)?;
        let class = Class::link(
            Arc::new(module),
            &self.natives,
            self.catalog(),
            &self.capabilities,
        )?;
        self.classes
            .insert(class.program.module().name.clone(), class);
        Ok(())
    }

    /// Read a module, JSON or binary (detected from its first bytes),
    /// and load it (or reload it, if already loaded).
    pub fn load_class_file(&mut self, path: impl AsRef<Path>) -> Result<String, RuntimeError> {
        self.load_class_file_reporting(path).map(|(name, _)| name)
    }

    /// [`load_class_file`](Self::load_class_file) from bytes already read
    /// (from a pak, say); `origin` names them in errors.
    pub fn load_class_bytes(
        &mut self,
        bytes: &[u8],
        origin: &Path,
    ) -> Result<(String, ReloadReport), RuntimeError> {
        let module = Module::decode(bytes).map_err(|source| RuntimeError::Parse {
            path: origin.to_owned(),
            source,
        })?;
        let name = module.name.clone();
        let report = if self.classes.contains_key(&name) {
            self.reload_class(module)?
        } else {
            self.load_class(module)?;
            ReloadReport::default()
        };
        Ok((name, report))
    }

    /// [`load_class_file`](Self::load_class_file), also returning what a
    /// reload did with waiting calls (empty for a first load).
    pub fn load_class_file_reporting(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(String, ReloadReport), RuntimeError> {
        let path = path.as_ref();
        let bytes = engine_fs::virtual_fs::read_file(path).map_err(|e| RuntimeError::Io {
            path: path.to_owned(),
            source: std::io::Error::other(e.to_string()),
        })?;
        self.load_class_bytes(&bytes, path)
    }

    pub fn has_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// The variables a loaded class declares, as `(name, type)`, in slot
    /// order. Hosts use it to find variables they fill at bind time (e.g.
    /// the hidden `__slot:<uuid>` component handles of class instances).
    pub fn class_variables(&self, class: &str) -> Option<Vec<(String, Type)>> {
        let module = self.classes.get(class)?.program.module();
        Some(
            module
                .variables
                .iter()
                .map(|v| (v.name.clone(), v.ty.clone()))
                .collect(),
        )
    }

    /// Swap a loaded class's code. Instances keep their identity, binding
    /// and every variable [`migrate::plan`](pulsar_script_vm::migrate::plan) matches (by stable id, else by
    /// name) to a variable of the same type; new, retyped and unmatched
    /// variables are reported in [`ReloadReport::variables`]. When the new
    /// module raises [`Module::class_version`] and exports `migrate`, it
    /// runs once per instance on the new state (see
    /// [`pulsar_script_vm::migrate`]).
    ///
    /// Waiting calls continue in the new code when their functions' layout
    /// is unchanged (see `Continuation::rebase`), and are dropped otherwise,
    /// listed in the report.
    ///
    /// The reload is all-or-nothing: every instance's new state is built
    /// first, and the code, event declarations and state are committed
    /// together only if all of that succeeds. On error the old class keeps
    /// running untouched.
    pub fn reload_class(&mut self, module: Module) -> Result<ReloadReport, RuntimeError> {
        let name = module.name.clone();
        if !self.classes.contains_key(&name) {
            return Err(RuntimeError::UnknownClass(name));
        }
        // Events are declared before linking: the class's own handlers link
        // against the host's catalog. The host cannot retract a declaration,
        // so a reload that fails after this leaves any newly declared events
        // registered (harmless; changed re-declarations are refused anyway);
        // class state, code and waiting calls are all-or-nothing.
        self.declare_events(&module)?;
        let new = Class::link(
            Arc::new(module),
            &self.natives,
            self.catalog(),
            &self.capabilities,
        )?;
        let old = &self.classes[&name];
        let old_module = Arc::clone(old.program.module());

        // Stage every instance; nothing is touched until all of them work.
        let mut scratch = World::new();
        let mut staged = Vec::new();
        for (object_id, instance) in self.instances.iter().filter(|(_, i)| i.class == name) {
            let old_values: Vec<Option<Value>> = (0..old_module.variables.len())
                .map(|i| old.program.var(&instance.state, i).cloned())
                .collect();
            let carried = carry_state(
                &new,
                object_id,
                &old_module.variables,
                old_module.class_version,
                &old_values,
                &mut scratch,
            )?;
            // Suspended calls ran the old code: they continue in the new
            // code where its layout is compatible, see
            // `Continuation::rebase`; the others are dropped.
            let waiting = instance
                .waiting
                .iter()
                .map(|(wake, continuation)| (*wake, continuation.rebase(new.program.module())))
                .collect::<Vec<_>>();
            staged.push((object_id.clone(), carried, waiting));
        }

        let mut report = ReloadReport::default();
        let mut migrated = 0;
        for (
            object_id,
            Carried {
                state,
                changes,
                kept,
            },
            waiting,
        ) in staged
        {
            let instance = self
                .instances
                .get_mut(&object_id)
                .expect("staged instances exist");
            let previous = std::mem::take(&mut instance.waiting);
            instance.state = state;
            for ((wake, rebased), (_, original)) in waiting.into_iter().zip(previous) {
                match rebased {
                    Ok(continuation) => {
                        instance.waiting.push((wake, continuation));
                        report.kept += 1;
                    }
                    Err(reason) => {
                        let function = original
                            .functions()
                            .last()
                            .map(|f| f.to_string())
                            .unwrap_or_default();
                        tracing::warn!(
                            class = %name,
                            function = %function,
                            "reload dropped a waiting script call of `{name}::{function}`: {reason}"
                        );
                        report.dropped.push(DroppedCall {
                            object_id: object_id.clone(),
                            function,
                            reason,
                        });
                    }
                }
            }
            report.variables_kept += kept;
            report.variables.extend(changes);
            migrated += 1;
        }
        tracing::info!(class = %name, instances = migrated, kept_waiting = report.kept, "reloaded script class");
        self.classes.insert(name.clone(), new);
        self.carry_latent_across_reload(&name, &mut report);
        Ok(report)
    }
}
