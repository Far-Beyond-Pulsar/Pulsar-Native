use crate::utils::types::TypeInfo;
use gpui::{prelude::*, *};
use plugin_editor_api::FileTypeId;
use std::collections::HashMap;
use std::path::PathBuf;
use ui::StyledExt;
use ui::{h_flex, input::InputState, v_flex, ActiveTheme as _};

use crate::utils::NavigateToType;

pub struct TypeDebuggerDrawer {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) types: Vec<TypeInfo>,
    pub(crate) filtered_kind: Option<FileTypeId>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) search_query: String,
    pub(crate) group_by_kind: bool,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) project_root: Option<PathBuf>,
}

impl TypeDebuggerDrawer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search types..."));

        Self {
            focus_handle,
            types: Vec::new(),
            filtered_kind: None,
            selected_index: None,
            search_query: String::new(),
            group_by_kind: true,
            search_input,
            project_root: None,
        }
    }

    pub fn set_types(&mut self, types: Vec<TypeInfo>, cx: &mut Context<Self>) {
        self.types = types;
        self.selected_index = None;
        cx.notify();
    }

    pub fn clear_types(&mut self, cx: &mut Context<Self>) {
        self.types.clear();
        self.selected_index = None;
        cx.notify();
    }

    pub fn set_project_root(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.project_root = project_root;
        cx.notify();
    }

    pub(crate) fn get_display_path(&self, absolute_path: &std::path::Path) -> String {
        display_path(self.project_root.as_deref(), absolute_path)
    }

    pub(crate) fn get_filtered_types(&self) -> Vec<TypeInfo> {
        filter_types(&self.types, self.filtered_kind.as_ref(), &self.search_query)
    }

    pub(crate) fn get_grouped_types(&self) -> HashMap<FileTypeId, Vec<TypeInfo>> {
        group_by_kind(self.get_filtered_types())
    }

    pub(crate) fn count_by_kind(&self, kind: &FileTypeId) -> usize {
        self.types
            .iter()
            .filter(|t| &t.file_type_id == kind)
            .count()
    }

    pub fn total_count(&self) -> usize {
        self.types.len()
    }

    pub(crate) fn set_filter(&mut self, kind: Option<FileTypeId>, cx: &mut Context<Self>) {
        self.filtered_kind = kind;
        self.selected_index = None;
        cx.notify();
    }

    pub(crate) fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.selected_index = None;
        cx.notify();
    }

    pub(crate) fn toggle_grouping(&mut self, cx: &mut Context<Self>) {
        self.group_by_kind = !self.group_by_kind;
        cx.notify();
    }

    pub(crate) fn navigate_to_type(&mut self, type_info: &TypeInfo, cx: &mut Context<Self>) {
        cx.emit(NavigateToType {
            file_path: Some(type_info.file_path.clone()),
            type_name: type_info.name.clone(),
        });
    }
}

impl Focusable for TypeDebuggerDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<NavigateToType> for TypeDebuggerDrawer {}

impl Render for TypeDebuggerDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input_value = self.search_input.read(cx).value().to_string();
        if current_input_value != self.search_query {
            self.search_query = current_input_value;
        }

        let alias_count = self.count_by_kind(&FileTypeId::new("alias"));
        let struct_count = self.count_by_kind(&FileTypeId::new("struct"));
        let enum_count = self.count_by_kind(&FileTypeId::new("enum"));
        let trait_count = self.count_by_kind(&FileTypeId::new("trait"));
        let total_count = self.total_count();

        let filtered_types = self.get_filtered_types();
        let selected_index = self.selected_index;
        let group_by_kind = self.group_by_kind;

        let content: AnyElement = if filtered_types.is_empty() {
            crate::components::render_empty_state(self, cx).into_any_element()
        } else if group_by_kind {
            crate::components::render_grouped_view(self, selected_index, cx).into_any_element()
        } else {
            crate::components::render_flat_view(self, filtered_types, selected_index, cx)
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(crate::handlers::on_filter_all))
            .on_action(cx.listener(crate::handlers::on_filter_aliases))
            .on_action(cx.listener(crate::handlers::on_filter_structs))
            .on_action(cx.listener(crate::handlers::on_filter_enums))
            .on_action(cx.listener(crate::handlers::on_filter_traits))
            .child(crate::components::render_header(
                self,
                alias_count,
                struct_count,
                enum_count,
                trait_count,
                total_count,
                cx,
            ))
            .child(div().flex_1().overflow_hidden().child(content))
    }
}

/// Path shown in the list: relative to the project root (prefixed with the
/// root directory's name) when inside it, otherwise the original path, always
/// with forward slashes.
fn display_path(project_root: Option<&std::path::Path>, absolute_path: &std::path::Path) -> String {
    if let Some(project_root) = project_root {
        if let Ok(relative) = absolute_path.strip_prefix(project_root) {
            if let Some(project_name) = project_root.file_name() {
                let mut display_path = PathBuf::from(project_name);
                display_path.push(relative);
                return display_path.to_string_lossy().replace('\\', "/");
            }
        }
    }
    absolute_path.to_string_lossy().replace('\\', "/")
}

/// Keep types of `kind` (if any) whose name, display name, description or file
/// path contains `search_query` (case-insensitive; empty matches all).
fn filter_types(
    types: &[TypeInfo],
    kind: Option<&FileTypeId>,
    search_query: &str,
) -> Vec<TypeInfo> {
    let mut filtered = types.to_vec();

    if let Some(kind) = kind {
        filtered.retain(|t| &t.file_type_id == kind);
    }

    if !search_query.is_empty() {
        let query = search_query.to_lowercase();
        filtered.retain(|t| {
            t.name.to_lowercase().contains(&query)
                || t.display_name.to_lowercase().contains(&query)
                || t.description
                    .as_ref()
                    .is_some_and(|d| d.to_lowercase().contains(&query))
                || t.file_path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query)
        });
    }

    filtered
}

/// Group types by their file type id, preserving order within each group.
fn group_by_kind(types: Vec<TypeInfo>) -> HashMap<FileTypeId, Vec<TypeInfo>> {
    let mut grouped: HashMap<FileTypeId, Vec<TypeInfo>> = HashMap::new();
    for type_info in types {
        grouped
            .entry(type_info.file_type_id.clone())
            .or_default()
            .push(type_info);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` (pulled in by `super::*`) exports a `test` macro that would shadow `#[test]`.
    use core::prelude::v1::test;
    use std::path::Path;

    fn ty(name: &str, kind: &str, description: Option<&str>, file: &str) -> TypeInfo {
        TypeInfo {
            uuid: uuid::Uuid::nil(),
            name: name.into(),
            display_name: format!("Display {name}"),
            description: description.map(String::from),
            file_path: PathBuf::from(file),
            file_type_id: FileTypeId::new(kind),
            last_modified: None,
        }
    }

    fn sample() -> Vec<TypeInfo> {
        vec![
            ty(
                "Health",
                "alias",
                Some("hit points"),
                "types/health.alias.json",
            ),
            ty("Player", "struct", None, "types/player.struct.json"),
            ty("Team", "enum", Some("Faction"), "types/team.enum.json"),
            ty("Enemy", "struct", None, "types/enemy.struct.json"),
        ]
    }

    #[test]
    fn no_filters_returns_all_types() {
        assert_eq!(filter_types(&sample(), None, ""), sample());
    }

    #[test]
    fn kind_filter_keeps_only_that_kind() {
        let out = filter_types(&sample(), Some(&FileTypeId::new("struct")), "");
        let names: Vec<_> = out.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Player", "Enemy"]);
    }

    #[test]
    fn search_matches_name_display_description_and_path_case_insensitively() {
        let names = |q: &str| -> Vec<String> {
            filter_types(&sample(), None, q)
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        assert_eq!(names("PLAYER"), vec!["Player"]);
        assert_eq!(names("display team"), vec!["Team"]);
        assert_eq!(names("faction"), vec!["Team"]);
        assert_eq!(names("enemy.struct"), vec!["Enemy"]);
        assert!(names("nothing like this").is_empty());
    }

    #[test]
    fn kind_and_search_combine_with_and() {
        let out = filter_types(&sample(), Some(&FileTypeId::new("struct")), "enemy");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Enemy");
        assert!(filter_types(&sample(), Some(&FileTypeId::new("enum")), "enemy").is_empty());
    }

    #[test]
    fn grouping_buckets_by_kind_in_order() {
        let grouped = group_by_kind(sample());
        assert_eq!(grouped.len(), 3);
        let structs = &grouped[&FileTypeId::new("struct")];
        assert_eq!(structs[0].name, "Player");
        assert_eq!(structs[1].name, "Enemy");
    }

    #[test]
    fn display_path_prefixes_project_name_and_normalises_separators() {
        let root = Path::new("/work/MyGame");
        assert_eq!(
            display_path(Some(root), Path::new("/work/MyGame/types/a.json")),
            "MyGame/types/a.json"
        );
        assert_eq!(
            display_path(Some(root), Path::new("/elsewhere/a.json")),
            "/elsewhere/a.json"
        );
        assert_eq!(display_path(None, Path::new("rel\\a.json")), "rel/a.json");
    }
}
