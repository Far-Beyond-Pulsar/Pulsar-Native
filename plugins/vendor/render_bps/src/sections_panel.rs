//! The Sections panel: every collapsed graph (section) of the render graph, to
//! open without hunting for its node on the canvas.

use blueprint_editor_plugin::BlueprintEditorPanel;
use blueprint_graph::SubGraphKind;
use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::{ActiveTheme as _, StyledExt as _};

pub struct SectionsPanel {
    editor: WeakEntity<BlueprintEditorPanel>,
    focus_handle: FocusHandle,
}

impl SectionsPanel {
    pub fn new(editor: WeakEntity<BlueprintEditorPanel>, cx: &mut Context<Self>) -> Self {
        // The list follows the editor: sections are added, renamed and removed there.
        if let Some(entity) = editor.upgrade() {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        }
        Self { editor, focus_handle: cx.focus_handle() }
    }

    /// `(id, name, nodes inside)` of each section, in the order the editor holds them.
    fn sections(&self, cx: &App) -> Vec<(String, String, usize)> {
        self.editor
            .upgrade()
            .map(|editor| {
                editor
                    .read(cx)
                    .subgraphs
                    .iter()
                    .filter(|subgraph| subgraph.kind == SubGraphKind::Collapsed)
                    .map(|subgraph| {
                        // Entry and exit are not passes.
                        let nodes = subgraph
                            .graph
                            .nodes
                            .values()
                            .filter(|node| !matches!(node.node_type.as_str(), "macro_entry" | "macro_exit"))
                            .count();
                        (subgraph.id.clone(), subgraph.name.clone(), nodes)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl EventEmitter<PanelEvent> for SectionsPanel {}

impl Focusable for SectionsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SectionsPanel {
    fn panel_name(&self) -> &'static str {
        "render-graph-sections"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Sections".into_any_element()
    }
}

impl Render for SectionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = self.sections(cx);
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().muted.opacity(0.4);

        let mut list = div().id("sections-list").flex().flex_col().gap_0p5().overflow_y_scroll().flex_1();
        for (index, (id, name, nodes)) in sections.iter().enumerate() {
            let id = id.clone();
            list = list.child(
                div()
                    .id(("section", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // A click lands while the dock is being updated, and opening a tab
                        // updates the dock: do it once this is done, as the Macros panel does.
                        let id = id.clone();
                        let editor = this.editor.clone();
                        let window_handle = window.window_handle();
                        cx.defer(move |cx| {
                            let _ = cx.update_window(window_handle, |_, window, cx| {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.open_macro_tab(&id, window, cx)
                                });
                            });
                        });
                    }))
                    .child(div().flex_1().text_sm().truncate().child(name.clone()))
                    .child(div().text_xs().text_color(muted).child(format!("{nodes} nodes"))),
            );
        }
        if sections.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(muted)
                    .child("No sections. Select nodes and collapse them to make one."),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .font_semibold()
                    .text_color(muted)
                    .child("Click a section to open it."),
            )
            .child(list)
    }
}
