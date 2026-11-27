//! Cooking a project's content: compiles its scripts and builds every file the
//! packaged game's `Content/` holds (classes, levels, assets, settings), the
//! steps 1 and 2 of [`crate::package`]. Writing the files out is the crate
//! root's job.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use pulsar_class::{ClassIndex, ClassRegistry};
use pulsar_content::{AssetRegistry, ProjectSettings, ASSET_REGISTRY_FILE, PROJECT_SETTINGS_FILE};

use crate::{
    compile, cook, default_engine_assets, project_settings, PackageError, PackageOptions,
    PackageReport,
};

/// Every asset the dependency walk cooked, by asset id, with its source file.
type CookedAssets = BTreeMap<String, (PathBuf, plugin_editor_api::file_types::CookOutput)>;

/// Scripts and cook (steps 1 and 2 of [`crate::package`]): every file the game's
/// content holds, content-relative, with the asset registry.
pub(crate) fn cook_content(
    project: &Path,
    options: &PackageOptions,
    report: &mut PackageReport,
) -> Result<BTreeMap<String, Vec<u8>>, PackageError> {
    let mut settings = project_settings(project, options.profile)?;
    // The project's plugins, as its game links them: what is cooked (and
    // which plugin folders are snapshotted) follows the project's list.
    pulsar_plugins::activate_project(project, engine_state::project_plugins(project));
    let file_types = plugin_manager::builtin::linked_file_type_definitions();

    // 1. Scripts.
    let compiled = compile_scripts(project, &settings, options, report)?;
    let registry = ClassRegistry::scan(project);

    // 2. Cook.
    let engine_assets = options.engine_assets.clone().or_else(default_engine_assets);
    let mut cook = Cook {
        project,
        options,
        report,
        resolver: cook::AssetResolver::new(project, engine_assets),
        assets: cook::AssetSet::default(),
        files: BTreeMap::new(),
        selected_pipelines: BTreeMap::new(),
    };
    cook.cook_classes(&compiled.classes)?;
    cook.add_class_index_and_scripting(&registry)?;
    cook.add_plugin_snapshot();
    let game_instance = cook.game_instance(&settings, &registry, &compiled.classes)?;
    let startup = cook.startup_level(&settings);
    cook.cook_levels(&registry, &startup)?;
    // A game instance draws its menu before the first level is in, and a project
    // may start with no level: both run the fallback pipeline until one loads.
    if game_instance.is_some() || !matches!(startup, Startup::Level(_)) {
        cook.add_fallback_pipeline();
    }

    let mut cooked_assets = BTreeMap::new();
    collect_asset_dependencies(
        project,
        options.optimization,
        &file_types,
        &cook.resolver,
        &mut cook.assets,
        &mut cooked_assets,
    )?;
    cook.warn_unresolved_assets();
    // Cooked content names the game instance by its class id.
    settings.game_instance = game_instance;
    cook.add_project_settings(&mut settings, &startup);

    let (mut registry_out, mut asset_bytes) = cook.cook_assets(&cooked_assets)?;
    cook.add_selected_pipelines(&file_types, &mut registry_out, &mut asset_bytes)?;
    cook.finish(registry_out, asset_bytes)
}

/// Compile the project's scripts; any error stops packaging, warnings go to
/// the report.
fn compile_scripts(
    project: &Path,
    settings: &ProjectSettings,
    options: &PackageOptions,
    report: &mut PackageReport,
) -> Result<compile::CompileOutput, PackageError> {
    let compiled = compile::compile_project(project, settings, options.optimization);
    if compiled.has_errors() {
        return Err(PackageError::Scripts(compiled.problems));
    }
    report
        .warnings
        .extend(compiled.problems.iter().map(ToString::to_string));
    Ok(compiled)
}

fn json_bytes(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// A class's directory relative to the project (`src/classes/Door`, or a
/// `Door.class/` folder anywhere in the project).
fn class_dir(project: &Path, entry: &pulsar_class::ClassEntry) -> String {
    entry
        .dir
        .strip_prefix(project)
        .ok()
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|rel| !rel.is_empty())
        .unwrap_or_else(|| format!("src/classes/{}", entry.name))
}

/// The level the game starts in.
#[derive(Debug)]
enum Startup {
    /// This level, project-relative.
    Level(String),
    /// The project says none.
    None,
    /// The project says nothing and has no default level.
    Missing,
}

/// What one cook accumulates: the shipped files, the assets they reference
/// and the render pipelines the levels select.
struct Cook<'a> {
    project: &'a Path,
    options: &'a PackageOptions,
    report: &'a mut PackageReport,
    resolver: cook::AssetResolver,
    assets: cook::AssetSet,
    files: BTreeMap<String, Vec<u8>>,
    selected_pipelines: BTreeMap<String, PathBuf>,
}

impl Cook<'_> {
    fn cook_classes(&mut self, classes: &[compile::CompiledClass]) -> Result<(), PackageError> {
        for class in classes {
            self.cook_class(class)?;
        }
        Ok(())
    }

    fn cook_class(&mut self, class: &compile::CompiledClass) -> Result<(), PackageError> {
        let entry = &class.entry;
        let dir = class_dir(self.project, entry);
        self.files.insert(
            format!("{dir}/{}", pulsar_class::CLASS_META_FILE),
            json_bytes(&serde_json::json!({ "class_id": entry.id.as_str() })),
        );
        if virtual_fs::is_file(&entry.dir.join(pulsar_class::PREFAB_FILE)) {
            self.cook_prefab(entry, &dir)?;
        }
        if let Some(module) = &class.module {
            // Keyed by the class id, where the game looks
            // (`pulsar_class::modules::module_file`).
            self.files.insert(
                pulsar_class::modules::cooked_module_path(&entry.id),
                module.to_binary(),
            );
        }
        self.report.classes.push((
            entry.name.clone(),
            entry.id.as_str().to_owned(),
            class.module.is_some(),
        ));
        Ok(())
    }

    fn cook_prefab(
        &mut self,
        entry: &pulsar_class::ClassEntry,
        dir: &str,
    ) -> Result<(), PackageError> {
        // Slot ids are assigned by the load; cooking repairs the project's
        // prefab so the cooked ids match the project's.
        let (prefab, outcome) =
            pulsar_class::PrefabAsset::load_and_repair(&entry.dir).map_err(PackageError::Cook)?;
        if let pulsar_class::prefab::RepairOutcome::Failed(e) = outcome {
            tracing::warn!(class = %entry.name, "slot ids not saved: {e}");
        }
        let mut value =
            serde_json::to_value(&prefab).map_err(|e| PackageError::Cook(e.to_string()))?;
        if let Some(class_ref) = value
            .get_mut("blueprint_class")
            .and_then(|v| v.as_object_mut())
        {
            class_ref.insert(
                "class_path".into(),
                serde_json::Value::String(dir.to_owned()),
            );
        }
        cook::rewrite_assets(
            &mut value,
            &self.resolver,
            &mut self.assets,
            &format!("{dir}/prefab.json"),
        );
        cook::relativize_project_paths(&mut value, self.project);
        self.files.insert(
            format!("{dir}/{}", pulsar_class::PREFAB_FILE),
            json_bytes(&value),
        );
        Ok(())
    }

    /// The class GUID index and the scripting config.
    fn add_class_index_and_scripting(
        &mut self,
        registry: &ClassRegistry,
    ) -> Result<(), PackageError> {
        self.files.insert(
            pulsar_class::CLASS_INDEX_FILE.into(),
            ClassIndex::from_registry(registry, self.project)
                .to_json()
                .into_bytes(),
        );
        if let Some(config) =
            cook::cook_scripting_config(self.project, registry).map_err(PackageError::Cook)?
        {
            self.files.insert(
                pulsar_game::scripting::SCRIPTING_CONFIG_FILE.into(),
                json_bytes(&config),
            );
        }
        Ok(())
    }

    /// The plugins' folders (`plugins/<id>/`): a snapshot of what each
    /// enabled plugin provides now, which the game serves read-only
    /// (`engine_fs::virtual_fs::plugin_snapshot`), so the game does not depend
    /// on where a plugin got its files in the editor.
    fn add_plugin_snapshot(&mut self) {
        for (rel, bytes) in virtual_fs::plugin_snapshot(self.project) {
            self.files.insert(rel, bytes);
        }
    }

    /// The game instance the project names, checked and as cooked content
    /// names it (its class id). A shipped game ships no trait index it could
    /// check it against, so a class that is not a Game Instance fails here.
    fn game_instance(
        &mut self,
        settings: &ProjectSettings,
        registry: &ClassRegistry,
        compiled: &[compile::CompiledClass],
    ) -> Result<Option<String>, PackageError> {
        let Some(class) = settings.game_instance_class() else {
            return Ok(None);
        };
        let problem = |why: String| {
            Err(PackageError::Project(format!(
                "game instance `{class}` (Pulsar/project.json `game_instance`) {why}"
            )))
        };
        let Some(entry) = registry
            .by_asset(self.project, class)
            .or_else(|| registry.by_id(&pulsar_class::ClassId::from(class)))
            .or_else(|| registry.by_name(class))
        else {
            return problem("is not a class of this project".into());
        };
        if !compiled
            .iter()
            .any(|c| c.entry.id == entry.id && c.module.is_some())
        {
            return problem("has no compiled script".into());
        }
        let dir = class_dir(self.project, entry);
        let index = pulsar_traits::TraitIndex::load(self.project)
            .map_err(|e| PackageError::Project(format!("the trait index: {e:#}")))?;
        let implements = index
            .implementers_of(pulsar_traits::builtin::GAME_INSTANCE_TRAIT_ID)
            .iter()
            .any(|implementer| {
                matches!(&implementer.binding,
                    Some(pulsar_traits::Binding::ScriptClass { dir: found })
                        if found.trim_matches('/').eq_ignore_ascii_case(&dir))
            });
        if !implements {
            return problem(
                "does not implement the Game Instance trait: add it in the Blueprint's Traits panel"
                    .into(),
            );
        }
        Ok(Some(entry.id.as_str().to_owned()))
    }

    /// The pipeline a game with no level runs (a menu before any level), when
    /// an enabled plugin offers one. Plugin files are in the snapshot already;
    /// selecting it records it as one the game needs.
    fn add_fallback_pipeline(&mut self) {
        use engine_backend::subsystems::render::helio_renderer::pipelines;
        let Some(id) = pipelines::fallback_pipeline() else {
            self.report.warnings.push(
                "no enabled plugin offers a pipeline for a game with no level: its widgets are \
                 not drawn until a level loads"
                    .into(),
            );
            return;
        };
        let source = self.project.join(&id);
        self.selected_pipelines.entry(id).or_insert(source);
    }

    /// The startup level (project-relative): the option, then the project's
    /// own choice; an empty one is none, an absent one the default level.
    fn startup_level(&self, settings: &ProjectSettings) -> Startup {
        let project = self.project;
        let chosen = match &self.options.startup_level {
            Some(option) => Some(option.as_str()),
            None => settings.startup_level.as_deref(),
        };
        match chosen.map(str::trim) {
            Some("") => Startup::None,
            Some(level) => Startup::Level(level.replace('\\', "/")),
            None => pulsar_content::default_level(default_map(project).as_deref(), |rel| {
                virtual_fs::is_file(&project.join(rel))
            })
            .map_or(Startup::Missing, Startup::Level),
        }
    }

    /// Levels: every `.level` file, plus the startup level.
    fn cook_levels(
        &mut self,
        registry: &ClassRegistry,
        startup: &Startup,
    ) -> Result<(), PackageError> {
        let mut levels = cook::find_levels(self.project, &[self.options.out.clone()]);
        match startup {
            Startup::Level(startup) => {
                if !virtual_fs::is_file(&self.project.join(startup)) {
                    return Err(PackageError::Project(format!(
                        "startup level {startup} does not exist"
                    )));
                }
                levels.insert(startup.clone());
            }
            // The project's choice: nothing to report.
            Startup::None => {}
            Startup::Missing => self
                .report
                .warnings
                .push("no startup level: the game starts with no level".into()),
        }
        for rel in &levels {
            self.cook_level(registry, rel)?;
        }
        Ok(())
    }

    fn cook_level(&mut self, registry: &ClassRegistry, rel: &str) -> Result<(), PackageError> {
        let path = self.project.join(rel);
        let bytes =
            virtual_fs::read_file(&path).map_err(|e| PackageError::Cook(format!("{rel}: {e}")))?;
        let level_text = std::str::from_utf8(&bytes)
            .map_err(|e| PackageError::Cook(format!("{rel}: level is not UTF-8: {e}")))?;
        if let Some(pipeline) = selected_pipeline_for_cook(level_text)? {
            self.selected_pipelines
                .entry(pipeline.clone())
                .or_insert_with(|| self.project.join(&pipeline));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| PackageError::Cook(format!("{rel}: {e}")))?;
        let (cooked, level_report) =
            cook::cook_level(value, registry, &self.resolver, &mut self.assets, rel);
        for class in level_report.unresolved_classes {
            self.report
                .warnings
                .push(format!("{rel}: placed class {class} is not in the project"));
        }
        let rel = pulsar_content::normalize_rel(rel)
            .ok_or_else(|| PackageError::Cook(format!("bad level path {rel}")))?;
        self.files.insert(rel.clone(), json_bytes(&cooked));
        self.report.levels.push(rel);
        Ok(())
    }

    fn warn_unresolved_assets(&mut self) {
        for (context, value) in &self.assets.unresolved {
            self.report
                .warnings
                .push(format!("{context}: asset `{value}` not found; not shipped"));
        }
    }

    /// Project settings, cooked.
    fn add_project_settings(&mut self, settings: &mut ProjectSettings, startup: &Startup) {
        settings.profile = self.options.profile;
        settings.startup_level = match startup {
            // Explicitly none: the game must not fall back to a default level.
            Startup::None => Some(String::new()),
            Startup::Level(level) => pulsar_content::normalize_rel(level),
            Startup::Missing => None,
        };
        if settings.name.is_empty() {
            settings.name = project_name(self.project);
        }
        self.report.startup_level = settings.startup_level.clone().filter(|l| !l.is_empty());
        self.files.insert(
            PROJECT_SETTINGS_FILE.into(),
            settings.to_json().into_bytes(),
        );
    }

    /// The assets' cooked files, by output id, with the registry describing
    /// them.
    fn cook_assets(
        &self,
        cooked_assets: &CookedAssets,
    ) -> Result<(AssetRegistry, BTreeMap<String, Vec<u8>>), PackageError> {
        let mut registry_out = AssetRegistry::new();
        let mut asset_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (id, source) in &self.assets.assets {
            let cooked = cooked_assets
                .get(id)
                .map(|(_, cooked)| cooked)
                .expect("the dependency walk cooks every reachable asset");
            for artifact in &cooked.files {
                let path_text = artifact.path.to_string_lossy().replace('\\', "/");
                let output_id = pulsar_content::normalize_rel(&path_text).ok_or_else(|| {
                    PackageError::Cook(format!("asset {id}: invalid cooked path `{path_text}`"))
                })?;
                if !output_id.starts_with("assets/") {
                    return Err(PackageError::Cook(format!(
                        "asset {id}: cooked output `{output_id}` must be under assets/"
                    )));
                }
                if asset_bytes.contains_key(&output_id) {
                    return Err(PackageError::Cook(format!(
                        "asset {id}: cooked output `{output_id}` collides with another asset"
                    )));
                }
                registry_out.insert(cook::asset_record(
                    &output_id,
                    source,
                    self.project,
                    &artifact.bytes,
                ));
                asset_bytes.insert(output_id, artifact.bytes.clone());
            }
        }
        Ok((registry_out, asset_bytes))
    }

    /// A level's render graph is a hard file dependency, just like its placed
    /// classes. Preserve its path because the runtime pipeline registry uses
    /// that same project-relative id (including plugin-folder mounts).
    fn add_selected_pipelines(
        &mut self,
        file_types: &[plugin_editor_api::FileTypeDefinition],
        registry_out: &mut AssetRegistry,
        asset_bytes: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<(), PackageError> {
        let pipelines = std::mem::take(&mut self.selected_pipelines);
        for (id, source) in &pipelines {
            let artifact_bytes = self.pipeline_bytes(file_types, id, source)?;
            self.files.insert(id.clone(), artifact_bytes.clone());
            asset_bytes.insert(id.clone(), artifact_bytes.clone());
            registry_out.insert(cook::asset_record(
                id,
                source,
                self.project,
                &artifact_bytes,
            ));
        }
        Ok(())
    }

    /// The cooked bytes of one selected render pipeline.
    fn pipeline_bytes(
        &self,
        file_types: &[plugin_editor_api::FileTypeDefinition],
        id: &str,
        source: &Path,
    ) -> Result<Vec<u8>, PackageError> {
        // Plugin-provided pipelines are already present in the snapshot above.
        // Some providers expose their generated files through that snapshot
        // without making them addressable through the project VFS path.
        if let Some(bytes) = self.files.get(id) {
            return Ok(bytes.clone());
        }
        if !virtual_fs::is_file(source) {
            return Err(PackageError::Cook(format!(
                "selected render pipeline `{id}` is unavailable to the packager; enable its provider plugin or keep the .rendergraph in the project"
            )));
        }
        let handler = cook_handler_for_path(file_types, source)
            .map_err(|e| PackageError::Cook(format!("render pipeline {id}: {e}")))?;
        let context = plugin_editor_api::CookContext {
            project_root: self.project,
            source_path: source,
            content_path: Path::new(id),
            optimization: self.options.optimization,
        };
        let cooked = handler(&context)
            .map_err(|e| PackageError::Cook(format!("render pipeline {id}: {e}")))?;
        if cooked.files.len() != 1 {
            return Err(PackageError::Cook(format!(
                "render pipeline {id}: its cook handler must emit exactly one graph file"
            )));
        }
        let artifact = cooked.files.into_iter().next().expect("one graph artifact");
        let path_text = artifact.path.to_string_lossy().replace('\\', "/");
        let output_id = pulsar_content::normalize_rel(&path_text).ok_or_else(|| {
            PackageError::Cook(format!(
                "render pipeline {id}: invalid cooked path `{path_text}`"
            ))
        })?;
        if output_id != *id {
            return Err(PackageError::Cook(format!(
                "render pipeline {id}: its cook handler changed the runtime path to `{output_id}`"
            )));
        }
        Ok(artifact.bytes)
    }

    /// Add the assets and the asset registry, then refuse any file that names
    /// a machine path.
    fn finish(
        mut self,
        registry_out: AssetRegistry,
        asset_bytes: BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeMap<String, Vec<u8>>, PackageError> {
        self.report.assets = asset_bytes.len();
        self.files.extend(asset_bytes);
        self.files.insert(
            ASSET_REGISTRY_FILE.into(),
            registry_out.to_json().into_bytes(),
        );

        for (rel, bytes) in &self.files {
            if let Some(problem) = machine_path_in(rel, bytes, self.project) {
                return Err(PackageError::AbsolutePaths(vec![problem]));
            }
        }
        Ok(self.files)
    }
}

/// Follow each asset type's direct references until the reachable asset set is
/// exhausted. The queue and visited ids make arbitrarily deep graphs and
/// cycles safe without a recursion limit.
fn collect_asset_dependencies(
    project: &Path,
    optimization: plugin_editor_api::CookOptimization,
    file_types: &[plugin_editor_api::FileTypeDefinition],
    resolver: &cook::AssetResolver,
    assets: &mut cook::AssetSet,
    cooked_assets: &mut CookedAssets,
) -> Result<(), PackageError> {
    collect_asset_dependencies_with(resolver, assets, |id, source| {
        let context = plugin_editor_api::CookContext {
            project_root: project,
            source_path: source,
            content_path: Path::new(id),
            optimization,
        };
        let handler = cook_handler_for_path(file_types, source)
            .map_err(|error| PackageError::Cook(format!("asset {id}: {error}")))?;
        let output = handler(&context)
            .map_err(|error| PackageError::Cook(format!("asset {id}: {error}")))?;
        if output.files.is_empty() {
            return Err(PackageError::Cook(format!(
                "asset {id}: its registered cook handler emitted no files"
            )));
        }
        let dependencies = output.dependencies.clone();
        cooked_assets.insert(id.to_owned(), (source.to_path_buf(), output));
        Ok(dependencies)
    })
}

fn collect_asset_dependencies_with(
    resolver: &cook::AssetResolver,
    assets: &mut cook::AssetSet,
    mut direct_dependencies: impl FnMut(&str, &Path) -> Result<Vec<String>, PackageError>,
) -> Result<(), PackageError> {
    let mut pending: VecDeque<String> = assets.assets.keys().cloned().collect();
    let mut visited = BTreeSet::new();

    while let Some(id) = pending.pop_front() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(source) = assets.assets.get(&id).cloned() else {
            continue;
        };
        let dependencies = direct_dependencies(&id, &source)?;

        for dependency in dependencies {
            match resolver.resolve(&dependency) {
                Some((dependency_id, dependency_source)) => {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        assets.assets.entry(dependency_id.clone())
                    {
                        entry.insert(dependency_source);
                        pending.push_back(dependency_id);
                    }
                }
                None => assets.unresolved.push((format!("asset {id}"), dependency)),
            }
        }
    }
    Ok(())
}

/// The selected graph is stored in the level as a path, not as one of the
/// generic asset-path properties. Resolve plugin aliases and old pipeline
/// ids using the same migration path as the runtime.
fn selected_pipeline_for_cook(level_json: &str) -> Result<Option<String>, PackageError> {
    let level: serde_json::Value = serde_json::from_str(level_json)
        .map_err(|error| PackageError::Cook(format!("invalid level JSON: {error}")))?;
    let selected = level
        .get("world_settings")
        .and_then(|settings| settings.get("render_pipeline"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty());
    // A missing/null setting is not a direct file reference. Legacy implicit
    // defaults remain supplied by the enabled plugin snapshot.
    let Some(_) = selected else {
        return Ok(None);
    };
    let pipeline =
        engine_backend::subsystems::render::helio_renderer::pipelines::render_pipeline_of_level(
            level_json,
        )
        .ok_or_else(|| {
            PackageError::Cook("could not read the level's selected render pipeline".into())
        })?;
    let pipeline = virtual_fs::canonical_mount_path(&pipeline);
    let pipeline = pulsar_content::normalize_rel(&pipeline).ok_or_else(|| {
        PackageError::Cook(format!(
            "selected render pipeline has an invalid path `{pipeline}`"
        ))
    })?;
    if Path::new(&pipeline)
        .extension()
        .and_then(|ext| ext.to_str())
        != Some("rendergraph")
    {
        return Err(PackageError::Cook(format!(
            "selected render pipeline `{pipeline}` does not resolve to a .rendergraph asset"
        )));
    }
    Ok(Some(pipeline))
}

/// Resolve the most specific registered file type suffix. Compound types
/// such as `trait.json` take precedence over a generic `json` type.
fn cook_handler_for_path(
    file_types: &[plugin_editor_api::FileTypeDefinition],
    path: &Path,
) -> Result<plugin_editor_api::file_types::CookCallback, String> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(plugin_editor_api::file_types::copy_file_for_cook);
    };
    let mut candidates: Vec<_> = file_types
        .iter()
        .filter(|definition| {
            let Some(suffix_start) = name.len().checked_sub(definition.extension.len()) else {
                return false;
            };
            name.get(suffix_start..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(&definition.extension))
                && name
                    .get(..suffix_start)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        .collect();
    candidates.sort_by(|a, b| b.extension.len().cmp(&a.extension.len()));
    let Some(selected) = candidates.first() else {
        return Ok(plugin_editor_api::file_types::copy_file_for_cook);
    };
    let tied: Vec<_> = candidates
        .iter()
        .take_while(|candidate| candidate.extension.len() == selected.extension.len())
        .collect();
    if tied
        .iter()
        .any(|candidate| !std::ptr::fn_addr_eq(candidate.cook, selected.cook))
    {
        return Err(format!(
            "multiple enabled file types register different cook handlers for .{}",
            selected.extension
        ));
    }
    Ok(selected.cook)
}

/// A machine-specific path in a shipped text file: the project's own
/// location, `CARGO_MANIFEST_DIR`, or any JSON string that is an absolute
/// path. Binary files are skipped.
pub fn machine_path_in(rel: &str, bytes: &[u8], project: &Path) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains("CARGO_MANIFEST_DIR") {
        return Some(format!("{rel}: mentions CARGO_MANIFEST_DIR"));
    }
    let project_text = project.display().to_string();
    if !project_text.is_empty() && text.contains(&project_text) {
        return Some(format!("{rel}: contains the project path {project_text}"));
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    fn find(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(s) if cook::is_absolute_path(s) && s.len() > 1 => {
                Some(s.clone())
            }
            serde_json::Value::Array(items) => items.iter().find_map(find),
            serde_json::Value::Object(map) => map.values().find_map(find),
            _ => None,
        }
    }
    find(&value).map(|path| format!("{rel}: absolute path `{path}`"))
}

/// The editor's `project.default_map` setting (`.pulsar/project/*.toml`).
fn default_map(project: &Path) -> Option<String> {
    pulsar_settings::register_all_settings(engine_state::settings::global_config());
    let settings = engine_state::settings::ProjectSettings::new(project)?;
    settings.load_all();
    let map = settings
        .get("project", "default_map")?
        .as_str()
        .ok()?
        .to_owned();
    (!map.is_empty() && virtual_fs::is_file(&project.join(&map))).then_some(map)
}

fn project_name(project: &Path) -> String {
    project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Pulsar Game".into())
}

#[cfg(test)]
mod asset_dependency_tests {
    use super::*;

    static PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn dependency_walk_is_unbounded_iterative_and_cycle_safe() {
        let _lock = PROCESS_LOCK.lock().unwrap();
        engine_fs::virtual_fs::reset_to_local();
        engine_fs::virtual_fs::serve_packaged_folders(None);
        let project = tempfile::tempdir().unwrap();
        let assets_dir = project.path().join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();

        const DEPTH: usize = 1_100;
        for index in 0..DEPTH {
            let next = if index + 1 == DEPTH { 0 } else { index + 1 };
            std::fs::write(
                assets_dir.join(format!("{index:04}.dep")),
                format!("assets/{next:04}.dep\n"),
            )
            .unwrap();
        }

        let resolver = cook::AssetResolver::new(project.path(), None);
        let (root_id, root_source) = resolver.resolve("assets/0000.dep").unwrap();
        let mut assets = cook::AssetSet::default();
        assets.assets.insert(root_id, root_source);

        collect_asset_dependencies_with(&resolver, &mut assets, |_, source| {
            engine_fs::virtual_fs::read_to_string(source)
                .map(|text| text.lines().map(str::to_owned).collect())
                .map_err(|error| PackageError::Cook(error.to_string()))
        })
        .unwrap();

        assert_eq!(assets.assets.len(), DEPTH);
        assert!(assets.unresolved.is_empty());
    }
}
