//! What the Extensions viewer shows, built from one catalog snapshot
//! (`plugin_manager::CatalogSnapshot`) and nothing else, so the list, its
//! counts and filters, each entry's dependencies and its enabled state
//! cannot disagree. GPUI-free: the viewer draws it, the tests read it.
//!
//! Entries are the installed plugins (object kinds and editor tools, by
//! their descriptors), the plugin libraries loaded at runtime, and the
//! engine itself, whose own registrations are labelled as the engine's.
//! Each entry's contributions are grouped by category (render features with
//! their nodes and the buffers they read, settings, editors, importers...),
//! as the registrations summarize themselves.

use plugin_manager::{
    CatalogPlugin, CatalogSnapshot, DependencyState, PluginDetails, Provider, RequirementState,
};
use pulsar_plugins::Summary;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// An installed plugin a game links (an object kind, physics, input...).
    Plugin,
    /// An installed editor tool (`editor_only`).
    EditorTool,
    /// A plugin library loaded at runtime.
    Library,
    /// The engine's own registrations.
    Engine,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Plugin => "Plugin",
            Self::EditorTool => "Editor tool",
            Self::Library => "Library",
            Self::Engine => "Engine",
        }
    }
}

/// One row of the list, with everything its details page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: EntryKind,
    /// Whether the open project has it (the engine and loaded libraries
    /// always do).
    pub enabled: bool,
    /// Where its registrations come from: its root crate, its library file,
    /// or the engine's crates.
    pub source: String,
    /// Its contributions, grouped by category.
    pub groups: Vec<Group>,
    /// The contracts it provides.
    pub provides: Vec<String>,
    /// The contracts (or, for a library, the plugins) it requires, each
    /// with its state.
    pub requirements: Vec<RequirementRow>,
    /// The entries that require what it provides.
    pub required_by: Vec<String>,
    /// What does not hold together, naming it: activation problems
    /// (consumer, contract, provider) and catalog problems.
    pub problems: Vec<String>,
}

/// A category of contributions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub category: String,
    pub title: String,
    pub items: Vec<Item>,
}

/// One contribution, with what it brings in turn (a feature's nodes and
/// the buffers it reads, a provider's file types and editors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub details: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequirementRow {
    pub contract: String,
    pub optional: bool,
    pub state: RowState,
    /// What a person reads: who meets it, or why not.
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Met,
    Absent,
    Unmet,
    Inactive,
}

impl Entry {
    /// How many registrations it contributes (top level).
    pub fn contribution_count(&self) -> usize {
        self.groups.iter().map(|group| group.items.len()).sum()
    }

    pub fn needs_attention(&self) -> bool {
        !self.problems.is_empty()
            || self
                .requirements
                .iter()
                .any(|requirement| requirement.state == RowState::Unmet)
    }

    fn matches(&self, query: &str) -> bool {
        let hit = |text: &str| text.to_lowercase().contains(query);
        hit(&self.id)
            || hit(&self.name)
            || hit(&self.description)
            || self.groups.iter().any(|group| {
                hit(&group.title)
                    || group.items.iter().any(|item| {
                        hit(&item.id) || hit(&item.label) || item.details.iter().any(|d| hit(d))
                    })
            })
            || self.provides.iter().any(|c| hit(c))
            || self.requirements.iter().any(|r| hit(&r.contract))
    }
}

/// Which entries the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Enabled,
    Disabled,
    NeedsAttention,
}

impl Filter {
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Enabled,
        Filter::Disabled,
        Filter::NeedsAttention,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::NeedsAttention => "Needs attention",
        }
    }

    fn admits(self, entry: &Entry) -> bool {
        match self {
            Self::All => true,
            Self::Enabled => entry.enabled,
            Self::Disabled => !entry.enabled,
            Self::NeedsAttention => entry.needs_attention(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Contributions,
}

/// The viewer's whole model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extensions {
    pub entries: Vec<Entry>,
    /// Problems naming no entry (a registration from an unknown origin).
    pub unattributed_problems: Vec<String>,
}

impl Extensions {
    /// The model of `catalog`.
    pub fn from_snapshot(catalog: &CatalogSnapshot) -> Self {
        let mut entries: Vec<Entry> = catalog
            .plugins
            .iter()
            .map(|plugin| plugin_entry(catalog, plugin))
            .collect();
        entries.extend(catalog.libraries.iter().map(library_entry));
        entries.push(Entry {
            id: "engine".into(),
            name: "Pulsar engine".into(),
            description: "What the engine itself registers: its own editors, roles, \
                          settings and scene seams."
                .into(),
            kind: EntryKind::Engine,
            enabled: true,
            source: "engine crates".into(),
            groups: groups(&catalog.engine),
            provides: Vec::new(),
            requirements: Vec::new(),
            required_by: Vec::new(),
            problems: Vec::new(),
        });
        let attributed: Vec<String> = entries
            .iter()
            .flat_map(|entry| entry.problems.clone())
            .collect();
        let unattributed_problems = catalog
            .problems
            .iter()
            .map(ToString::to_string)
            .chain(
                catalog
                    .activation_problems()
                    .iter()
                    .map(ToString::to_string),
            )
            .filter(|problem| !attributed.contains(problem))
            .collect();
        Self {
            entries,
            unattributed_problems,
        }
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// How many entries `filter` admits (before any search).
    pub fn count(&self, filter: Filter) -> usize {
        self.entries
            .iter()
            .filter(|entry| filter.admits(entry))
            .count()
    }

    /// The entries to list, as indices into [`Self::entries`].
    pub fn visible(&self, filter: Filter, query: &str, sort: Sort) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        let mut indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                filter.admits(entry) && (query.is_empty() || entry.matches(&query))
            })
            .map(|(index, _)| index)
            .collect();
        let name = |index: &usize| self.entries[*index].name.to_lowercase();
        match sort {
            Sort::Name => indices.sort_by_key(name),
            Sort::Contributions => indices.sort_by(|a, b| {
                self.entries[*b]
                    .contribution_count()
                    .cmp(&self.entries[*a].contribution_count())
                    .then_with(|| name(a).cmp(&name(b)))
            }),
        }
        indices
    }
}

fn plugin_entry(catalog: &CatalogSnapshot, plugin: &CatalogPlugin) -> Entry {
    let descriptor = plugin.descriptor;
    let mut problems: Vec<String> = catalog
        .activation
        .iter()
        .flat_map(|resolution| resolution.problems_of(&descriptor.id))
        .map(ToString::to_string)
        .collect();
    problems.extend(
        catalog
            .problems
            .iter()
            .filter(|problem| names(problem, descriptor))
            .map(ToString::to_string),
    );
    let requirements = plugin
        .requirements
        .iter()
        .map(|requirement| {
            let (state, note) = match &requirement.state {
                RequirementState::Met { provider } => (RowState::Met, format!("met by {provider}")),
                RequirementState::AbsentOptional => (
                    RowState::Absent,
                    "optional; nothing enabled provides it".to_string(),
                ),
                RequirementState::Unmet(problem) => (RowState::Unmet, problem.to_string()),
                RequirementState::Inactive => (
                    RowState::Inactive,
                    if requirement.providers.is_empty() {
                        "no installed plugin provides it".to_string()
                    } else {
                        format!("provided by {}", requirement.providers.join(", "))
                    },
                ),
            };
            RequirementRow {
                contract: requirement.required.id.clone(),
                optional: requirement.required.optional,
                state,
                note,
            }
        })
        .collect();
    let mut groups = groups(&plugin.contributions);
    // Each feature's reads, linked to their provider.
    for group in groups.iter_mut().filter(|g| g.category == "render.feature") {
        for item in &mut group.items {
            for link in plugin.reads.iter().filter(|link| link.feature == item.id) {
                let provider = match &link.provider {
                    Provider::Plugin { plugin, contract } => format!("{contract} ({plugin})"),
                    Provider::Engine => "the engine".to_string(),
                    Provider::Nobody => "nobody".to_string(),
                };
                let read = format!("reads {}", link.key);
                if let Some(detail) = item.details.iter_mut().find(|d| **d == read) {
                    *detail = format!("reads {} from {provider}", link.key);
                }
            }
        }
    }
    Entry {
        id: descriptor.id.clone(),
        name: descriptor.name.clone(),
        description: descriptor.description.clone(),
        kind: if descriptor.editor_only {
            EntryKind::EditorTool
        } else {
            EntryKind::Plugin
        },
        enabled: plugin.enabled,
        source: descriptor.root.clone(),
        groups,
        provides: descriptor
            .provides
            .iter()
            .map(|contract| {
                format!(
                    "{} ({} v{}: {})",
                    contract.id,
                    contract.kind,
                    contract.version,
                    contract.members.join(", ")
                )
            })
            .collect(),
        requirements,
        required_by: plugin.required_by.iter().map(|id| id.to_string()).collect(),
        problems,
    }
}

/// Whether a catalog problem names `descriptor`'s plugin.
fn names(
    problem: &plugin_manager::CatalogProblem,
    descriptor: &pulsar_plugins::PluginDescriptor,
) -> bool {
    use plugin_manager::CatalogProblem as P;
    match problem {
        P::UnknownOrigin { .. } => false,
        P::DuplicateId { origin, .. } => descriptor.owns(origin),
        P::UnprovidedBuffer { plugin, .. }
        | P::UndeclaredRead { plugin, .. }
        | P::UndeclaredTraitUse { plugin, .. }
        | P::UndeclaredTraitDefinition { plugin, .. } => *plugin == descriptor.id,
    }
}

fn library_entry(library: &PluginDetails) -> Entry {
    let item = |label: &String| Item {
        id: label.clone(),
        label: label.clone(),
        details: Vec::new(),
    };
    let group = |category: &str, title: &str, items: &[String]| Group {
        category: category.into(),
        title: title.into(),
        items: items.iter().map(item).collect(),
    };
    let groups = [
        group("editor.file_type", "File types", &library.file_types),
        group("editor.editor", "Editors", &library.editors),
        group("component.definition", "Components", &library.components),
        group(
            "editor.statusbar",
            "Statusbar widgets",
            &library.statusbar_buttons,
        ),
        group("editor.ai_tool", "AI tools", &library.ai_tools),
        group("engine.subsystem", "Engine subsystems", &library.subsystems),
    ]
    .into_iter()
    .filter(|group| !group.items.is_empty())
    .collect();
    let requirements = library
        .dependencies
        .iter()
        .map(|dependency| {
            let (state, note) = match &dependency.state {
                DependencyState::Satisfied { version } => {
                    (RowState::Met, format!("loaded, version {version}"))
                }
                DependencyState::VersionMismatch { found } => (
                    RowState::Unmet,
                    format!(
                        "version {found} does not satisfy {}",
                        dependency.dependency.requirement
                    ),
                ),
                DependencyState::Missing if dependency.optional => {
                    (RowState::Absent, "not installed (optional)".to_string())
                }
                DependencyState::Missing => (RowState::Unmet, "not installed".to_string()),
            };
            RequirementRow {
                contract: dependency.dependency.id.to_string(),
                optional: dependency.optional,
                state,
                note,
            }
        })
        .collect();
    let metadata = &library.metadata;
    Entry {
        id: metadata.id.to_string(),
        name: metadata.name.clone(),
        description: metadata.description.clone(),
        kind: EntryKind::Library,
        enabled: true,
        source: library.library_path.display().to_string(),
        groups,
        provides: Vec::new(),
        requirements,
        required_by: library
            .required_by
            .iter()
            .map(ToString::to_string)
            .collect(),
        problems: Vec::new(),
    }
}

/// `summaries` grouped by category, in category order. A render feature
/// registration is shown as its features (each with its nodes and reads),
/// a linked editor provider with what it brings.
fn groups(summaries: &[Summary]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut push =
        |category: &str, item: Item| match groups.iter_mut().find(|g| g.category == category) {
            Some(group) => group.items.push(item),
            None => groups.push(Group {
                category: category.to_string(),
                title: title_of(category),
                items: vec![item],
            }),
        };
    for summary in summaries {
        if summary.category == "render.features" {
            for feature in &summary.children {
                push(
                    "render.feature",
                    Item {
                        id: feature.id.clone(),
                        label: feature.label.clone(),
                        details: feature.children.iter().map(child_line).collect(),
                    },
                );
            }
            continue;
        }
        push(
            summary.category,
            Item {
                id: summary.id.clone(),
                label: summary.label.clone(),
                details: summary.children.iter().map(child_line).collect(),
            },
        );
    }
    groups.sort_by(|a, b| a.category.cmp(&b.category));
    groups
}

fn child_line(child: &Summary) -> String {
    match child.category {
        "render.node" => format!("node {} ({})", child.id, child.label),
        "render.reads" => format!("reads {}", child.id),
        _ => format!("{}: {}", title_of(child.category), child.label),
    }
}

/// A person's name for a category id (`render.feature` -> "Render
/// features"): the known ones by name, any other from its words.
pub fn title_of(category: &str) -> String {
    let known = match category {
        "render.feature" => Some("Render features"),
        "component.class" => Some("Component classes"),
        "settings.group" => Some("Settings groups"),
        "settings.project" => Some("Project settings"),
        "editor.provider" => Some("Editors"),
        "editor.create_entry" => Some("Create menu entries"),
        "asset.importer" => Some("Importers"),
        "fs.file_providers" => Some("Plugin folders"),
        "gameplay.roles" => Some("Gameplay roles"),
        "traits.definition" => Some("Traits"),
        "traits.implementer_sources" => Some("Trait implementer sources"),
        "traits.native_implementers" => Some("Trait implementations"),
        _ => None,
    };
    if let Some(known) = known {
        return known.to_string();
    }
    let words = category.replace(['.', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_plugins::{PluginDescriptor, ProjectPlugins};

    fn installed() -> &'static [PluginDescriptor] {
        let parse = |source: &str| PluginDescriptor::parse(source).unwrap();
        Box::leak(Box::new([
            parse(
                r#"
                id = "p.fog"
                name = "Fog"
                description = "Media"
                root = "fog_plugin"
                requires = [{ id = "p.lights.rows" }]
                "#,
            ),
            parse(
                r#"
                id = "p.lights"
                name = "Lights"
                root = "lights_plugin"
                provides = [{ id = "p.lights.rows", kind = "buffer", members = ["lights"] }]
                "#,
            ),
            parse(
                r#"
                id = "p.viewer"
                name = "Viewer"
                root = "viewer_plugin"
                editor_only = true
                "#,
            ),
        ]))
    }

    fn contributions() -> Vec<Summary> {
        let mut features = Summary::new("render.features", "fog", "Render features", "fog_plugin");
        features.children = vec![Summary {
            children: vec![
                Summary::new("render.node", "fog.march", "March", "fog_plugin"),
                Summary::new("render.reads", "lights", "lights", "fog_plugin"),
            ],
            ..Summary::new("render.feature", "helio.fog", "helio.fog", "fog_plugin")
        }];
        vec![
            features,
            Summary::new("settings.group", "fog", "Fog", "fog_plugin"),
            Summary::new(
                "editor.provider",
                "levels",
                "Level Editor",
                "engine:ui_core",
            ),
        ]
    }

    /// The viewer's model, headless: one snapshot gives every entry (the
    /// engine labelled as such), each feature with its nodes and the buffer
    /// it reads linked to its provider, and the counts the filters show.
    #[test]
    fn the_model_is_read_from_one_snapshot() {
        let resolution = pulsar_plugins::resolve(installed(), &ProjectPlugins::default());
        let catalog = CatalogSnapshot::from_parts(installed(), contributions(), Some(resolution));
        let model = Extensions::from_snapshot(&catalog);
        assert_eq!(
            model
                .entries
                .iter()
                .map(|e| (e.id.as_str(), e.kind))
                .collect::<Vec<_>>(),
            [
                ("p.fog", EntryKind::Plugin),
                ("p.lights", EntryKind::Plugin),
                ("p.viewer", EntryKind::EditorTool),
                ("engine", EntryKind::Engine),
            ]
        );
        let fog = model.entry("p.fog").unwrap();
        let features = fog
            .groups
            .iter()
            .find(|g| g.category == "render.feature")
            .unwrap();
        assert_eq!(features.title, "Render features");
        assert_eq!(
            features.items[0].details,
            [
                "node fog.march (March)",
                "reads lights from p.lights.rows (p.lights)"
            ]
        );
        assert_eq!(fog.requirements[0].note, "met by p.lights");
        assert_eq!(model.entry("p.lights").unwrap().required_by, ["p.fog"]);
        let engine = model.entry("engine").unwrap();
        assert_eq!(engine.groups[0].title, "Editors");
        assert_eq!(engine.groups[0].items[0].label, "Level Editor");

        assert_eq!(model.count(Filter::All), 4);
        assert_eq!(model.count(Filter::Enabled), 4);
        assert_eq!(model.count(Filter::NeedsAttention), 0);
        // Search reaches contributions: a node id finds its plugin.
        let found = model.visible(Filter::All, "MARCH", Sort::Name);
        assert_eq!(found.len(), 1);
        assert_eq!(model.entries[found[0]].id, "p.fog");
        // By contributions, the plugin with most comes first.
        assert_eq!(
            model.entries[model.visible(Filter::All, "", Sort::Contributions)[0]].id,
            "p.fog"
        );
    }

    /// A disabled provider: its consumer is disabled too, and both entries
    /// carry the named problem; the filters count them from the same data.
    #[test]
    fn a_disabled_provider_shows_its_named_problem() {
        let resolution = pulsar_plugins::resolve(
            installed(),
            &ProjectPlugins::default().with(&["p.lights"], false),
        );
        let catalog = CatalogSnapshot::from_parts(installed(), contributions(), Some(resolution));
        let model = Extensions::from_snapshot(&catalog);
        let fog = model.entry("p.fog").unwrap();
        assert!(!fog.enabled && fog.needs_attention());
        let named = "plugin p.fog needs contract p.lights.rows, provided by p.lights, which the \
                     project disables; p.fog is disabled";
        assert_eq!(fog.problems, [named]);
        assert_eq!(fog.requirements[0].state, RowState::Unmet);
        assert_eq!(model.entry("p.lights").unwrap().problems, [named]);
        assert!(model.unattributed_problems.is_empty());
        assert_eq!(model.count(Filter::Disabled), 2);
        assert_eq!(model.count(Filter::NeedsAttention), 2);
        assert_eq!(
            model.visible(Filter::Enabled, "", Sort::Name).len(),
            2,
            "the viewer and the engine"
        );
    }
}
