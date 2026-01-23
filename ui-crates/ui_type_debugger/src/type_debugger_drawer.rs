//! Type Debugger Drawer - Studio-quality type database inspection panel
//! Displays all registered types with professional UI and search capabilities

use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    popup_menu::{PopupMenu, PopupMenuExt},
};
use ui::StyledExt;
use std::path::PathBuf;
use std::collections::HashMap;
use type_db::{TypeInfo, TypeKind};

// Define actions for filter menu
actions!(type_debugger_drawer, [FilterAll, FilterAliases, FilterStructs, FilterEnums, FilterTraits]);

// Navigation event
#[derive(Clone, Debug)]
pub struct NavigateToType {
    pub file_path: Option<PathBuf>,
    pub type_name: String,
}

impl EventEmitter<NavigateToType> for TypeDebuggerDrawer {}

pub struct TypeDebuggerDrawer {
    focus_handle: FocusHandle,
    // Store types locally for UI display
    types: Vec<TypeInfo>,
    filtered_kind: Option<TypeKind>,
    selected_index: Option<usize>,
    search_query: String,
    group_by_kind: bool,
    /// InputState for the search bar
    search_input: Entity<InputState>,
    /// Project root path for computing relative paths
    project_root: Option<PathBuf>,
}

impl TypeDebuggerDrawer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        // Create search input state
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search types...")
        });

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

    /// Update the displayed types (called from backend)
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

    /// Set the project root path for computing relative paths
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.project_root = project_root;
        cx.notify();
    }

    /// Compute relative path from absolute path using project root
    fn get_display_path(&self, absolute_path: &PathBuf) -> String {
        if let Some(project_root) = &self.project_root {
            // Try to strip the project root prefix
            if let Ok(relative) = absolute_path.strip_prefix(project_root) {
                // Get the project folder name
                if let Some(project_name) = project_root.file_name() {
                    // Return project_name/relative_path
                    let mut display_path = PathBuf::from(project_name);
                    display_path.push(relative);
                    return display_path.to_string_lossy().replace('\\', "/");
                }
            }
        }
        
        // Fallback to absolute path if project root not set or path doesn't match
        absolute_path.to_string_lossy().replace('\\', "/")
    }

    fn get_filtered_types(&self) -> Vec<TypeInfo> {
        let mut filtered = self.types.clone();

        // Filter by kind
        if let Some(kind) = &self.filtered_kind {
            filtered.retain(|t| &t.type_kind == kind);
        }

        // Filter by search query
        if !self.search_query.is_empty() {
            let query = self.search_query.to_lowercase();
            filtered.retain(|t| {
                t.name.to_lowercase().contains(&query) ||
                t.display_name.to_lowercase().contains(&query) ||
                t.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query)) ||
                t.file_path.as_ref().map_or(false, |p| p.to_string_lossy().to_lowercase().contains(&query))
            });
        }

        filtered
    }

    fn get_grouped_types(&self) -> HashMap<TypeKind, Vec<TypeInfo>> {
        let types = self.get_filtered_types();
        let mut grouped: HashMap<TypeKind, Vec<TypeInfo>> = HashMap::new();

        for type_info in types {
            grouped
                .entry(type_info.type_kind.clone())
                .or_insert_with(Vec::new)
                .push(type_info);
        }

        grouped
    }

    pub fn count_by_kind(&self, kind: TypeKind) -> usize {
        self.types.iter().filter(|t| t.type_kind == kind).count()
    }

    pub fn total_count(&self) -> usize {
        self.types.len()
    }

    fn set_filter(&mut self, kind: Option<TypeKind>, cx: &mut Context<Self>) {
        self.filtered_kind = kind;
        self.selected_index = None;
        cx.notify();
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.selected_index = None;
        cx.notify();
    }

    fn toggle_grouping(&mut self, cx: &mut Context<Self>) {
        self.group_by_kind = !self.group_by_kind;
        cx.notify();
    }

    fn navigate_to_type(&mut self, type_info: &TypeInfo, cx: &mut Context<Self>) {
        cx.emit(NavigateToType {
            file_path: type_info.file_path.clone(),
            type_name: type_info.name.clone(),
        });
    }

    // Action handlers for filter menu
    fn on_filter_all(&mut self, _: &FilterAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(None, cx);
    }

    fn on_filter_aliases(&mut self, _: &FilterAliases, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(TypeKind::Alias), cx);
    }

    fn on_filter_structs(&mut self, _: &FilterStructs, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(TypeKind::Struct), cx);
    }

    fn on_filter_enums(&mut self, _: &FilterEnums, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(TypeKind::Enum), cx);
    }

    fn on_filter_traits(&mut self, _: &FilterTraits, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(TypeKind::Trait), cx);
    }

    fn render_header(
        &mut self,
        alias_count: usize,
        struct_count: usize,
        enum_count: usize,
        trait_count: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current_filter_label = match &self.filtered_kind {
            None => format!("All Types ({})", total_count),
            Some(TypeKind::Alias) => format!("Aliases ({})", alias_count),
            Some(TypeKind::Struct) => format!("Structs ({})", struct_count),
            Some(TypeKind::Enum) => format!("Enums ({})", enum_count),
            Some(TypeKind::Trait) => format!("Traits ({})", trait_count),
        };

        v_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            // Top row: Title, stats, and actions
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(t!("TypeDebugger.Title").to_string())
                            )
                            // Type counts with professional styling
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .when(alias_count > 0, |this| {
                                        this.child(self.render_type_badge(
                                            TypeKind::Alias,
                                            alias_count,
                                            cx
                                        ))
                                    })
                                    .when(struct_count > 0, |this| {
                                        this.child(self.render_type_badge(
                                            TypeKind::Struct,
                                            struct_count,
                                            cx
                                        ))
                                    })
                                    .when(enum_count > 0, |this| {
                                        this.child(self.render_type_badge(
                                            TypeKind::Enum,
                                            enum_count,
                                            cx
                                        ))
                                    })
                                    .when(trait_count > 0, |this| {
                                        this.child(self.render_type_badge(
                                            TypeKind::Trait,
                                            trait_count,
                                            cx
                                        ))
                                    })
                            )
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("toggle-grouping")
                                    .ghost()
                                    .small()
                                    .icon(if self.group_by_kind {
                                        IconName::List
                                    } else {
                                        IconName::Folder
                                    })
                                    .tooltip(if self.group_by_kind {
                                        t!("TypeDebugger.Action.ShowFlatList").to_string()
                                    } else {
                                        t!("TypeDebugger.Action.GroupByKind").to_string()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_grouping(cx);
                                    }))
                            )
                            .child(
                                Button::new("clear-all")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .tooltip(t!("TypeDebugger.Action.ClearAll").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_types(cx);
                                    }))
                            )
                    )
            )
            // Search and filter row
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    // Functional search bar
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.0))
                            .child(
                                TextInput::new(&self.search_input)
                                    .w_full()
                                    .prefix(
                                        ui::Icon::new(IconName::Search)
                                            .size_4()
                                            .text_color(cx.theme().muted_foreground)
                                    )
                            )
                    )
                    // Filter dropdown button using proper PopupMenu
                    .child({
                        let is_all_selected = self.filtered_kind.is_none();
                        let is_aliases_selected = self.filtered_kind == Some(TypeKind::Alias);
                        let is_structs_selected = self.filtered_kind == Some(TypeKind::Struct);
                        let is_enums_selected = self.filtered_kind == Some(TypeKind::Enum);
                        let is_traits_selected = self.filtered_kind == Some(TypeKind::Trait);

                        Button::new("filter-dropdown")
                            .ghost()
                            .small()
                            .icon(IconName::Filter)
                            .label(current_filter_label.clone())
                            .popup_menu_with_anchor(Corner::BottomRight, move |menu, _window, _cx| {
                                menu.menu_with_check("All Types", is_all_selected, Box::new(FilterAll))
                                    .separator()
                                    .menu_with_check("Aliases", is_aliases_selected, Box::new(FilterAliases))
                                    .menu_with_check("Structs", is_structs_selected, Box::new(FilterStructs))
                                    .menu_with_check("Enums", is_enums_selected, Box::new(FilterEnums))
                                    .menu_with_check("Traits", is_traits_selected, Box::new(FilterTraits))
                            })
                    })
            )
    }

    fn render_type_badge(
        &self,
        kind: TypeKind,
        count: usize,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_0p5()
            .rounded_md()
            .bg(self.kind_color(&kind, cx).opacity(0.15))
            .child(
                self.kind_icon(&kind)
                    .size_3()
                    .text_color(self.kind_color(&kind, cx))
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.kind_color(&kind, cx))
                    .child(count.to_string())
            )
    }

    fn render_empty_state(&self, cx: &App) -> Div {
        div().size_full().child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .gap_4()
                        .items_center()
                        .max_w(px(400.0))
                        .px_6()
                        .py_8()
                        .rounded_xl()
                        .bg(cx.theme().secondary.opacity(0.2))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.3))
                        .child(
                            div()
                                .w(px(64.0))
                                .h(px(64.0))
                                .rounded_full()
                                .bg(cx.theme().accent.opacity(0.15))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    ui::Icon::new(IconName::Database)
                                        .size(px(32.0))
                                        .text_color(cx.theme().accent)
                                )
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child(t!("TypeDebugger.Empty.Title").to_string())
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .line_height(rems(1.5))
                                .child(if !self.search_query.is_empty() {
                                    "No types match your search. Try a different query."
                                } else {
                                    "The type database is empty. Types will appear here once registered."
                                })
                        )
                )
        )
    }

    fn render_type_item(
        &self,
        type_info: &TypeInfo,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let type_info_clone = type_info.clone();

        div()
            .w_full()
            .px_3()
            .py_2()
            .child(
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(if is_selected {
                        cx.theme().accent
                    } else {
                        cx.theme().border.opacity(0.5)
                    })
                    .bg(if is_selected {
                        cx.theme().accent.opacity(0.08)
                    } else {
                        cx.theme().sidebar.opacity(0.5)
                    })
                    .shadow_sm()
                    .when(is_selected, |this| {
                        this.border_l_3()
                            .border_color(cx.theme().accent)
                    })
                    .hover(|this| {
                        this.bg(cx.theme().secondary.opacity(0.7))
                            .border_color(cx.theme().accent.opacity(0.5))
                    })
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                        this.navigate_to_type(&type_info_clone, cx);
                    }))
                    .child(
                        v_flex()
                            .gap_2()
                            .w_full()
                            // Type kind and name
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .w_full()
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                self.kind_icon(&type_info.type_kind)
                                                    .size_4()
                                                    .text_color(self.kind_color(&type_info.type_kind, cx))
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(self.kind_color(&type_info.type_kind, cx))
                                                    .child(Self::kind_label(&type_info.type_kind))
                                            )
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("ID: {}", type_info.id))
                                    )
                            )
                            // Display name
                            .child(
                                div()
                                    .w_full()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .line_height(rems(1.4))
                                    .child(type_info.display_name.clone())
                            )
                            // Description
                            .when(type_info.description.is_some(), |container| {
                                container.child(
                                    div()
                                        .w_full()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .line_height(rems(1.4))
                                        .child(type_info.description.as_ref().unwrap().clone())
                                )
                            })
                            // File path
                            .when(type_info.file_path.is_some(), |container| {
                                let display_path = self.get_display_path(type_info.file_path.as_ref().unwrap());
                                container.child(
                                    div()
                                        .w_full()
                                        .px_2()
                                        .py_1()
                                        .mt_1()
                                        .rounded_md()
                                        .bg(cx.theme().background.opacity(0.5))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.3))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    ui::Icon::new(IconName::Folder)
                                                        .size_3()
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_family("monospace")
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(display_path)
                                                )
                                        )
                                )
                            })
                    )
            )
    }

    fn kind_icon(&self, kind: &TypeKind) -> Icon {
        let icon_name = match kind {
            TypeKind::Alias => IconName::Link,
            TypeKind::Struct => IconName::Box,
            TypeKind::Enum => IconName::List,
            TypeKind::Trait => IconName::Code,
        };
        Icon::new(icon_name)
    }

    fn kind_color(&self, kind: &TypeKind, _cx: &App) -> Hsla {
        match kind {
            TypeKind::Alias => gpui::rgb(0x607D8B).into(),   // Blue Gray (matches FileType::AliasType)
            TypeKind::Struct => gpui::rgb(0x00BCD4).into(),  // Cyan (matches FileType::StructType)
            TypeKind::Enum => gpui::rgb(0x673AB7).into(),    // Deep Purple (matches FileType::EnumType)
            TypeKind::Trait => gpui::rgb(0x3F51B5).into(),   // Indigo (matches FileType::TraitType)
        }
    }

    fn kind_label(kind: &TypeKind) -> &'static str {
        match kind {
            TypeKind::Alias => "Aliases",
            TypeKind::Struct => "Structs",
            TypeKind::Enum => "Enums",
            TypeKind::Trait => "Traits",
        }
    }

    fn render_flat_view(
        &self,
        types: Vec<TypeInfo>,
        selected_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("type-debugger-scroll-container")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .children(
                        types.into_iter().enumerate().map(|(index, type_info)| {
                            let is_selected = selected_index == Some(index);
                            self.render_type_item(&type_info, is_selected, cx)
                        })
                    )
            )
    }

    fn render_grouped_view(
        &self,
        selected_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let grouped = self.get_grouped_types();
        let mut global_index = 0;

        div()
            .id("type-debugger-scroll-container-grouped")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .children({
                        let mut groups = Vec::new();
                        // Order: Aliases, Structs, Enums, Traits
                        for kind in [TypeKind::Alias, TypeKind::Struct, TypeKind::Enum, TypeKind::Trait] {
                            if let Some(types) = grouped.get(&kind) {
                                if !types.is_empty() {
                                    let kind_clone = kind.clone();
                                    let types_clone = types.clone();
                                    
                                    groups.push(
                                        v_flex()
                                            .w_full()
                                            .px_3()
                                            .child(
                                                // Kind header - styled like a section header
                                                div()
                                                    .w_full()
                                                    .px_3()
                                                    .py_2()
                                                    .mb_2()
                                                    .rounded_md()
                                                    .bg(cx.theme().secondary.opacity(0.3))
                                                    .border_1()
                                                    .border_color(cx.theme().border.opacity(0.3))
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .gap_3()
                                                            .items_center()
                                                            .child(
                                                                self.kind_icon(&kind_clone)
                                                                    .size_4()
                                                                    .text_color(self.kind_color(&kind_clone, cx))
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .text_sm()
                                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                                    .text_color(cx.theme().foreground)
                                                                    .child(format!("{} ({})", Self::kind_label(&kind_clone), types_clone.len()))
                                                            )
                                                    )
                                            )
                                            .children(
                                                types_clone.iter().map(|type_info| {
                                                    let is_selected = selected_index == Some(global_index);
                                                    global_index += 1;
                                                    self.render_type_item(type_info, is_selected, cx)
                                                }).collect::<Vec<_>>()
                                            )
                                    );
                                }
                            }
                        }
                        groups
                    })
            )
    }
}

impl Focusable for TypeDebuggerDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TypeDebuggerDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update search query from input state
        let current_input_value = self.search_input.read(cx).value().to_string();
        if current_input_value != self.search_query {
            self.search_query = current_input_value;
        }

        let alias_count = self.count_by_kind(TypeKind::Alias);
        let struct_count = self.count_by_kind(TypeKind::Struct);
        let enum_count = self.count_by_kind(TypeKind::Enum);
        let trait_count = self.count_by_kind(TypeKind::Trait);
        let total_count = self.total_count();

        let filtered_types = self.get_filtered_types();
        let selected_index = self.selected_index;
        let group_by_kind = self.group_by_kind;

        // Pre-render content area based on state
        let content: AnyElement = if filtered_types.is_empty() {
            self.render_empty_state(cx).into_any_element()
        } else if group_by_kind {
            self.render_grouped_view(selected_index, cx).into_any_element()
        } else {
            self.render_flat_view(filtered_types, selected_index, cx).into_any_element()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Register action handlers for filter menu
            .on_action(cx.listener(Self::on_filter_all))
            .on_action(cx.listener(Self::on_filter_aliases))
            .on_action(cx.listener(Self::on_filter_structs))
            .on_action(cx.listener(Self::on_filter_enums))
            .on_action(cx.listener(Self::on_filter_traits))
            // Professional header with search
            .child(self.render_header(
                alias_count, struct_count, enum_count, trait_count, total_count, cx
            ))
            // Main content area - flex_1 + overflow_hidden to constrain scroll
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(content)
            )
    }
}
