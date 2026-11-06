//! Blueprint Editor Toolbar
//!
//! Top toolbar rendered above all workspace panels. Follows the exact same
//! 48-px, separator-grouped, icon-button language used by the Level Editor
//! toolbar so the two feel visually continuous.
//!
//! # Group layout (left → right)
//!
//! ```text
//! [ Save ] | [ 🔨 Compile ] | [ Comment ] | [ 🔍 Find ] | [ Map Bug ⚙ ] ···flex··· status  [ 📦 Name ● ]
//! ```
//!
//! The compile button changes colour and label reactively:
//! - **Idle**      – neutral secondary (needs compile)
//! - **Compiling** – warning / loading spinner
//! - **Success**   – success green
//! - **Error**     – danger red

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Disableable, Icon, IconName,
};

use crate::actions::{AddGraphComment, CompileShader, ReloadShaderFromDisk};
use crate::features::compilation::CompilationState;
use crate::editor::panel::{MeshType, ShaderEditorPanel};
use crate::features::shader_model;

// ─────────────────────────────────────────────────────────────────────────────
// Public renderer
// ─────────────────────────────────────────────────────────────────────────────

pub struct ToolbarRenderer;

impl ToolbarRenderer {
    pub fn render(
        panel: &ShaderEditorPanel,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> impl IntoElement {
        // ── Snapshot mutable state before building the element tree ──────────
        // (avoids multiple re-borrows of `panel` inside closures)
        let compile_state = panel.compilation_status.state.clone();
        let is_compiling = panel.compilation_status.is_compiling;
        let is_dirty = panel.is_dirty();
        let previews_on = panel.previews.enabled();
        let shader_name = panel
            .tab_title
            .clone()
            .unwrap_or_else(|| "Shader Editor".to_string());
        let current_shader_model = panel.shader_model.clone();
        let current_preview_mesh = panel.preview_mesh;
        let preview_auto_rotate = panel.preview_auto_rotate;

        // ── Compile-button icon (reflects last result) ───────────────────────
        let compile_icon = match &compile_state {
            CompilationState::Success => IconName::BadgeCheck,
            CompilationState::Error => IconName::X,
            _ => IconName::Flash,
        };

        // ── Right-side status badge ──────────────────────────────────────────
        let show_status = compile_state != CompilationState::Idle;
        let status_text = panel.compilation_status.message.clone();
        let status_color: Hsla = match &compile_state {
            CompilationState::Compiling => cx.theme().warning,
            CompilationState::Success => cx.theme().success,
            CompilationState::Error => cx.theme().danger,
            CompilationState::Idle => cx.theme().muted_foreground,
        };

        // ── Edit menu (Pulsar-Native#1012): the active graph's history ───────
        let history = Some(panel.history.clone());
        let mut menus = plugin_editor_api::EditorMenuStrip::new("shader-editor-menus").menu(
            plugin_editor_api::EditorMenu::new("Edit").undo_redo(
                "Undo",
                "Redo",
                history.map(|history| history as _),
            ),
        );
        // ── Tools menu: the shader's own tools ───────────────────────────────
        menus = menus.menu(
            plugin_editor_api::EditorMenu::new("Tools")
                .action_if("Compile Shader", CompileShader, !is_compiling)
                .action("Reload from Disk", ReloadShaderFromDisk)
                .separator()
                .action("Add Comment", AddGraphComment),
        );
        // Undo/Redo reach the graph the menu is for.
        menus = menus.action_context(panel.canvas.read(cx).focus_handle(cx));

        // ── Assemble toolbar ─────────────────────────────────────────────────
        h_flex()
            .w_full()
            .h(px(48.0))
            .px_4()
            .gap_3()
            .items_center()
            // Same surface treatment as the level-editor toolbar
            .bg(cx.theme().sidebar.opacity(0.98))
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.8))
            .shadow_sm()
            .child(menus)
            .child(toolbar_separator(cx))
            // ── Group 1 · File ───────────────────────────────────────────────
            .child(
                h_flex().gap_1p5().items_center().child(
                    Button::new("toolbar-save")
                        .icon(IconName::FloppyDisk)
                        // Unsaved-changes dot keeps the user informed without a modal
                        .tooltip(if is_dirty {
                            "Save Shader (Ctrl+S)  ●"
                        } else {
                            "Save Shader (Ctrl+S)"
                        })
                        .on_click(cx.listener(|panel, _, window, cx| {
                            panel.plugin_save(window, cx);
                        })),
                ),
            )
            .child(toolbar_separator(cx))
            // ── Group 2 · Compile ────────────────────────────────────────────
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    // Build the base button, then apply colour variant in a
                    // single match so the type stays `Button` throughout.
                    .child({
                        let btn = Button::new("toolbar-compile")
                            .icon(compile_icon)
                            .label(if is_compiling {
                                "Compiling…"
                            } else {
                                "Compile"
                            })
                            .loading(is_compiling)
                            .disabled(is_compiling)
                            .tooltip("Compile Shader (F7)")
                            .on_click(cx.listener(|panel, _, _window, cx| {
                                panel.start_compilation(cx);
                            }));

                        match compile_state {
                            CompilationState::Success => btn.success(),
                            CompilationState::Error => btn.danger(),
                            CompilationState::Compiling => btn.warning(),
                            CompilationState::Idle => btn,
                        }
                    }),
            )
            .child(toolbar_separator(cx))
            // ── Group 3 · Blueprint Graph Editing ────────────────────────────
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    // Reload resets the graph to the last saved version
                    .child(
                        Button::new("toolbar-reload")
                            .icon(IconName::Refresh)
                            .tooltip("Reload Material from Disk")
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.plugin_reload(window, cx);
                            })),
                    )
                    // Add Comment box at the centre of the current viewport
                    .child(
                        Button::new("toolbar-add-comment")
                            .icon(IconName::Message)
                            .tooltip("Add Comment to Graph")
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.add_comment(cx);
                            })),
                    ),
            )
            .child(toolbar_separator(cx))
            // ── Group 4 · Find & Navigate ────────────────────────────────────
            .child(
                h_flex().gap_1p5().items_center().child(
                    Button::new("toolbar-find")
                        .icon(IconName::Search)
                        .tooltip("Find in Graph (Ctrl+F)")
                        .on_click(cx.listener(|_panel, _, _window, _cx| {
                            // TODO: focus the Find Results panel in the
                            // workspace dock when the workspace API supports
                            // programmatic panel activation.
                        })),
                ),
            )
            .child(toolbar_separator(cx))
            // ── Group 5 · View Toggles ───────────────────────────────────────
            // Matches the level-editor toggle pattern:
            //   inactive → secondary (default), active → primary
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child({
                        // Debugging aid, off by default: live per-pin texture
                        // previews. Each compiles its own shader and renders
                        // every frame, so shaders only compile when the user
                        // presses Compile unless this is switched on.
                        let btn = Button::new("toolbar-debug-previews")
                            .icon(IconName::Bug)
                            .tooltip("Debug: live pin texture previews (compiles on every edit)")
                            .on_click(cx.listener(|panel, _, _, cx| {
                                let enabled = !panel.previews.enabled();
                                panel.previews.set_enabled(enabled);
                                // The presenter reserves (or frees) the preview rows.
                                panel.canvas.update(cx, |canvas, cx| canvas.refresh_scene(cx));
                                cx.notify();
                            }));
                        if previews_on {
                            btn.primary()
                        } else {
                            btn
                        }
                    })
            )
            // ── Group 6 · Shader Model & Preview ─────────────────────────────
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child({
                        let models = shader_model::get_all_models();
                        let current = current_shader_model.clone();
                        let current_name = models.iter()
                            .find(|m| m.id == current)
                            .map(|m| m.name.as_str())
                            .unwrap_or("Shader Model");
                        Button::new("toolbar-shader-model")
                            .label(current_name)
                            .tooltip("Shader Model (click to cycle)")
                            .on_click(cx.listener(|panel, _, _window, cx| {
                                let models = shader_model::get_all_models();
                                if models.is_empty() { return; }
                                let current_idx = models.iter()
                                    .position(|m| m.id == panel.shader_model)
                                    .unwrap_or(0);
                                let next_idx = (current_idx + 1) % models.len();
                                panel.shader_model = models[next_idx].id.clone();
                                cx.notify();
                            }))
                    })
                    .child({
                        let mesh_name = current_preview_mesh.name();
                        Button::new("toolbar-preview-mesh")
                            .label(mesh_name)
                            .tooltip("Preview Mesh (click to cycle)")
                            .on_click(cx.listener(|panel, _, _window, cx| {
                                let variants = MeshType::variants();
                                let current_idx = variants.iter()
                                    .position(|m| *m == panel.preview_mesh)
                                    .unwrap_or(0);
                                let next_idx = (current_idx + 1) % variants.len();
                                panel.preview_mesh = variants[next_idx];
                                cx.notify();
                            }))
                    })
                    .child({
                        let btn = Button::new("toolbar-auto-rotate")
                            .label("Auto")
                            .tooltip("Toggle auto-rotate preview");
                        if preview_auto_rotate { btn.primary() } else { btn }
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.preview_auto_rotate = !panel.preview_auto_rotate;
                                cx.notify();
                            }))
                    }),
            )
            .child(toolbar_separator(cx))
            // ── Flex spacer pushes right-side content to the edge ────────────
            .child(div().flex_1())
            // ── Right side · Compile status + Blueprint name pill ────────────
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    // Live compile status text (hidden when idle)
                    .when(show_status, |el| {
                        el.child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(status_color)
                                .child(status_text),
                        )
                    })
                    // Blueprint identity pill ─────────────────────────────────
                    // Shows the class name and an unsaved-changes dot
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .px_3()
                            .h_8()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted.opacity(0.3))
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .child(
                                Icon::new(IconName::Component)
                                    .size(px(14.0)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(shader_name),
                            )
                            // Amber dot when there are unsaved changes
                            .when(is_dirty, |el| {
                                el.child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(cx.theme().warning)
                                        .flex_shrink_0(),
                                )
                            }),
                    ),
            )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Thin vertical separator – identical to the level-editor toolbar separator.
fn toolbar_separator(cx: &mut Context<ShaderEditorPanel>) -> impl IntoElement {
    div()
        .h_6()
        .w_px()
        .bg(cx.theme().border.opacity(0.4))
        .flex_shrink_0()
}
