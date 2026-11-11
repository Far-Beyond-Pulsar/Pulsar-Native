//! Finding, verifying and installing plugin libraries.
//!
//! Loading is two phases so the slow half can run on worker threads:
//! [`PluginManager::prepare_plugin`] loads a library and checks its SHA-256
//! against `plugin_integrity.json` (no plugin code runs), then
//! [`PluginManager::install_prepared_plugin`] runs the plugin and registers
//! what it provides (on the thread that owns the manager). Entry points for
//! callers are [`PluginManager::load_plugins_from_dir`] and
//! [`PluginManager::load_plugin`]; `order_prepared_plugins` puts dependencies
//! first.

use crate::attach::{attach_event_bus, attach_profiler, attach_world_runtime, read_manifest};
use crate::{
    dependencies, panel_restorers, LoadedPlugin, PermanentLibrary, PluginManager,
    PluginManagerError, PreparedPlugin,
};
use plugin_editor_api::*;
use std::path::{Path, PathBuf};

impl PluginManager {
    /// Load all plugins from a directory.
    ///
    /// This will scan the directory for dynamic libraries (.dll on Windows,
    /// .so on Linux, .dylib on macOS) and attempt to load each one as a plugin.
    ///
    /// Plugins that fail version checks or loading will be logged but won't
    /// prevent other plugins from loading.
    ///
    /// # Important
    ///
    /// Plugins are loaded ONCE and NEVER unloaded. This is intentional and
    /// necessary for safety. See `PLUGIN_ARCHITECTURE.md` for details.
    pub fn load_plugins_from_dir(
        &mut self,
        dir: impl AsRef<Path>,
        cx: &gpui::App,
    ) -> Result<(), PluginManagerError> {
        let dir = dir.as_ref();

        if !dir.exists() {
            tracing::warn!("Plugin directory does not exist: {:?}", dir);
            return Ok(());
        }

        tracing::info!("Loading plugins from: {:?}", dir);

        // Prepare every library, then install them dependencies first.
        for (path, prepared) in Self::prepare_plugins_in_dir(dir) {
            match prepared.and_then(|prepared| self.install_prepared_plugin(prepared, cx)) {
                Ok(plugin_id) => {
                    tracing::info!("✅ Successfully loaded plugin: {}", plugin_id);
                }
                Err(e) => {
                    tracing::error!("❌ Failed to load plugin from {:?}: {}", path, e);
                }
            }
        }

        Ok(())
    }

    /// Load a single plugin from a library file.
    ///
    /// # Safety
    ///
    /// This function loads and executes code from a dynamic library. The library
    /// must be compiled with the same Rust version and for the same engine version
    /// as the current build. These are checked at runtime.
    ///
    /// The loaded library will NEVER be unloaded. This is intentional and necessary
    /// to prevent undefined behavior.
    ///
    /// # Returns
    ///
    /// Returns the `PluginId` of the loaded plugin on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The library file cannot be loaded
    /// - Required symbols are missing
    /// - Version compatibility check fails
    /// - Plugin creation fails
    pub fn load_plugin(
        &mut self,
        path: impl AsRef<Path>,
        cx: &gpui::App,
    ) -> Result<PluginId, PluginManagerError> {
        let prepared = Self::prepare_plugin(path.as_ref())?;
        self.install_prepared_plugin(prepared, cx)
    }

    /// Every plugin library in `dir`, prepared in parallel on worker threads.
    /// This is the slow, thread-safe half of loading (OS library load, SHA-256
    /// of the file, integrity manifest); it touches no manager state and needs
    /// no UI. Finish each with [`PluginManager::install_prepared_plugin`], in
    /// the order returned: dependencies before the plugins that need them
    /// (see [`PluginManager::order_prepared_plugins`]).
    pub fn prepare_plugins_in_dir(
        dir: impl AsRef<Path>,
    ) -> Vec<(PathBuf, Result<PreparedPlugin, PluginManagerError>)> {
        let dir = dir.as_ref();
        if !dir.exists() {
            tracing::warn!("Plugin directory does not exist: {:?}", dir);
            return Vec::new();
        }
        #[cfg(target_os = "windows")]
        let extension = "dll";
        #[cfg(target_os = "linux")]
        let extension = "so";
        #[cfg(target_os = "macos")]
        let extension = "dylib";

        let paths: Vec<PathBuf> = walkdir::WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.into_path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(extension))
            .collect();
        let prepared = std::thread::scope(|scope| {
            let workers: Vec<_> = paths
                .iter()
                .map(|path| scope.spawn(move || Self::prepare_plugin(path)))
                .collect();
            paths
                .iter()
                .cloned()
                .zip(workers)
                .map(|(path, worker)| {
                    let result = worker.join().unwrap_or_else(|_| {
                        Err(PluginManagerError::PluginCreationFailed {
                            message: "plugin preparation panicked".to_string(),
                        })
                    });
                    (path, result)
                })
                .collect::<Vec<_>>()
        });
        Self::order_prepared_plugins(prepared)
    }

    /// Load the library at `path` and verify it against the integrity manifest.
    pub fn prepare_plugin(path: &Path) -> Result<PreparedPlugin, PluginManagerError> {
        // Resolve to an absolute path before handing it to the OS loader.
        // Windows `LoadLibraryExW` fails with ERROR_MOD_NOT_FOUND (126) for a
        // relative path (e.g. `plugins/editor\script_editor_plugin.dll`) even
        // when the file exists relative to the process CWD, while the absolute
        // form loads cleanly.
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        tracing::debug!("Loading plugin from: {:?}", path);

        // Load the library permanently
        let library =
            PermanentLibrary::new(&path).map_err(|e| PluginManagerError::LibraryLoadError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        // Verify the plugin binary against the required integrity manifest.
        // The manifest is a sibling JSON file named "plugin_integrity.json" that
        // maps plugin filenames to their expected SHA-256 hex digests.
        // If no manifest exists, plugins are rejected (manifest is mandatory).
        let plugin_dir = path.parent().unwrap_or(Path::new("."));
        let manifest = Self::load_plugin_manifest(plugin_dir)?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let expected_hex = manifest.get(file_name);
        if let Some(expected_hex) = expected_hex {
            let expected: [u8; 32] = Self::parse_hex_hash(expected_hex).map_err(|e| {
                PluginManagerError::LibraryLoadError {
                    path: path.to_path_buf(),
                    message: format!("Invalid hash in manifest for '{}': {}", file_name, e),
                }
            })?;
            if library.sha256() != &expected {
                return Err(PluginManagerError::IntegrityCheckFailed {
                    path: path.to_path_buf(),
                    expected: expected,
                    actual: *library.sha256(),
                });
            }
            tracing::debug!("Integrity check passed for plugin: {:?}", path);
        } else {
            // Plugin not listed in manifest — reject unless manifest explicitly
            // allows unlisted plugins via the special "allow_unlisted": true key.
            if !manifest
                .get("__allow_unlisted__")
                .map_or(false, |v| v == "true")
            {
                return Err(PluginManagerError::IntegrityCheckFailed {
                    path: path.to_path_buf(),
                    expected: [0u8; 32],
                    actual: *library.sha256(),
                });
            }
        }

        // The ABI gate. Nothing of the plugin's own may run, and no Rust-ABI
        // value may cross the library boundary, before the library has proven
        // it was built by this compiler for this engine: the manifest that
        // `order_prepared_plugins` reads next is an `Option<PluginManifest>`
        // laid out by the plugin's compiler. A stale build passes the
        // integrity check (its hash sits in an equally stale manifest), so
        // this check is what keeps it out.
        let version_fn: libloading::Symbol<extern "C" fn() -> VersionInfo> = unsafe {
            // SAFETY: `_plugin_version` is `extern "C" fn() -> VersionInfo`
            // (`#[repr(C)]`) in every library `export_plugin!` produced, and
            // the library is never unloaded. A library without the symbol is
            // rejected before anything else of it is called.
            library
                .get(b"_plugin_version")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_version".to_string(),
                    message: e.to_string(),
                })?
        };
        let plugin_version = version_fn();
        let engine_version = VersionInfo::current();
        if !engine_version.is_compatible(&plugin_version) {
            tracing::error!(
                "Plugin at {:?} was built for another engine or compiler (engine rustc hash {:#x}, plugin {:#x}); not loading it",
                path,
                engine_version.rustc_version_hash,
                plugin_version.rustc_version_hash,
            );
            return Err(PluginManagerError::VersionMismatch {
                expected: engine_version,
                actual: plugin_version,
            });
        }

        Ok(PreparedPlugin { path, library })
    }

    /// Run a prepared plugin's code and register what it provides. Runs on the
    /// thread that owns the manager (the plugin constructor takes the UI theme).
    ///
    /// Steps, in order (each depends on the ones before it):
    /// 1. [`Self::check_compatibility`]: engine version and plugin dependencies.
    /// 2. [`Self::attach_host_state`]: share the host's runtimes with the library.
    /// 3. [`Self::construct_plugin`]: run the plugin constructor and `on_load`.
    /// 4. The `register_*` / `collect_*` steps: copy what the plugin
    ///    provides into the manager's registries.
    /// 5. Store the plugin under its id.
    pub fn install_prepared_plugin(
        &mut self,
        prepared: PreparedPlugin,
        cx: &gpui::App,
    ) -> Result<PluginId, PluginManagerError> {
        let PreparedPlugin { path, library } = prepared;

        self.check_compatibility(&library, &path)?;
        Self::attach_host_state(&library, &path)?;
        let plugin = Self::construct_plugin(&library, &path, cx)?;

        // Get plugin metadata
        let metadata = plugin.metadata();
        let plugin_id = metadata.id.clone();

        tracing::info!(
            "📦 Loaded plugin: {} v{} by {}",
            metadata.name,
            metadata.version,
            metadata.author
        );

        // Call on_load hook
        plugin.on_load();

        // After load-time initialization we keep only an immutable static plugin ref.
        let plugin: &'static dyn EditorPluginFull = plugin;

        self.register_file_types_and_editors(plugin, &plugin_id);
        self.register_statusbar_buttons(plugin, &plugin_id);
        let subsystem_ids = self.collect_subsystems(plugin);
        self.collect_component_factories(plugin, &plugin_id);
        let editor_factories = Self::collect_editor_factories(plugin, &plugin_id);

        // Store the plugin
        // SAFETY: Both plugin reference and library handle have 'static lifetime
        // because the library is never unloaded
        let loaded_plugin = LoadedPlugin {
            plugin,
            library,
            metadata: metadata.clone(),
            editor_factories,
            subsystem_ids,
        };

        self.plugins.insert(plugin_id.clone(), loaded_plugin);

        Ok(plugin_id)
    }

    /// Step 1 of install: reject a library built for another engine/compiler,
    /// or one whose required plugins are not installed yet. Nothing of the
    /// plugin's own runs before this passes (only its version/manifest symbols).
    fn check_compatibility(
        &self,
        library: &PermanentLibrary,
        path: &Path,
    ) -> Result<(), PluginManagerError> {
        // Get the version info function
        let version_fn: libloading::Symbol<extern "C" fn() -> VersionInfo> = unsafe {
            // SAFETY: We're loading a symbol from the permanently loaded library.
            // The symbol must exist and have the correct signature.
            // If it doesn't, we'll get an error and reject the plugin.
            library
                .get(b"_plugin_version")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_version".to_string(),
                    message: e.to_string(),
                })?
        };

        // Check version compatibility
        let plugin_version = version_fn();

        tracing::debug!(
            "Version check - Engine: {:?}, Plugin: {:?}",
            self.engine_version,
            plugin_version
        );

        if !self.engine_version.is_compatible(&plugin_version) {
            tracing::error!(
                "Plugin version mismatch! Expected engine v{}.{}.{} (rustc hash {:#x}), got v{}.{}.{} (rustc hash {:#x})",
                self.engine_version.engine_version.0,
                self.engine_version.engine_version.1,
                self.engine_version.engine_version.2,
                self.engine_version.rustc_version_hash,
                plugin_version.engine_version.0,
                plugin_version.engine_version.1,
                plugin_version.engine_version.2,
                plugin_version.rustc_version_hash,
            );

            return Err(PluginManagerError::VersionMismatch {
                expected: self.engine_version,
                actual: plugin_version,
            });
        }

        tracing::debug!("✅ Version check passed for plugin at {:?}", path);

        // Every plugin this one requires must already be installed: that is
        // what `order_prepared_plugins` arranges, and it also covers a
        // dependency that was ordered first but failed to install.
        if let Some(manifest) = read_manifest(library)? {
            self.check_dependencies(&manifest)?;
        }

        Ok(())
    }

    /// Step 2 of install: share the host's world runtimes, profiler and event
    /// bus with the library. Must run before the plugin constructor so the
    /// constructor's registrations already land in the host's registries.
    fn attach_host_state(
        library: &PermanentLibrary,
        path: &Path,
    ) -> Result<(), PluginManagerError> {
        // Attach the plugin's world crates to the editor's world runtimes
        // before it runs any code of its own, so its component classes are
        // World components of the editor (#1083). A library built from other
        // world-crate sources or features refuses, and is not loaded.
        match attach_world_runtime(library) {
            Some(true) => tracing::debug!("Plugin at {:?} attached to the world runtimes", path),
            Some(false) => {
                return Err(PluginManagerError::WorldRuntimeRefused {
                    path: path.to_path_buf(),
                })
            }
            None => tracing::debug!(
                "Plugin at {:?} predates the shared world runtimes; its component classes stay local",
                path
            ),
        }

        // Share the editor's profiler so the plugin's scopes reach the
        // flamegraph; plugins built before this export nothing and keep a
        // private (never-read) profiler.
        match attach_profiler(library) {
            Some(true) => tracing::debug!("Plugin at {:?} attached to the host profiler", path),
            Some(false) => tracing::warn!("Plugin at {:?} refused the host profiler", path),
            None => {}
        }

        // Give the plugin the host's event bus before it runs any code of
        // its own, so its asset updates and events reach the editor (#930).
        match attach_event_bus(library) {
            Some(pulsar_events::host::ATTACH_OK) => {
                tracing::debug!("Plugin at {:?} attached to the host event bus", path)
            }
            Some(status) => tracing::warn!(
                "Plugin at {:?} refused the host event bus (status {status}); its events stay local",
                path
            ),
            None => tracing::debug!(
                "Plugin at {:?} predates the shared event bus; its events stay local",
                path
            ),
        }

        Ok(())
    }

    /// Step 3 of install: call the library's `_plugin_create` with the UI theme
    /// pointer. This is the first plugin code that runs.
    fn construct_plugin(
        library: &PermanentLibrary,
        path: &Path,
        cx: &gpui::App,
    ) -> Result<&'static mut dyn EditorPluginFull, PluginManagerError> {
        // Get the plugin constructor
        let create_fn: libloading::Symbol<PluginCreate> = unsafe {
            // SAFETY: Loading symbol from permanently loaded library.
            // The returned function pointer remains valid forever because
            // the library is never unloaded.
            library
                .get(b"_plugin_create")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_create".to_string(),
                    message: e.to_string(),
                })?
        };

        // Create the plugin instance with Theme pointer for cross-DLL global state sync
        let plugin = unsafe {
            // SAFETY: Calling the plugin constructor is safe because:
            // 1. We trust the plugin code (internal plugins only)
            // 2. We've verified version compatibility
            // 3. The returned &'static mut reference is valid because the library never unloads
            //
            // We pass the Theme pointer to sync global state across the DLL boundary.
            // The Theme must remain valid for the process lifetime (guaranteed by engine).
            let theme_ptr = ui::theme::Theme::global(cx) as *const _ as *const std::ffi::c_void;

            // Validate theme pointer before passing to plugin
            if theme_ptr.is_null() {
                return Err(PluginManagerError::PluginCreationFailed {
                    message: "Theme pointer is null (engine global state not initialized)"
                        .to_string(),
                });
            }

            create_fn(theme_ptr)
        };

        plugin.ok_or_else(|| PluginManagerError::PluginCreationFailed {
            message: format!("{:?} refused or panicked while being created", path),
        })
    }

    /// Step 4a of install: add the plugin's file types and editors to the
    /// manager's registries.
    fn register_file_types_and_editors(
        &mut self,
        plugin: &'static dyn EditorPluginFull,
        plugin_id: &PluginId,
    ) {
        // Register file types
        let file_types = plugin.file_types();
        for file_type in file_types {
            tracing::debug!(
                "  📄 Registering file type: {} (.{})",
                file_type.display_name,
                file_type.extension
            );
            self.file_type_registry
                .register(file_type, plugin_id.clone());
        }

        // Register editors
        let editors = plugin.editors();
        for editor in editors {
            tracing::debug!("  📝 Registering editor: {}", editor.display_name);
            self.editor_registry.register(editor, plugin_id.clone());
        }
    }

    /// Step 4b of install: add the plugin's statusbar buttons, keeping the
    /// list ordered left-then-right and by priority within each side.
    fn register_statusbar_buttons(
        &mut self,
        plugin: &'static dyn EditorPluginFull,
        plugin_id: &PluginId,
    ) {
        // Register statusbar buttons
        let statusbar_buttons = plugin.statusbar_buttons();
        if !statusbar_buttons.is_empty() {
            tracing::debug!(
                "  🔘 Registering {} statusbar buttons",
                statusbar_buttons.len()
            );
            for button in statusbar_buttons {
                tracing::debug!("    - Button: {} at {:?}", button.id, button.position);

                // Store with plugin ID for tracking
                // SAFETY: Function pointers in buttons remain valid because
                // the plugin library is never unloaded
                self.statusbar_buttons.push((plugin_id.clone(), button));
            }

            // Sort buttons by priority within their position groups
            self.statusbar_buttons.sort_by(|(_, a), (_, b)| {
                match (&a.position, &b.position) {
                    (StatusbarPosition::Left, StatusbarPosition::Left)
                    | (StatusbarPosition::Right, StatusbarPosition::Right) => {
                        b.priority.cmp(&a.priority) // Higher priority comes first
                    }
                    (StatusbarPosition::Left, StatusbarPosition::Right) => std::cmp::Ordering::Less,
                    (StatusbarPosition::Right, StatusbarPosition::Left) => {
                        std::cmp::Ordering::Greater
                    }
                }
            });
        }
    }

    /// Step 4c of install: queue the plugin's subsystems for the engine
    /// backend (see `drain_subsystems`); returns their ids for the details view.
    fn collect_subsystems(&mut self, plugin: &'static dyn EditorPluginFull) -> Vec<String> {
        // Collect plugin subsystems
        let subsystems = plugin.subsystems();
        let subsystem_ids: Vec<String> = subsystems.iter().map(|ss| ss.id().to_string()).collect();
        if !subsystems.is_empty() {
            tracing::debug!(
                "  🧩 Registering {} subsystem(s) from plugin",
                subsystems.len()
            );
            for ss in &subsystems {
                tracing::debug!("    - Subsystem: {}", ss.id());
            }
            self.plugin_subsystems.extend(subsystems);
        }
        subsystem_ids
    }

    /// Step 4d of install: queue the plugin's component factories for the
    /// engine backend (see `drain_component_registrations`), warning about
    /// classes that are not World components.
    fn collect_component_factories(
        &mut self,
        plugin: &'static dyn EditorPluginFull,
        plugin_id: &PluginId,
    ) {
        // Collect plugin component registrations
        let component_regs = plugin.component_factories();
        if !component_regs.is_empty() {
            tracing::debug!(
                "  🔧 Registering {} component(s) from plugin",
                component_regs.len()
            );
            for (name, _) in &component_regs {
                tracing::debug!("    - Component: {}", name);
            }
            // A plugin class is a World component only when its
            // `#[register_world_component]` registration reached the host's
            // registry: the class has one, and the plugin attached to the
            // editor's world runtimes (#1083). A plugin built before them
            // registered into its own copy's registry, which the host never
            // reads (#1081).
            let unregistered: Vec<&str> = component_regs
                .iter()
                .map(|(name, _)| name.as_str())
                .filter(|name| pulsar_world_registry::component_id_for_class(name).is_none())
                .collect();
            if !unregistered.is_empty() {
                tracing::warn!(
                    "plugin {} provides components that are not World components: {}. \
                     They have no #[register_world_component], or the plugin predates \
                     the shared world runtimes; they cannot be placed in a level.",
                    plugin_id,
                    unregistered.join(", ")
                );
            }
            self.plugin_component_registrations.extend(component_regs);
        }
    }

    /// Step 4e of install: gather the plugin's editor factories and merge its
    /// panel restorers into the host-wide set. The factories are kept on the
    /// [`LoadedPlugin`] and used by `create_editor`.
    fn collect_editor_factories(
        plugin: &'static dyn EditorPluginFull,
        plugin_id: &PluginId,
    ) -> EditorFactoryRegistry {
        // Collect editor factories from the plugin
        let mut editor_factories = EditorFactoryRegistry::new();
        EditorPluginEditor::register_editors(plugin, &mut editor_factories);
        if !editor_factories.factories().is_empty() {
            tracing::debug!(
                "  📝 Registering {} editor factories",
                editor_factories.factories().len()
            );
        }

        // Panel restorers ride the same registration call: merge them into the
        // host's global set so layout restore can rebuild this plugin's panels.
        for (kind, restorer) in editor_factories.take_restorers().take() {
            if panel_restorers().register_shared(kind.clone(), restorer) {
                tracing::warn!(
                    "plugin {} replaced the panel restorer for kind '{}'",
                    plugin_id,
                    kind
                );
            }
        }

        editor_factories
    }

    /// Fail unless every plugin `manifest` requires is installed in range.
    fn check_dependencies(&self, manifest: &PluginManifest) -> Result<(), PluginManagerError> {
        for dependency in &manifest.requires {
            let reason = match self.plugins.get(&dependency.id) {
                None => Some(format!(
                    "requires plugin {} ({}), which is not loaded",
                    dependency.id, dependency.requirement
                )),
                Some(loaded) => {
                    let installed = PluginManifest::from_metadata(&loaded.metadata);
                    (!plugin_editor_api::manifest::satisfies(&installed, dependency)).then(|| {
                        format!(
                            "requires plugin {} {}, found version {}",
                            dependency.id, dependency.requirement, installed.version
                        )
                    })
                }
            };
            if let Some(reason) = reason {
                return Err(PluginManagerError::DependencyUnmet {
                    plugin: manifest.id.clone(),
                    reason,
                });
            }
        }
        Ok(())
    }

    /// Put prepared plugins in the order they must be installed: plugins
    /// that export no manifest first (they cannot be depended on), then the
    /// rest with dependencies before dependents. A plugin whose dependencies
    /// cannot be met comes back as an error, last, with the reason.
    ///
    /// Reads manifests only; no plugin constructor runs.
    pub fn order_prepared_plugins(
        prepared: Vec<(PathBuf, Result<PreparedPlugin, PluginManagerError>)>,
    ) -> Vec<(PathBuf, Result<PreparedPlugin, PluginManagerError>)> {
        let mut unmanifested = Vec::new();
        let mut manifested = Vec::new();
        let mut manifests = Vec::new();
        let mut failed = Vec::new();
        for (path, result) in prepared {
            match result {
                Err(e) => failed.push((path, Err(e))),
                Ok(plugin) => match read_manifest(&plugin.library) {
                    Ok(Some(manifest)) => {
                        manifests.push(manifest);
                        manifested.push((path, plugin));
                    }
                    Ok(None) => unmanifested.push((path, Ok(plugin))),
                    Err(e) => {
                        tracing::error!("Plugin at {:?} not loaded: {}", path, e);
                        failed.push((path, Err(e)));
                    }
                },
            }
        }

        let plan = dependencies::plan_load_order(&manifests);
        let mut slots: Vec<Option<(PathBuf, PreparedPlugin)>> =
            manifested.into_iter().map(Some).collect();
        let mut ordered = unmanifested;
        for index in plan.order {
            if let Some((path, plugin)) = slots[index].take() {
                ordered.push((path, Ok(plugin)));
            }
        }
        for (index, error) in plan.rejected {
            if let Some((path, _library_stays_loaded)) = slots[index].take() {
                let manifest = &manifests[index];
                tracing::error!("Plugin {} not loaded: it {}", manifest.id, error);
                failed.push((
                    path,
                    Err(PluginManagerError::DependencyUnmet {
                        plugin: manifest.id.clone(),
                        reason: error.to_string(),
                    }),
                ));
            }
        }
        ordered.extend(failed);
        ordered
    }

    /// Load the plugin integrity manifest from a JSON file in the plugin directory.
    ///
    /// The manifest file must be named `plugin_integrity.json` and contains a flat
    /// JSON object mapping plugin filenames (e.g. `"my_editor.dll"`) to their
    /// expected SHA-256 hex digest strings.
    ///
    /// Special keys:
    /// - `"__allow_unlisted__": "true"` allows plugins not in the manifest to load
    ///   (use with caution).
    ///
    /// Returns `Err` if no manifest file exists (verification is required).
    fn load_plugin_manifest(
        plugin_dir: &Path,
    ) -> Result<std::collections::HashMap<String, String>, PluginManagerError> {
        let manifest_path = plugin_dir.join("plugin_integrity.json");
        if !manifest_path.exists() {
            return Err(PluginManagerError::LibraryLoadError {
                path: manifest_path,
                message: "plugin_integrity.json not found — manifest is required for security. "
                    .to_string(),
            });
        }
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| {
            PluginManagerError::LibraryLoadError {
                path: manifest_path.clone(),
                message: format!("Failed to read manifest: {}", e),
            }
        })?;
        let map: std::collections::HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| PluginManagerError::LibraryLoadError {
                path: manifest_path,
                message: format!("Invalid manifest JSON: {}", e),
            })?;
        Ok(map)
    }

    /// Parse a hex-encoded SHA-256 digest string into a 32-byte array.
    fn parse_hex_hash(hex: &str) -> Result<[u8; 32], String> {
        let hex = hex.trim();
        if hex.len() != 64 {
            return Err(format!("Expected 64 hex chars, got {}", hex.len()));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("Invalid hex at position {}: {}", i * 2, e))?;
        }
        Ok(out)
    }
}
