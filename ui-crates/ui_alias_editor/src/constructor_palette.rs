use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, divider::Divider, ActiveTheme, StyledExt, button::{Button, ButtonVariants}, input::TextInput, Colorize};
use pulsar_std::{get_all_type_constructors, get_type_constructors_by_category, TypeConstructorMetadata};
use ui_types_common::PRIMITIVES;
use std::collections::HashMap;
use crate::type_block::TypeBlock;

/// Event emitted when a type is selected from the palette
#[derive(Clone, Debug)]
pub struct TypeSelected {
    pub block: TypeBlock,
}

/// Palette of available type constructors (like Scratch's block palette)
/// Now dynamically loaded from the type constructor registry!
pub struct ConstructorPalette {
    categories: Vec<CategoryData>,
    search_query: String,
    collapsed_categories: HashMap<String, bool>,
    show_primitives: bool,
}

struct CategoryData {
    name: String,
    icon: &'static str,
    constructors: Vec<&'static TypeConstructorMetadata>,
}

impl ConstructorPalette {
    pub fn new() -> Self {
        // Load all type constructors from the registry
        let all_constructors = get_all_type_constructors();

        // Group by category
        let mut categories_map: HashMap<String, Vec<&'static TypeConstructorMetadata>> = HashMap::new();

        for constructor in all_constructors {
            categories_map
                .entry(constructor.category.to_string())
                .or_insert_with(Vec::new)
                .push(constructor);
        }

        // Map categories to icons
        let category_icons: HashMap<&str, &'static str> = [
            ("Smart Pointers", "📦"),
            ("Option & Result", "🎯"),
            ("Collections", "📚"),
            ("Interior Mutability", "🔒"),
            ("Other", "🔧"),
        ]
        .iter()
        .copied()
        .collect();

        // Convert to CategoryData with consistent ordering
        let category_order = [
            "Smart Pointers",
            "Option & Result",
            "Collections",
            "Interior Mutability",
            "Other",
        ];

        let mut categories = Vec::new();

        for &category_name in &category_order {
            if let Some(constructors) = categories_map.get(category_name) {
                categories.push(CategoryData {
                    name: category_name.to_string(),
                    icon: category_icons.get(category_name).copied().unwrap_or("🔧"),
                    constructors: constructors.clone(),
                });
            }
        }

        // Add any categories not in the predefined order
        for (category_name, constructors) in categories_map {
            if !category_order.contains(&category_name.as_str()) {
                categories.push(CategoryData {
                    name: category_name.clone(),
                    icon: "🔧",
                    constructors,
                });
            }
        }

        Self { 
            categories,
            search_query: String::new(),
            collapsed_categories: HashMap::new(),
            show_primitives: true,
        }
    }

    /// Set search query and filter results
    pub fn set_search(&mut self, query: String) {
        self.search_query = query;
    }

    /// Toggle category collapsed state
    pub fn toggle_category(&mut self, category_name: &str) {
        let collapsed = self.collapsed_categories.get(category_name).copied().unwrap_or(false);
        self.collapsed_categories.insert(category_name.to_string(), !collapsed);
    }

    /// Check if category is collapsed
    fn is_category_collapsed(&self, category_name: &str) -> bool {
        self.collapsed_categories.get(category_name).copied().unwrap_or(false)
    }

    /// Filter constructors based on search query
    fn filter_constructors<'a>(&'a self, constructors: &'a [&'static TypeConstructorMetadata]) -> Vec<&'static TypeConstructorMetadata> {
        if self.search_query.is_empty() {
            return constructors.to_vec();
        }

        let query_lower = self.search_query.to_lowercase();
        constructors
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&query_lower)
                    || c.description.to_lowercase().contains(&query_lower)
                    || c.category.to_lowercase().contains(&query_lower)
            })
            .copied()
            .collect()
    }

    /// Filter primitives based on search query
    fn filter_primitives(&self) -> Vec<&'static str> {
        if self.search_query.is_empty() {
            return PRIMITIVES.to_vec();
        }

        let query_lower = self.search_query.to_lowercase();
        PRIMITIVES
            .iter()
            .filter(|p| p.to_lowercase().contains(&query_lower))
            .copied()
            .collect()
    }

    pub fn render(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .w(px(320.0))
            .h_full()
            .bg(cx.theme().sidebar)
            .border_r_2()
            .border_color(cx.theme().border)
            .child(
                // Header with search
                v_flex()
                    .w_full()
                    .bg(cx.theme().secondary)
                    .border_b_2()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .px_4()
                            .py_3()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_bold()
                                    .text_color(cx.theme().foreground)
                                    .child("🎨 Type Library")
                            )
                    )
                    .child(
                        // Search box
                        h_flex()
                            .w_full()
                            .px_3()
                            .pb_3()
                            .child(
                                div()
                                    .w_full()
                                    .child("🔍 Search...")  // Placeholder for TextInput
                            )
                    )
            )
            .child(
                // Scrollable categories
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_3()
                    .when(self.show_primitives, |this| {
                        let filtered = self.filter_primitives();
                        if !filtered.is_empty() {
                            this.child(self.render_primitives_category(&filtered, cx))
                        } else {
                            this
                        }
                    })
                    .children(
                        self.categories
                            .iter()
                            .filter_map(|category| {
                                let filtered = self.filter_constructors(&category.constructors);
                                if !filtered.is_empty() {
                                    Some(self.render_category_with_filtered(category, &filtered, cx))
                                } else {
                                    None
                                }
                            })
                    )
                    .when(!self.search_query.is_empty() && self.has_no_results(), |this| {
                        this.child(self.render_no_results(cx))
                    })
            )
            .child(
                // Footer hint
                div()
                    .px_4()
                    .py_3()
                    .bg(cx.theme().secondary.opacity(0.5))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("💡 Drag types to the canvas or click to add")
                    )
            )
    }

    fn has_no_results(&self) -> bool {
        self.filter_primitives().is_empty() && 
        self.categories.iter().all(|c| self.filter_constructors(&c.constructors).is_empty())
    }

    fn render_no_results(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .w_full()
            .items_center()
            .justify_center()
            .p_6()
            .gap_2()
            .child(
                div()
                    .text_2xl()
                    .child("🔍")
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("No types found for \"{}\"", self.search_query))
            )
    }

    fn render_primitives_category(&self, primitives: &[&'static str], cx: &App) -> impl IntoElement {
        let is_collapsed = self.is_category_collapsed("Primitives");
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                // Category header - clickable to collapse/expand
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().muted.opacity(0.3))
                    .rounded(px(6.0))
                    .hover(|s| s.bg(cx.theme().muted.opacity(0.4)))
                    .cursor_pointer()
                    .child(
                        div()
                            .text_sm()
                            .child(if is_collapsed { "▶" } else { "▼" })
                    )
                    .child(
                        div()
                            .text_base()
                            .child("🔢")
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(format!("Primitives ({})", primitives.len()))
                    )
            )
            .when(!is_collapsed, |this| {
                this.child(
                    // Primitive blocks in a grid
                    div()
                        .w_full()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .px_2()
                        .children(primitives.iter().map(|prim| {
                            self.render_primitive_block(prim, cx)
                        }))
                )
            })
    }

    fn render_primitive_block(&self, name: &str, cx: &App) -> impl IntoElement {
        let color = hsla(0.55, 0.7, 0.5, 1.0); // Blue for primitives
        
        div()
            .px_3()
            .py_2()
            .bg(color)
            .rounded(px(6.0))
            .border_1()
            .border_color(color.lighten(0.1))
            .hover(|s| s.bg(color.lighten(0.1)).shadow_md())
            .cursor_pointer()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(gpui::white())
                    .child(name.to_string())
            )
    }

    fn render_category_with_filtered(&self, category: &CategoryData, filtered: &[&'static TypeConstructorMetadata], cx: &App) -> impl IntoElement {
        let is_collapsed = self.is_category_collapsed(&category.name);
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                // Category header
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().muted.opacity(0.3))
                    .rounded(px(6.0))
                    .hover(|s| s.bg(cx.theme().muted.opacity(0.4)))
                    .cursor_pointer()
                    .child(
                        div()
                            .text_sm()
                            .child(if is_collapsed { "▶" } else { "▼" })
                    )
                    .child(
                        div()
                            .text_base()
                            .child(category.icon)
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(format!("{} ({})", category.name, filtered.len()))
                    )
            )
            .when(!is_collapsed, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .px_2()
                        .children(filtered.iter().map(|constructor| {
                            self.render_constructor_block(constructor, cx)
                        }))
                )
            })
    }

    fn render_constructor_block(&self, constructor: &TypeConstructorMetadata, cx: &App) -> impl IntoElement {
        let color = hsla(0.08, 0.8, 0.6, 1.0); // Orange for constructors
        
        v_flex()
            .w_full()
            .gap_1()
            .child(
                // Main block
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(color)
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(color.lighten(0.1))
                    .hover(|s| s.bg(color.lighten(0.1)).shadow_md())
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_bold()
                            .text_color(gpui::white())
                            .child(format!("{}<>", constructor.name))
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .bg(gpui::white().opacity(0.2))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(gpui::white())
                            .child(format!("{}", constructor.params_count))
                    )
            )
            .child(
                // Description
                div()
                    .w_full()
                    .px_3()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(constructor.description)
            )
    }

    fn _render_category(&self, category: &CategoryData, cx: &App) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                // Category header
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_base()
                            .child(category.icon)
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(category.name.clone())
                    )
            )
            .child(
                // Blocks
                v_flex()
                    .gap_1()
                    .pl_4()
                    .children(
                        category.constructors.iter().map(|constructor| {
                            self.render_constructor_block(constructor, cx)
                        })
                    )
            )
    }

    fn render_palette_block(&self, constructor: &TypeConstructorMetadata, cx: &mut App) -> impl IntoElement {
        let color = hsla(0.08, 0.8, 0.6, 1.0); // Orange for constructors

        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .bg(color)
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(color.lighten(0.1))
                    .shadow_sm()
                    .cursor_pointer()
                    .hover(|style| {
                        style.bg(color.lighten(0.1)).shadow_md()
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .text_color(gpui::white())
                            .child(constructor.name)
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(gpui::white().opacity(0.7))
                            .child(format!("<{}params>", constructor.params_count))
                    )
            )
            .child(
                div()
                    .pl_3()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(constructor.description)
            )
    }
}

impl Default for ConstructorPalette {
    fn default() -> Self {
        Self::new()
    }
}
