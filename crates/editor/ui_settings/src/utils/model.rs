//! What a settings window shows, as plain data: its pages, each page's
//! sections, and the navigation tree over them (at most three levels: a
//! folder, its pages, a page's sections).
//!
//! The Project Settings window has one page per engine owner of the
//! `project` namespace at the top level, then a "Plugins" folder with one
//! page per plugin: every project owner a plugin's registration added
//! (`pulsar_settings::project::owner_origin`) and the plugin's own `plugin`
//! namespace groups. The Editor Settings window has one page per owner of
//! the `editor` namespace; plugins cannot add to it.
//!
//! Nothing here touches GPUI: the model is built from the settings database
//! and tested on its own.

use std::collections::{BTreeMap, BTreeSet};

use engine_state::{global_config, SettingInfo, NS_EDITOR, NS_PROJECT};
use pulsar_settings::plugin::NS as NS_PLUGIN;

/// Which settings a window edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// The open project's settings: the engine's and every enabled plugin's.
    Project,
    /// The editor's own settings, for every project.
    Editor,
}

/// A setting's place in the database: its namespace, owner path and key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SettingId {
    pub namespace: String,
    pub owner: String,
    pub key: String,
}

impl SettingId {
    pub fn of(info: &SettingInfo) -> Self {
        Self {
            namespace: info.namespace.clone(),
            owner: info.owner.clone(),
            key: info.key.clone(),
        }
    }

    /// Plugin-namespace settings are layered (`engine_state::plugin_settings`):
    /// they are written through that module, not straight into the database.
    pub fn is_plugin(&self) -> bool {
        self.namespace == NS_PLUGIN
    }

    /// A string unique to this setting (an element or state key).
    pub fn key_string(&self) -> String {
        format!("{}/{}/{}", self.namespace, self.owner, self.key)
    }
}

/// One heading of a page and the settings under it, in declaration order.
#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub settings: Vec<SettingInfo>,
}

/// What the navigation tree and the content show for one entry.
#[derive(Clone, Debug)]
pub struct Page {
    /// Stable across rebuilds (`project:<owner>`, `plugin:<plugin id>`, ...).
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub sections: Vec<Section>,
    /// The page sits in the Plugins folder.
    pub plugin: bool,
}

impl Page {
    pub fn setting_count(&self) -> usize {
        self.sections.iter().map(|s| s.settings.len()).sum()
    }
}

/// The id of the Plugins folder in the tree.
pub const PLUGINS_FOLDER: &str = "folder:plugins";

/// What a tree row selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Folder,
    Page(usize),
    Section(usize, usize),
}

/// One visible row of the navigation tree.
#[derive(Clone, Debug)]
pub struct TreeRow {
    /// Stable id: the folder's, the page's, or `<page id>#<section index>`.
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub target: Target,
    /// The row has children (it shows a disclosure chevron).
    pub expandable: bool,
    pub expanded: bool,
}

/// Every page of a window, in display order (engine pages, then plugin pages).
pub fn build_pages(kind: WindowKind) -> Vec<Page> {
    match kind {
        WindowKind::Editor => owner_pages(NS_EDITOR, |_| true, "editor"),
        WindowKind::Project => {
            let mut pages = owner_pages(
                NS_PROJECT,
                |owner| pulsar_settings::project::owner_origin(owner).is_none(),
                "project",
            );
            pages.extend(plugin_pages());
            pages
        }
    }
}

/// One page per owner of `ns` that `keep` accepts, ordered by title.
fn owner_pages(ns: &str, keep: impl Fn(&str) -> bool, id_prefix: &str) -> Vec<Page> {
    let cfg = global_config();
    let mut pages: Vec<Page> = cfg
        .list_owners(ns)
        .into_iter()
        .map(|segments| segments.join("/"))
        .filter(|owner| keep(owner))
        .filter_map(|owner| {
            let (title, description) = owner_title(ns, &owner);
            let sections = sections_of(&[(ns, owner.as_str())], false);
            (!sections.is_empty()).then(|| Page {
                id: format!("{id_prefix}:{owner}"),
                title,
                description,
                sections,
                plugin: false,
            })
        })
        .collect();
    pages.sort_by_key(|page| page.title.to_lowercase());
    pages
}

/// The display name and description `owner` registered with.
fn owner_title(ns: &str, owner: &str) -> (String, Option<String>) {
    match global_config().owner_info(ns, owner) {
        Some((title, description)) => {
            let title = if title.is_empty() {
                humanize(owner)
            } else {
                title
            };
            (title, (!description.is_empty()).then_some(description))
        }
        None => (humanize(owner), None),
    }
}

/// `virtual_geometry` → `Virtual geometry`.
fn humanize(owner: &str) -> String {
    let last = owner
        .rsplit('/')
        .next()
        .unwrap_or(owner)
        .replace(['_', '-', '.'], " ");
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The sections of the owners `(namespace, owner)`: one per schema page
/// (in the order each first appears), settings in declaration order. With
/// several owners, a section title that two owners share is told apart by
/// the owner's name.
fn sections_of(owners: &[(&str, &str)], name_owners: bool) -> Vec<Section> {
    let cfg = global_config();
    let mut sections: Vec<(String, Section)> = Vec::new();
    for (ns, owner) in owners {
        let Some(mut settings) = cfg.list_settings(ns, owner) else {
            continue;
        };
        settings.sort_by_key(|info| info.order);
        let (owner_name, _) = owner_title(ns, owner);
        for info in settings
            .into_iter()
            .filter(|info| info.field_type.is_some())
        {
            let page = info.page.clone().unwrap_or_else(|| owner_name.clone());
            let page = page.rsplit(" / ").next().unwrap_or(&page).to_string();
            let key = format!("{ns}/{owner}#{page}");
            match sections.iter_mut().find(|(k, _)| *k == key) {
                Some((_, section)) => section.settings.push(info),
                None => sections.push((
                    key,
                    Section {
                        title: if name_owners && page != owner_name {
                            format!("{owner_name}: {page}")
                        } else {
                            page
                        },
                        settings: vec![info],
                    },
                )),
            }
        }
    }
    sections.into_iter().map(|(_, section)| section).collect()
}

/// One page per enabled plugin with settings: the project owners its
/// registrations added and its `plugin` namespace groups.
fn plugin_pages() -> Vec<Page> {
    let cfg = global_config();
    // plugin page key -> [(namespace, owner)]
    let mut by_plugin: BTreeMap<String, Vec<(&'static str, String)>> = BTreeMap::new();
    for owner in cfg.list_owners(NS_PROJECT) {
        let owner = owner.join("/");
        if let Some(origin) = pulsar_settings::project::owner_origin(&owner) {
            by_plugin
                .entry(pulsar_settings::plugin::page_key(origin))
                .or_default()
                .push((NS_PROJECT, owner));
        }
    }
    for owner in cfg.list_owners(NS_PLUGIN) {
        if let Some(page) = owner.first().cloned() {
            by_plugin
                .entry(page)
                .or_default()
                .push((NS_PLUGIN, owner.join("/")));
        }
    }

    let mut pages: Vec<Page> = by_plugin
        .into_iter()
        .filter_map(|(page_key, mut owners)| {
            owners.sort();
            let named: BTreeSet<&str> = owners.iter().map(|(_, o)| o.as_str()).collect();
            let refs: Vec<(&str, &str)> = owners.iter().map(|(ns, o)| (*ns, o.as_str())).collect();
            let sections = sections_of(&refs, named.len() > 1);
            if sections.is_empty() {
                return None;
            }
            let descriptor = pulsar_plugins::descriptor(&page_key);
            let title = descriptor
                .map(|plugin| plugin.name.clone())
                .unwrap_or_else(|| pulsar_settings::plugin::page_title(cfg, &page_key));
            let description = descriptor
                .map(|plugin| plugin.description.clone())
                .filter(|d| !d.is_empty());
            Some(Page {
                id: format!("plugin:{page_key}"),
                title,
                description,
                sections,
                plugin: true,
            })
        })
        .collect();
    pages.sort_by_key(|page| page.title.to_lowercase());
    pages
}

/// Whether `info` matches the search `query` (already lowercased): its
/// label, key or description.
pub fn setting_matches(info: &SettingInfo, query: &str) -> bool {
    query.is_empty()
        || info.key.to_lowercase().contains(query)
        || info
            .label
            .as_deref()
            .is_some_and(|label| label.to_lowercase().contains(query))
        || info.description.to_lowercase().contains(query)
}

/// `pages` narrowed to the settings matching `query` (lowercased). A page or
/// section whose own title matches keeps everything under it.
pub fn filter_pages(pages: &[Page], query: &str) -> Vec<Page> {
    if query.is_empty() {
        return pages.to_vec();
    }
    pages
        .iter()
        .filter_map(|page| {
            if page.title.to_lowercase().contains(query) {
                return Some(page.clone());
            }
            let sections: Vec<Section> = page
                .sections
                .iter()
                .filter_map(|section| {
                    if section.title.to_lowercase().contains(query) {
                        return Some(section.clone());
                    }
                    let settings: Vec<SettingInfo> = section
                        .settings
                        .iter()
                        .filter(|info| setting_matches(info, query))
                        .cloned()
                        .collect();
                    (!settings.is_empty()).then(|| Section {
                        title: section.title.clone(),
                        settings,
                    })
                })
                .collect();
            (!sections.is_empty()).then(|| Page {
                sections,
                ..page.clone()
            })
        })
        .collect()
}

/// The visible tree rows over `pages`: engine pages at the top level, then
/// the Plugins folder holding the plugin pages; a page with two or more
/// sections lists them under it. `expanded` holds the ids of open rows;
/// `force_open` opens every row (while searching).
pub fn tree_rows(pages: &[Page], expanded: &BTreeSet<String>, force_open: bool) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let is_open = |id: &str| force_open || expanded.contains(id);

    let push_page = |rows: &mut Vec<TreeRow>, ix: usize, depth: usize| {
        let page = &pages[ix];
        let expandable = page.sections.len() > 1;
        let open = expandable && is_open(&page.id);
        rows.push(TreeRow {
            id: page.id.clone(),
            label: page.title.clone(),
            depth,
            target: Target::Page(ix),
            expandable,
            expanded: open,
        });
        if open {
            for (section_ix, section) in page.sections.iter().enumerate() {
                rows.push(TreeRow {
                    id: format!("{}#{section_ix}", page.id),
                    label: section.title.clone(),
                    depth: depth + 1,
                    target: Target::Section(ix, section_ix),
                    expandable: false,
                    expanded: false,
                });
            }
        }
    };

    for ix in (0..pages.len()).filter(|&ix| !pages[ix].plugin) {
        push_page(&mut rows, ix, 0);
    }
    let plugin_pages: Vec<usize> = (0..pages.len()).filter(|&ix| pages[ix].plugin).collect();
    if !plugin_pages.is_empty() {
        let open = is_open(PLUGINS_FOLDER);
        rows.push(TreeRow {
            id: PLUGINS_FOLDER.to_string(),
            label: "Plugins".to_string(),
            depth: 0,
            target: Target::Folder,
            expandable: true,
            expanded: open,
        });
        if open {
            for ix in plugin_pages {
                push_page(&mut rows, ix, 1);
            }
        }
    }
    rows
}

/// One row of a page's content list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentRow {
    /// The page's title and description.
    PageHeader,
    /// A section heading (the row a tree section jumps to).
    SectionHeader(usize),
    /// A setting: section index, setting index within the section.
    Setting(usize, usize),
}

/// The content rows of `page`, and for each section the row its heading is
/// on (what a section jump scrolls to).
pub fn content_rows(page: &Page) -> (Vec<ContentRow>, Vec<usize>) {
    let mut rows = vec![ContentRow::PageHeader];
    let mut headings = Vec::with_capacity(page.sections.len());
    for (section_ix, section) in page.sections.iter().enumerate() {
        headings.push(rows.len());
        rows.push(ContentRow::SectionHeader(section_ix));
        rows.extend((0..section.settings.len()).map(|ix| ContentRow::Setting(section_ix, ix)));
    }
    (rows, headings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::{ConfigValue, FieldType};

    fn info(key: &str, page: &str) -> SettingInfo {
        SettingInfo {
            key: key.into(),
            namespace: "project".into(),
            owner: "o".into(),
            label: Some(key.to_uppercase()),
            page: Some(page.into()),
            description: format!("about {key}"),
            current_value: ConfigValue::Bool(false),
            default_value: ConfigValue::Bool(false),
            tags: vec![],
            read_only: false,
            field_type: Some(FieldType::Checkbox),
            order: 0,
        }
    }

    fn page(id: &str, plugin: bool, sections: &[(&str, &[&str])]) -> Page {
        Page {
            id: id.into(),
            title: id.into(),
            description: None,
            plugin,
            sections: sections
                .iter()
                .map(|(title, keys)| Section {
                    title: (*title).into(),
                    settings: keys.iter().map(|k| info(k, title)).collect(),
                })
                .collect(),
        }
    }

    fn pages() -> Vec<Page> {
        vec![
            page(
                "engine",
                false,
                &[("Budgets", &["pool"]), ("Loading", &["bind"])],
            ),
            page(
                "alpha",
                true,
                &[("Probes", &["spacing"]), ("Rays", &["count"])],
            ),
            page("beta", true, &[("Beta", &["reflection"])]),
        ]
    }

    #[test]
    fn plugins_fold_under_one_folder_three_levels_deep() {
        let all: BTreeSet<String> = ["engine", "alpha", PLUGINS_FOLDER]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = tree_rows(&pages(), &all, false);
        let shape: Vec<(usize, &str)> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(
            shape,
            [
                (0, "engine"),
                (1, "Budgets"),
                (1, "Loading"),
                (0, "Plugins"),
                (1, "alpha"),
                (2, "Probes"),
                (2, "Rays"),
                (1, "beta"),
            ]
        );
        assert_eq!(rows[5].target, Target::Section(1, 0));
        assert!(
            !rows[7].expandable,
            "a page with one section lists none under it"
        );
    }

    #[test]
    fn a_closed_folder_hides_its_plugins() {
        let rows = tree_rows(&pages(), &BTreeSet::new(), false);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["engine", "Plugins"]);
    }

    #[test]
    fn search_keeps_matching_settings_and_titled_matches() {
        let filtered = filter_pages(&pages(), "spac");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "alpha");
        assert_eq!(filtered[0].sections.len(), 1);
        let by_section = filter_pages(&pages(), "rays");
        assert_eq!(by_section[0].sections[0].settings.len(), 1);
        let by_page = filter_pages(&pages(), "beta");
        assert_eq!(by_page[0].setting_count(), 1);
    }

    #[test]
    fn every_section_heading_has_a_row_to_jump_to() {
        let (rows, headings) = content_rows(&pages()[0]);
        assert_eq!(headings, [1, 3]);
        assert_eq!(rows[headings[1]], ContentRow::SectionHeader(1));
        assert_eq!(rows.last(), Some(&ContentRow::Setting(1, 0)));
    }
}
