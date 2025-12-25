//! Type Debugger Drawer - Studio-quality type database inspection panel
//! Displays all registered types with professional UI and search capabilities

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};
use std::path::PathBuf;
use std::collections::HashMap;
use type_db::{TypeInfo, TypeKind};

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
}

impl TypeDebuggerDrawer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        Self {
            focus_handle,
            types: Vec::new(),
            filtered_kind: None,
            selected_index: None,
            search_query: String::new(),
            group_by_kind: true,
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

    fn render_header(
        &self,
        alias_count: usize,
        struct_count: usize,
        enum_count: usize,
        trait_count: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::Database)
                            .size_4()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(format!("{} Types", total_count)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("filter-all")
                            .small()
                            .when(self.filtered_kind.is_none(), |b| {
                                b.primary()
                            })
                            .when(self.filtered_kind.is_some(), |b| {
                                b.ghost()
                            })
                            .child(format!("All ({})", total_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(None, cx);
                            })),
                    )
                    .child(
                        Button::new("filter-aliases")
                            .small()
                            .when(
                                self.filtered_kind.as_ref() == Some(&TypeKind::Alias),
                                |b| b.primary(),
                            )
                            .when(
                                self.filtered_kind.as_ref() != Some(&TypeKind::Alias),
                                |b| b.ghost(),
                            )
                            .child(format!("Aliases ({})", alias_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(TypeKind::Alias), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-structs")
                            .small()
                            .when(
                                self.filtered_kind.as_ref() == Some(&TypeKind::Struct),
                                |b| b.primary(),
                            )
                            .when(
                                self.filtered_kind.as_ref() != Some(&TypeKind::Struct),
                                |b| b.ghost(),
                            )
                            .child(format!("Structs ({})", struct_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(TypeKind::Struct), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-enums")
                            .small()
                            .when(
                                self.filtered_kind.as_ref() == Some(&TypeKind::Enum),
                                |b| b.primary(),
                            )
                            .when(
                                self.filtered_kind.as_ref() != Some(&TypeKind::Enum),
                                |b| b.ghost(),
                            )
                            .child(format!("Enums ({})", enum_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(TypeKind::Enum), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-traits")
                            .small()
                            .when(
                                self.filtered_kind.as_ref() == Some(&TypeKind::Trait),
                                |b| b.primary(),
                            )
                            .when(
                                self.filtered_kind.as_ref() != Some(&TypeKind::Trait),
                                |b| b.ghost(),
                            )
                            .child(format!("Traits ({})", trait_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(TypeKind::Trait), cx);
                            })),
                    )
                    .child(
                        Button::new("toggle-grouping")
                            .small()
                            .ghost()
                            .icon(if self.group_by_kind {
                                IconName::List
                            } else {
                                IconName::Folder
                            })
                            .child(if self.group_by_kind { "Ungroup" } else { "Group" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_grouping(cx);
                            })),
                    ),
            )
    }

    fn render_empty_state(
        &self,
        container: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();

        container.child(
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    Icon::new(IconName::Database)
                        .size(px(64.))
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_lg()
                        .text_color(theme.muted_foreground)
                        .child("No types found"),
                )
                .when(!self.search_query.is_empty(), |container| {
                    container.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("No types match '{}'", self.search_query)),
                    )
                }),
        )
    }

    fn render_type_item(
        &self,
        type_info: &TypeInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let type_info_clone = type_info.clone();

        div()
            .w_full()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .hover(|style| style.bg(theme.secondary))
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                this.navigate_to_type(&type_info_clone, cx);
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                self.kind_icon(&type_info.type_kind)
                                    .size_4()
                                    .text_color(self.kind_color(&type_info.type_kind, cx)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child(type_info.display_name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("[ID: {}]", type_info.id)),
                            ),
                    )
                    .when(type_info.description.is_some(), |container| {
                        container.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(type_info.description.as_ref().unwrap().clone()),
                        )
                    })
                    .when(type_info.file_path.is_some(), |container| {
                        container.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(
                                    type_info
                                        .file_path
                                        .as_ref()
                                        .unwrap()
                                        .to_string_lossy()
                                        .to_string(),
                                ),
                        )
                    }),
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
            TypeKind::Alias => Hsla { h: 210.0, s: 0.80, l: 0.60, a: 1.0 }, // Blue
            TypeKind::Struct => Hsla { h: 150.0, s: 0.70, l: 0.50, a: 1.0 }, // Green
            TypeKind::Enum => Hsla { h: 38.0, s: 0.95, l: 0.55, a: 1.0 },   // Orange
            TypeKind::Trait => Hsla { h: 280.0, s: 0.75, l: 0.60, a: 1.0 }, // Purple
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
        container: Div,
        types: Vec<TypeInfo>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut result = container;
        for type_info in types {
            result = result.child(self.render_type_item(&type_info, cx));
        }
        result
    }

    fn render_grouped_view(
        &self,
        container: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let grouped = self.get_grouped_types();
        let mut result = container;

        // Order: Aliases, Structs, Enums, Traits
        for kind in [TypeKind::Alias, TypeKind::Struct, TypeKind::Enum, TypeKind::Trait] {
            if let Some(types) = grouped.get(&kind) {
                if !types.is_empty() {
                    let theme = cx.theme();
                    // Group header
                    result = result.child(
                        div()
                            .w_full()
                            .px_4()
                            .py_2()
                            .bg(theme.secondary)
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        self.kind_icon(&kind)
                                            .size_4()
                                            .text_color(self.kind_color(&kind, cx)),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child(format!("{} ({})", Self::kind_label(&kind), types.len())),
                                    ),
                            ),
                    );

                    // Group items
                    for type_info in types {
                        result = result.child(self.render_type_item(type_info, cx));
                    }
                }
            }
        }

        result
    }
}

impl Focusable for TypeDebuggerDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TypeDebuggerDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let alias_count = self.count_by_kind(TypeKind::Alias);
        let struct_count = self.count_by_kind(TypeKind::Struct);
        let enum_count = self.count_by_kind(TypeKind::Enum);
        let trait_count = self.count_by_kind(TypeKind::Trait);
        let total_count = self.total_count();

        let filtered_types = self.get_filtered_types();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_header(
                alias_count, struct_count, enum_count, trait_count, total_count, cx
            ))
            .child(
                div()
                    .flex_1()
                    .when(filtered_types.is_empty(), |container| {
                        self.render_empty_state(container, cx)
                    })
                    .when(!filtered_types.is_empty(), |container| {
                        if self.group_by_kind {
                            self.render_grouped_view(container, cx)
                        } else {
                            self.render_flat_view(container, filtered_types, cx)
                        }
                    })
            )
    }
}
