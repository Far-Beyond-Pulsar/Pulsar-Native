//! Rendering - GPUI render implementation and trait implementations

use gpui::prelude::*;
use gpui::*;
use ui::{
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    scroll::Scrollbar,
    v_flex, v_virtual_list, ActiveTheme,
};

use super::panel::ShaderEditorPanel;
use super::toolbar::ToolbarRenderer;
use crate::actions::*;
use crate::features::compilation::CompilationState;

impl Panel for ShaderEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Shader Editor"
    }

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.current_material_path.clone()
    }

    fn tab_unsaved(&self, _cx: &App) -> bool {
        self.is_dirty()
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().child(if let Some(title) = &self.tab_title {
                title.clone()
            } else {
                "Shader Editor".to_string()
            }))
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

impl Focusable for ShaderEditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ShaderEditorPanel {}

impl ShaderEditorPanel {
    /// Render compiler results panel (compilation history and status)
    pub fn render_compiler_results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {

        let history_entries: Vec<_> = self.compilation_history.iter().rev().cloned().collect();
        let item_sizes = std::rc::Rc::new(
            history_entries
                .iter()
                .map(|_| size(px(0.0), px(56.0)))
                .collect::<Vec<_>>(),
        );
        // Current preview build failures (title, message): listed until the
        // material/pin builds again successfully or the pin goes away.
        let mut preview_errors: Vec<(String, String)> = Vec::new();
        if let Some(message) = &self.material_preview_error {
            preview_errors.push(("Material preview".to_string(), message.clone()));
        }
        preview_errors.extend(self.previews.errors().into_iter().map(|error| {
            (
                format!("Pin preview: {}", error.label),
                error.message,
            )
        }));
        let compiler_entity = cx.entity().clone();
        let scroll_handle = self.compiler_output_scroll_handle.clone();
        let scrollbar_state = self.compiler_output_scrollbar_state.clone();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(match self.compilation_status.state {
                                CompilationState::Success => gpui::green(),
                                CompilationState::Error => gpui::red(),
                                CompilationState::Compiling => gpui::yellow(),
                                _ => cx.theme().foreground,
                            })
                            .child(match self.compilation_status.state {
                                CompilationState::Idle => "Compiler Output",
                                CompilationState::Compiling => "⟳ Compiling...",
                                CompilationState::Success => "✓ Build Succeeded",
                                CompilationState::Error => "✗ Build Failed",
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} entries", self.compilation_history.len())),
                    ),
            )
            .when(!preview_errors.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("compiler-preview-errors")
                        .w_full()
                        .max_h(px(160.0))
                        .overflow_y_scroll()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .children(preview_errors.into_iter().map(|(title, message)| {
                            v_flex()
                                .w_full()
                                .px_2()
                                .py_1()
                                .gap_0p5()
                                .border_l_2()
                                .border_color(cx.theme().danger)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().danger)
                                        .child(format!("✗ {title}")),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(message),
                                )
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .relative()
                    .when(history_entries.is_empty(), |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No compilation messages yet."),
                        )
                    })
                    .when(!history_entries.is_empty(), |this| {
                        this.child(
                            v_virtual_list(
                                compiler_entity,
                                "compiler-history-list",
                                item_sizes,
                                move |_panel, range, _window, cx| {
                                    range
                                        .map(|ix| -> AnyElement {
                                            let Some(entry) = history_entries.get(ix) else {
                                                return div().h(px(56.0)).into_any_element();
                                            };

                                            let accent = match entry.state {
                                                CompilationState::Success => cx.theme().success,
                                                CompilationState::Error => cx.theme().danger,
                                                CompilationState::Compiling => cx.theme().warning,
                                                CompilationState::Idle => {
                                                    cx.theme().muted_foreground.opacity(0.7)
                                                }
                                            };

                                            let icon = match entry.state {
                                                CompilationState::Success => "✓",
                                                CompilationState::Error => "✗",
                                                CompilationState::Compiling => "•",
                                                CompilationState::Idle => "•",
                                            };

                                            h_flex()
                                                .w_full()
                                                .h(px(56.0))
                                                .px_2()
                                                .py_1()
                                                .gap_2()
                                                .border_b_1()
                                                .border_color(cx.theme().border.opacity(0.1))
                                                .hover(|s| s.bg(cx.theme().muted.opacity(0.06)))
                                                .child(
                                                    div()
                                                        .w(px(2.0))
                                                        .h_full()
                                                        .rounded_full()
                                                        .bg(accent)
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    v_flex()
                                                        .w(px(76.0))
                                                        .gap_0p5()
                                                        .flex_shrink_0()
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .font_family(
                                                                    "JetBrainsMono-Regular",
                                                                )
                                                                .text_color(
                                                                    cx.theme()
                                                                        .muted_foreground
                                                                        .opacity(0.8),
                                                                )
                                                                .child(entry.timestamp.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(accent)
                                                                .child(entry.stage.to_uppercase()),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(14.0))
                                                        .text_xs()
                                                        .text_color(accent)
                                                        .child(icon),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_0p5()
                                                        .overflow_hidden()
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .text_color(cx.theme().foreground)
                                                                .child(entry.message.clone()),
                                                        )
                                                        .when(entry.detail.is_some(), |this| {
                                                            this.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(
                                                                        entry
                                                                            .detail
                                                                            .clone()
                                                                            .unwrap_or_default(),
                                                                    ),
                                                            )
                                                        }),
                                                )
                                                .into_any_element()
                                        })
                                        .collect()
                                },
                            )
                            .size_full()
                            .track_scroll(&scroll_handle),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&scrollbar_state, &scroll_handle)),
                        )
                    }),
            )
    }

    /// The graph index: every node, in id order. A click selects the node and
    /// brings it into view.
    pub fn render_find_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (node_count, comment_count, nodes, selected_nodes) = {
            let canvas = self.canvas.read(cx);
            let graph = canvas.graph();
            let mut nodes: Vec<(String, String, (f64, f64))> = graph
                .nodes
                .values()
                .map(|n| {
                    (
                        n.id.clone(),
                        crate::features::compilation::lowering::node_title(n),
                        (n.position.x, n.position.y),
                    )
                })
                .collect();
            nodes.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            (
                graph.nodes.len(),
                graph.comments.len(),
                nodes,
                canvas.controller().selection.nodes.clone(),
            )
        };
        let item_sizes = std::rc::Rc::new(
            nodes
                .iter()
                .map(|_| size(px(0.0), px(34.0)))
                .collect::<Vec<_>>(),
        );
        let panel_entity = cx.entity().clone();
        let scroll_handle = self.find_output_scroll_handle.clone();
        let scrollbar_state = self.find_output_scrollbar_state.clone();

        v_flex()
            .size_full()
            .p_2()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Graph Index"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} nodes, {} comments", node_count, comment_count)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Click a node entry to select it in the graph."),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .relative()
                    .when(nodes.is_empty(), |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No nodes in graph."),
                        )
                    })
                    .when(!nodes.is_empty(), |this| {
                        this.child(
                            v_virtual_list(
                                panel_entity,
                                "find-panel-node-list",
                                item_sizes,
                                move |_panel, range, _window, cx| {
                                    range
                                        .map(|ix| -> AnyElement {
                                            let Some((node_id, node_title, position)) = nodes.get(ix)
                                            else {
                                                return div().h(px(34.0)).into_any_element();
                                            };
                                            let node_id = node_id.clone();
                                            let is_selected = selected_nodes.contains(&node_id);

                                            h_flex()
                                                .w_full()
                                                .h(px(34.0))
                                                .items_center()
                                                .justify_between()
                                                .px_2()
                                                .cursor_pointer()
                                                .bg(if is_selected {
                                                    cx.theme().accent.opacity(0.12)
                                                } else {
                                                    gpui::transparent_black()
                                                })
                                                .border_b_1()
                                                .border_color(cx.theme().border.opacity(0.08))
                                                .hover(|s| s.bg(cx.theme().muted.opacity(0.2)))
                                                .on_mouse_down(
                                                    gpui::MouseButton::Left,
                                                    cx.listener(move |panel, _, _window, cx| {
                                                        panel.focus_node(&node_id, cx);
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().foreground)
                                                        .child(node_title.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!(
                                                            "({:.0}, {:.0})",
                                                            position.0, position.1
                                                        )),
                                                )
                                                .into_any_element()
                                        })
                                        .collect()
                                },
                            )
                            .size_full()
                            .track_scroll(&scroll_handle),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&scrollbar_state, &scroll_handle)),
                        )
                    }),
            )
    }
}

impl Render for ShaderEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.load_error.as_deref() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(720.))
                        .gap_3()
                        .p_6()
                        .bg(cx.theme().sidebar)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(
                            div()
                                .text_lg()
                                .text_color(cx.theme().danger)
                                .child("Material could not be opened"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(error.to_string()),
                        ),
                )
                .into_any_element();
        }

        if self.workspace.is_none() {
            self.initialize_workspace(window, cx);
        }

        v_flex()
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .key_context("ShaderEditor")
            .on_action(cx.listener(|panel, _: &CompileShader, _window, cx| {
                if !panel.compilation_status.is_compiling {
                    panel.start_compilation(cx);
                }
            }))
            .on_action(cx.listener(|panel, _: &ReloadShaderFromDisk, window, cx| {
                panel.plugin_reload(window, cx);
            }))
            .on_action(cx.listener(|panel, _: &AddGraphComment, _window, cx| {
                panel.add_comment(cx);
            }))
            .child(ToolbarRenderer::render(self, cx))
            .child(div().flex_1().min_h_0().map(|el| {
                if let Some(workspace) = &self.workspace {
                    el.child(workspace.clone())
                } else {
                    el.child(div().child("Initializing workspace..."))
                }
            }))
            .when(self.file_loading, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().background.opacity(0.88))
                        .child(
                            v_flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().foreground)
                                        .child("Loading Material…"),
                                )
                                .child(
                                    div()
                                        .w(px(220.))
                                        .h(px(3.))
                                        .rounded_full()
                                        .bg(cx.theme().border)
                                        .child(
                                            div()
                                                .w(px(70.))
                                                .h_full()
                                                .rounded_full()
                                                .bg(cx.theme().primary),
                                        ),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}
