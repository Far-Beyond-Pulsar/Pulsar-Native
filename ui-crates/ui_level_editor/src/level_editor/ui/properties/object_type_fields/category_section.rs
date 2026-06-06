//! Collapsible property-category group rendering.
//!
//! A "category" is an optional grouping declared on reflected properties via
//! `#[reflect(category = "Physics", category_color = "#FF6B6B")]`.
//! Multiple properties that share the same category name are collected into a
//! single collapsible section with a coloured header.

use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use super::ObjectTypeFieldsSection;

/// Parses a CSS-style hex colour string (with or without `#`) into [`Hsla`].
/// Supports 6-digit RGB and 8-digit RGBA forms.
pub(super) fn parse_hex_category_color(hex: &str) -> Option<Hsla> {
    let raw = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    match raw.len() {
        6 => u32::from_str_radix(raw, 16).ok().map(rgb).map(Into::into),
        8 => u32::from_str_radix(raw, 16)
            .ok()
            .map(rgba)
            .map(Into::into),
        _ => None,
    }
}

impl ObjectTypeFieldsSection {
    /// Converts a sorted list of `(category_name, rows, color_hex, default_collapsed, order)`
    /// tuples into rendered collapsible section elements.
    ///
    /// Collapse state is read from and written back to
    /// `self.collapsed_property_categories` / `self.expanded_property_categories`.
    pub(super) fn render_categorized_rows(
        &mut self,
        class_name: &str,
        mut categorized_rows: Vec<(String, Vec<AnyElement>, Option<String>, bool, usize)>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        categorized_rows.sort_by_key(|(_, _, _, _, order)| *order);

        categorized_rows
            .into_iter()
            .map(|(category_name, category_rows, category_color_hex, default_collapsed, _)| {
                let category_key = (class_name.to_string(), category_name.clone());

                let is_collapsed =
                    if self.collapsed_property_categories.contains(&category_key) {
                        true
                    } else if self.expanded_property_categories.contains(&category_key) {
                        false
                    } else {
                        default_collapsed
                    };

                let toggle_key = category_key.clone();
                let was_collapsed = is_collapsed;
                let accent = category_color_hex
                    .as_deref()
                    .and_then(parse_hex_category_color);

                v_flex()
                    .w_full()
                    .gap_1()
                    .p_2()
                    .rounded(px(6.0))
                    .border_1()
                    .when_some(accent, |el, color| {
                        el.border_color(color.opacity(0.7)).bg(color.opacity(0.08))
                    })
                    .when(accent.is_none(), |el| {
                        el.border_color(cx.theme().border)
                            .bg(cx.theme().border.opacity(0.08))
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    if was_collapsed {
                                        this.collapsed_property_categories.remove(&toggle_key);
                                        this.expanded_property_categories
                                            .insert(toggle_key.clone());
                                    } else {
                                        this.expanded_property_categories.remove(&toggle_key);
                                        this.collapsed_property_categories
                                            .insert(toggle_key.clone());
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when_some(accent, |el, color| el.text_color(color))
                                    .when(accent.is_none(), |el| {
                                        el.text_color(cx.theme().muted_foreground)
                                    })
                                    .child(category_name),
                            )
                            .child(
                                Icon::new(if is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .xsmall()
                                .when_some(accent, |el, color| el.text_color(color))
                                .when(accent.is_none(), |el| {
                                    el.text_color(cx.theme().muted_foreground)
                                }),
                            ),
                    )
                    .when(!is_collapsed, |el| el.children(category_rows))
                    .into_any_element()
            })
            .collect()
    }
}

/// Collects a flat list of `(row, category?, color?, default_collapsed, order)` property rows
/// into separate uncategorised and categorised buckets.
///
/// Returns `(uncategorized_rows, categorized_rows)` where categorised rows are grouped
/// by category name and ready to pass to [`ObjectTypeFieldsSection::render_categorized_rows`].
pub(super) fn group_rows_by_category(
    rows: Vec<(
        AnyElement,
        Option<String>,
        Option<String>,
        bool,
        Option<usize>,
    )>,
) -> (
    Vec<AnyElement>,
    Vec<(String, Vec<AnyElement>, Option<String>, bool, usize)>,
) {
    let mut uncategorized: Vec<AnyElement> = Vec::new();
    let mut categorized: Vec<(String, Vec<AnyElement>, Option<String>, bool, usize)> = Vec::new();
    let mut category_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (row, category, category_color, default_collapsed, category_order) in rows {
        if let Some(cat) = category.filter(|c| !c.trim().is_empty()) {
            if let Some(&ix) = category_index.get(&cat) {
                let entry = &mut categorized[ix];
                entry.1.push(row);
                if entry.2.is_none() {
                    entry.2 = category_color;
                }
                entry.3 = entry.3 || default_collapsed;
                if category_order.unwrap_or(usize::MAX) < entry.4 {
                    entry.4 = category_order.unwrap_or(usize::MAX);
                }
            } else {
                let ix = categorized.len();
                category_index.insert(cat.clone(), ix);
                categorized.push((
                    cat,
                    vec![row],
                    category_color,
                    default_collapsed,
                    category_order.unwrap_or(usize::MAX),
                ));
            }
        } else {
            uncategorized.push(row);
        }
    }

    (uncategorized, categorized)
}
