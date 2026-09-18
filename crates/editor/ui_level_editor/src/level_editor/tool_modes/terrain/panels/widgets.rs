//! The widget kit shared by Terrain's dock panels, styled after Unreal's
//! Landscape/Foliage panels: an icon-over-label **tool grid** whose active
//! tool is highlighted, **collapsible sections** with a chevron, dark inset
//! **value boxes** (`− 25.0 +`), plain checkbox rows, and a swatch grid.
//!
//! Every helper is a free function returning a GPUI element and holds no
//! state. Mutations go through a caller-supplied closure over
//! [`TerrainDomain`]: every control in these panels edits brush/library
//! configuration, never the scene, so the domain's clamped setters are the
//! whole write surface.

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable,
};

use crate::level_editor::state::terrain::TerrainDomain;
use crate::level_editor::state::LevelEditorState;

pub type SharedState = Arc<parking_lot::RwLock<LevelEditorState>>;

// ── Headers & sections ────────────────────────────────────────────────────

/// A panel heading: title plus a muted subtitle line.
pub fn panel_header(theme: &ui::Theme, title: String, subtitle: String) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(subtitle),
        )
}

/// A non-collapsible upper-case section label with a hairline rule.
pub fn section(theme: &ui::Theme, label_key: &'static str) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_1()
        .pt_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(t!(label_key).to_string().to_uppercase()),
        )
        .child(div().w_full().h_px().bg(theme.border.opacity(0.4)))
}

/// A collapsible section header (chevron + label + optional trailing
/// element). The caller owns the collapsed set and passes a click handler —
/// usually `cx.listener(..)` — that toggles it.
pub fn collapsible_header(
    theme: &ui::Theme,
    id: &'static str,
    label_key: &'static str,
    collapsed: bool,
    trailing: Option<AnyElement>,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut row = h_flex()
        .id(SharedString::from(format!("section_{id}")))
        .w_full()
        .items_center()
        .gap_1()
        .pt_2()
        .pb_1()
        .px_1()
        .border_b_1()
        .border_color(theme.border.opacity(0.5))
        .cursor_pointer()
        .on_click(on_toggle)
        .child(
            Icon::new(if collapsed {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .size_3p5()
            .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!(label_key).to_string()),
        );
    if let Some(trailing) = trailing {
        row = row.child(trailing);
    }
    row
}

// ── Tool grid ─────────────────────────────────────────────────────────────

/// One button of a [`tool_grid`].
pub struct ToolSpec {
    pub id: &'static str,
    pub icon: IconName,
    pub label_key: &'static str,
    pub active: bool,
    /// Runs on click; a tool button both selects and activates its tool.
    pub apply: Arc<dyn Fn(&mut TerrainDomain) + Send + Sync>,
}

/// A wrapping row of icon-over-label tool buttons, the active one
/// highlighted — Unreal's tool strip.
pub fn tool_grid(theme: &ui::Theme, state: SharedState, tools: Vec<ToolSpec>) -> impl IntoElement {
    let mut grid = h_flex().w_full().flex_wrap().gap_1();
    for tool in tools {
        let state = state.clone();
        let apply = tool.apply.clone();
        let (bg, border, fg) = if tool.active {
            (
                theme.primary.opacity(0.28),
                theme.primary,
                theme.foreground,
            )
        } else {
            (
                theme.muted.opacity(0.10),
                theme.border.opacity(0.6),
                theme.muted_foreground,
            )
        };
        let hover_bg = theme.muted.opacity(0.24);
        grid = grid.child(
            v_flex()
                .id(SharedString::from(format!("tool_{}", tool.id)))
                .w(px(62.0))
                .h(px(54.0))
                .items_center()
                .justify_center()
                .gap_1()
                .rounded(px(4.0))
                .border_1()
                .border_color(border)
                .bg(bg)
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_click(move |_, _, _| {
                    let mut st = state.write();
                    apply(&mut st.editor.terrain);
                })
                .child(Icon::new(tool.icon).size_5())
                .child(
                    div()
                        .text_xs()
                        .font_weight(if tool.active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(t!(tool.label_key).to_string()),
                ),
        );
    }
    grid
}

// ── Rows ──────────────────────────────────────────────────────────────────

/// A one-of-many picker rendered as a row of small toggle buttons.
pub fn segmented_row<V, F>(
    state: SharedState,
    cx: &mut Context<V>,
    control_id: &'static str,
    options: &[(&'static str, &'static str)],
    selected: &'static str,
    apply: F,
) -> impl IntoElement
where
    V: 'static,
    F: Fn(&mut TerrainDomain, &'static str) + Clone + Send + Sync + 'static,
{
    let mut group = h_flex()
        .w_full()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .bg(cx.theme().muted.opacity(0.1))
        .p(px(2.0))
        .gap_1();

    for &(label_key, value) in options {
        let is_sel = value == selected;
        let state = state.clone();
        let apply = apply.clone();
        let btn = Button::new(format!("{control_id}_{value}"))
            .label(t!(label_key))
            .small()
            .on_click(move |_, _, _| {
                let mut st = state.write();
                apply(&mut st.editor.terrain, value);
            });
        group = group.child(if is_sel { btn.primary() } else { btn.ghost() });
    }
    group
}

/// Label on the left, an inset `− value +` box on the right (Unreal's
/// numeric field).
#[allow(clippy::too_many_arguments)]
pub fn stepper_row<V, F>(
    state: SharedState,
    cx: &mut Context<V>,
    control_id: String,
    label: String,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    apply: F,
) -> impl IntoElement
where
    V: 'static,
    F: Fn(&mut TerrainDomain, f32) + Clone + Send + Sync + 'static,
{
    let theme = cx.theme();
    let dec_val = (value - step).clamp(min, max);
    let inc_val = (value + step).clamp(min, max);

    let (dec_state, dec_apply) = (state.clone(), apply.clone());
    let dec = Button::new(format!("{control_id}_dec"))
        .icon(IconName::Minus)
        .xsmall()
        .ghost()
        .disabled(value <= min)
        .on_click(move |_, _, _| {
            let mut st = dec_state.write();
            dec_apply(&mut st.editor.terrain, dec_val);
        });

    let inc = Button::new(format!("{control_id}_inc"))
        .icon(IconName::Plus)
        .xsmall()
        .ghost()
        .disabled(value >= max)
        .on_click(move |_, _, _| {
            let mut st = state.write();
            apply(&mut st.editor.terrain, inc_val);
        });

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            h_flex()
                .items_center()
                .rounded(px(3.0))
                .border_1()
                .border_color(theme.border.opacity(0.6))
                .bg(theme.muted.opacity(0.18))
                .child(dec)
                .child(
                    div()
                        .min_w(px(46.0))
                        .text_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format_step(value, step)),
                )
                .child(inc),
        )
}

/// A labelled checkbox that writes through `apply`.
pub fn checkbox_row<F>(
    state: SharedState,
    control_id: String,
    label: String,
    checked: bool,
    apply: F,
) -> impl IntoElement
where
    F: Fn(&mut TerrainDomain, bool) + Send + Sync + 'static,
{
    Checkbox::new(SharedString::from(control_id))
        .label(label)
        .checked(checked)
        .on_click(move |_, _, _| {
            let mut st = state.write();
            apply(&mut st.editor.terrain, !checked);
        })
}

/// A muted `label ........ value` read-only row.
pub fn info_row(theme: &ui::Theme, label: String, value: String) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(value))
}

/// A stable, distinct colour per index (golden-ratio hue walk) — used for
/// material swatches, since the terrain material table has ids but no
/// authored colours or names.
pub fn swatch_color(index: u32) -> Hsla {
    let hue = (index as f32 * 0.618_034).fract();
    hsla(hue, 0.55, 0.45, 1.0)
}

/// Display precision follows the step: whole numbers for step ≥ 1, one
/// decimal for ≥ 0.1, two otherwise.
fn format_step(value: f32, step: f32) -> String {
    if step >= 1.0 {
        format!("{value:.0}")
    } else if step >= 0.1 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::{format_step, swatch_color};

    #[test]
    fn precision_follows_the_step() {
        assert_eq!(format_step(12.0, 1.0), "12");
        assert_eq!(format_step(0.55, 0.1), "0.6");
        assert_eq!(format_step(0.555, 0.01), "0.56");
    }

    #[test]
    fn swatch_colours_differ_for_neighbouring_ids() {
        assert_ne!(swatch_color(1), swatch_color(2));
        assert_eq!(swatch_color(3), swatch_color(3));
    }
}
