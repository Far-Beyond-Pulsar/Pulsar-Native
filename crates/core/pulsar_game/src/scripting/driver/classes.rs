//! Class registry upkeep and module loading: resolving class references,
//! loading compiled modules, hot-reloading a class when its asset changes.
//! Entry point: [`ScriptDriver::reload_class_for_asset`].

use engine_backend::scene::SceneWorldExt;
use std::sync::Arc;

use pulsar_class::{ClassEntry, ClassId, ClassInstance, ClassRegistry};
use pulsar_scenedb::{Entity, World};
use pulsar_script_runtime::{RuntimeError, ScriptRuntime};

use super::config::{module_file, read_module_bytes};
use super::reconcile::{depth, sort_roots};
use super::{DriverReport, ReloadRequests, ScriptDriver};
use crate::scripting::bind_class_slots;

impl ScriptDriver {
    pub(super) fn refresh_registry(&mut self) {
        if self.rescan_registry {
            self.registry = ClassRegistry::scan(&self.project_root);
            self.templates.clear();
            if let Some(events) = &self.events {
                for entry in self.registry.entries() {
                    events.bridge().add_class(&entry.name, entry.id.as_str());
                }
                self.predeclare_project_events();
            }
        }
    }

    /// The class `instance` names: by GUID, then by its name hint.
    pub(super) fn resolve(&mut self, instance: &ClassInstance) -> Option<ClassEntry> {
        if let Some(entry) = self.registry.resolve(instance) {
            return Some(entry.clone());
        }
        self.refresh_registry();
        self.registry.resolve(instance).cloned()
    }

    /// The class a script names: its GUID, or else its name.
    pub(super) fn resolve_class_ref(&mut self, class_ref: &str) -> Option<ClassEntry> {
        let find = |registry: &ClassRegistry| {
            registry
                .by_id(&ClassId::from(class_ref))
                .or_else(|| registry.by_name(class_ref))
                .cloned()
        };
        if let Some(entry) = find(&self.registry) {
            return Some(entry);
        }
        self.refresh_registry();
        find(&self.registry)
    }

    /// The runtime's name for `entry`'s script class, loading its module
    /// on first use. `None` when the class has no compiled script.
    pub(super) fn ensure_class_loaded(
        &mut self,
        entry: &ClassEntry,
        report: &mut DriverReport,
    ) -> Option<String> {
        if let Some(name) = self.loaded.get(&entry.id) {
            return Some(name.clone());
        }
        if self.runtime.has_class(&entry.name) {
            self.loaded.insert(entry.id.clone(), entry.name.clone());
            return Some(entry.name.clone());
        }
        let module = module_file(entry);
        let loaded = match read_module_bytes(&module) {
            None => {
                tracing::info!(
                    class = %entry.name,
                    module = %module.display(),
                    "Class has no compiled script module; nothing runs for it (compile the class)"
                );
                return None;
            }
            Some(Ok(bytes)) => self
                .runtime
                .load_class_bytes(&bytes, &module)
                .map(|(name, _)| name),
            Some(Err(source)) => Err(RuntimeError::Io {
                path: module.clone(),
                source,
            }),
        };
        match loaded {
            Ok(name) => {
                tracing::info!(class = %entry.name, module = %module.display(), "Loaded script class");
                self.loaded.insert(entry.id.clone(), name.clone());
                Some(name)
            }
            Err(error) => {
                let message = format!(
                    "script module of class '{}' did not load: {error}",
                    entry.name
                );
                tracing::warn!("{message}");
                report.failures.push(message);
                report.load_errors.push(error);
                None
            }
        }
    }

    /// Asset updates to apply at the start of the next frame. Share it with
    /// whatever receives asset events; see [`subscribe_class_reloads`](Self::subscribe_class_reloads).
    pub fn reload_requests(&self) -> ReloadRequests {
        Arc::clone(&self.reloads)
    }

    /// Queue every published Blueprint class update for the next frame's
    /// reload. The subscription never touches the driver or the world, so
    /// it is safe from any thread, even during the script phase.
    pub fn subscribe_class_reloads(&self) -> pulsar_events::AssetSubscription {
        let requests = self.reload_requests();
        pulsar_events::subscribe_asset_updates(
            Some(pulsar_events::AssetKind::Blueprint),
            move |event| {
                requests
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(event.clone());
            },
        )
    }

    /// Apply a class update now: reload the class's script module if the
    /// runtime has it (instances keep their state, see
    /// [`ScriptRuntime::reload_class`]), then bind the component slots of
    /// every live instance of the class again against its current
    /// components, and start the script of instances that had none (the
    /// class just got a compiled module). Returns the class name.
    pub fn reload_class_for_asset(
        &mut self,
        world: &World,
        event: &pulsar_events::AssetUpdated,
    ) -> Option<String> {
        let mut report = DriverReport::default();
        self.reload_class_for_asset_into(world, event, &mut report)
    }

    pub(super) fn reload_class_for_asset_into(
        &mut self,
        world: &World,
        event: &pulsar_events::AssetUpdated,
        report: &mut DriverReport,
    ) -> Option<String> {
        if event.kind != pulsar_events::AssetKind::Blueprint {
            return None;
        }
        // A class changed: what it declares and provides may have too.
        self.traits.clear();
        self.refresh_registry();
        let by_id = event
            .id
            .as_deref()
            .and_then(|id| self.registry.by_id(&ClassId::from(id)))
            .cloned();
        let by_path = || {
            let name = event.path.as_ref()?.file_name()?.to_str()?;
            self.registry.by_name(name).cloned()
        };
        let entry = by_id.or_else(by_path)?;

        if let Some(class) = self.loaded.get(&entry.id).cloned() {
            let module = module_file(&entry);
            if let Some(read) = read_module_bytes(&module) {
                let loaded = read
                    .map_err(|source| RuntimeError::Io {
                        path: module.clone(),
                        source,
                    })
                    .and_then(|bytes| self.runtime.load_class_bytes(&bytes, &module));
                match loaded {
                    Ok((name, reloaded)) => {
                        report.dropped_calls.extend(
                            reloaded
                                .dropped
                                .into_iter()
                                .map(|call| (name.clone(), call)),
                        );
                        if name != class {
                            // The module was renamed: follow it.
                            tracing::warn!(class = %entry.name, module = %name, "Class module name changed on reload");
                            self.loaded.insert(entry.id.clone(), name.clone());
                        }
                        // Its subscriptions may have changed.
                        self.resubscribe(Some(&name), Some(world));
                    }
                    Err(error) => {
                        // The old code keeps running.
                        tracing::warn!(class = %entry.name, "Class updated but its script module did not reload: {error}");
                        report.load_errors.push(error);
                        return None;
                    }
                }
            }
        }

        let mut roots: Vec<(usize, String, Entity)> = self
            .tracked
            .iter()
            .filter(|(_, tracked)| tracked.class == entry.id)
            .map(|(entity, _)| {
                let stable = world.stable_id_of(*entity).unwrap_or_default().to_owned();
                (depth(world, *entity), stable, *entity)
            })
            .collect();
        sort_roots(&mut roots);
        for (_, _, root) in roots {
            match self.tracked.get(&root).and_then(|t| t.instance.clone()) {
                Some(id) => {
                    bind_class_slots(&mut self.runtime, &id, world, root);
                }
                None => {
                    if let Some(instance) = pulsar_class::world::class_instance_of(world, root) {
                        self.try_spawn(world, root, &instance.variable_overrides, &entry, report);
                    }
                }
            }
        }
        Some(entry.name)
    }
}
