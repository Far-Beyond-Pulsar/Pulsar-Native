//! The editor's views: toolbar, the canvas area with its overlays (context menu,
//! node picker, comment title editor), the dock layout, and the Properties panel.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{DockChannel, DockItem, Panel, PanelEvent, PanelView};
use ui::input::{InputEvent, InputState, TextInput};
use ui::workspace::Workspace;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable as _, StyledExt as _};

use crate::buffers_panel::BuffersPanel;
use crate::editor::{CanvasMenu, RenderGraphEditor, Status};
use crate::model::{self, ParamKind};
use crate::preview::PipelinePreview;
use crate::sections_panel::SectionsPanel;

impl RenderGraphEditor {
    /// Builds the docking workspace once the document is installed.
    pub(crate) fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }
        let editor = cx.entity().downgrade();
        let preview = cx.new(PipelinePreview::new);
        self.preview = Some(preview.clone());

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel("render-graph-editor-workspace", DockChannel(1), window, cx)
        });
        workspace.update(cx, |workspace, cx| {
            let dock = workspace.dock_area().downgrade();
            let canvas = cx.new(|cx| CanvasPanel::new(editor.clone(), cx));
            let sections = cx.new(|cx| SectionsPanel::new(editor.clone(), cx));
            let buffers = cx.new(|cx| BuffersPanel::new(editor.clone(), window, cx));
            let properties = cx.new(|cx| PropertiesPanel::new(editor.clone(), cx));

            let center = DockItem::tabs(
                vec![Arc::new(canvas) as Arc<dyn PanelView>],
                Some(0),
                &dock,
                window,
                cx,
            );
            let left = DockItem::tabs(
                vec![
                    Arc::new(sections) as Arc<dyn PanelView>,
                    Arc::new(buffers) as Arc<dyn PanelView>,
                ],
                None,
                &dock,
                window,
                cx,
            );
            let right = DockItem::split(
                Axis::Vertical,
                vec![
                    DockItem::tabs(
                        vec![Arc::new(preview) as Arc<dyn PanelView>],
                        None,
                        &dock,
                        window,
                        cx,
                    ),
                    DockItem::tabs(
                        vec![Arc::new(properties) as Arc<dyn PanelView>],
                        None,
                        &dock,
                        window,
                        cx,
                    ),
                ],
                &dock,
                window,
                cx,
            );
            workspace.initialize(center, Some(left), Some(right), None, window, cx);
        });
        self.workspace = Some(workspace);
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.is_dirty();
        let status = self.status.clone();
        let active = self.active_tab();
        let mut menus = plugin_editor_api::EditorMenuStrip::new("render-graph-editor-menus").menu(
            plugin_editor_api::EditorMenu::new("Edit").undo_redo(
                "Undo",
                "Redo",
                active.map(|tab| tab.history.clone() as _),
            ),
        );
        if let Some(tab) = active {
            menus = menus.action_context(tab.canvas.read(cx).focus_handle(cx));
        }
        let (status_color, status_text) = match &status {
            Some(Status { is_error: true, text }) => (cx.theme().danger, text.clone()),
            Some(Status { text, .. }) => (cx.theme().success, text.clone()),
            None => (cx.theme().muted_foreground, String::new()),
        };
        h_flex()
            .w_full()
            .h(px(44.0))
            .px_2()
            .gap_1()
            .items_center()
            .bg(cx.theme().sidebar.opacity(0.98))
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.8))
            .child(menus)
            .child(separator(cx))
            .child(
                Button::new("toolbar-save")
                    .ghost()
                    .small()
                    .icon(IconName::FloppyDisk)
                    .tooltip(if dirty {
                        "Save Render Graph · Unsaved changes"
                    } else {
                        "Save Render Graph"
                    })
                    .on_click(cx.listener(|editor, _, _window, cx| editor.save(cx))),
            )
            .child(
                Button::new("toolbar-compile")
                    .small()
                    .icon(IconName::Flash)
                    .label("Compile")
                    .tooltip("Check the graph and run it in the preview")
                    .on_click(cx.listener(|editor, _, _window, cx| editor.validate(cx))),
            )
            .child(separator(cx))
            .child(
                Button::new("toolbar-add-comment")
                    .ghost()
                    .small()
                    .icon(IconName::Message)
                    .tooltip("Add Comment to Graph")
                    .on_click(cx.listener(|editor, _, _window, cx| editor.add_comment(cx))),
            )
            .child(
                Button::new("toolbar-auto-layout")
                    .ghost()
                    .small()
                    .icon(IconName::GitBranch)
                    .tooltip("Auto-layout the graph")
                    .on_click(cx.listener(|editor, _, _window, cx| editor.auto_layout(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(status_color)
                    .child(status_text),
            )
    }

    /// The canvas area: the graph tabs, the canvas of the active graph and the
    /// overlays over it.
    pub(crate) fn render_center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(canvas) = self.active_canvas().cloned() else {
            return div().into_any_element();
        };
        let mut strip = h_flex()
            .w_full()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.6));
        for &index in &self.open {
            let tab = &self.tabs[index];
            let selected = index == self.active;
            let name = if index == 0 {
                self.file
                    .file_stem()
                    .map_or_else(|| tab.name.clone(), |s| s.to_string_lossy().into_owned())
            } else {
                tab.name.clone()
            };
            strip = strip.child(
                h_flex()
                    .id(("graph-tab", index))
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |tab| tab.bg(cx.theme().accent.opacity(0.2)))
                    .hover(|tab| tab.bg(cx.theme().muted.opacity(0.4)))
                    .on_click(cx.listener(move |editor, _, _, cx| editor.activate(index, cx)))
                    .child(name)
                    .when(index != 0, |tab| {
                        tab.child(
                            div()
                                .id(("graph-tab-close", index))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("×")
                                .on_click(cx.listener(move |editor, _, _, cx| {
                                    cx.stop_propagation();
                                    editor.close_tab(index, cx);
                                })),
                        )
                    }),
            );
        }
        v_flex()
            .size_full()
            .when(self.open.len() > 1, |root| root.child(strip))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_action(cx.listener(|editor, _: &plugin_editor_api::Undo, _, cx| editor.undo(cx)))
                    .on_action(cx.listener(|editor, _: &plugin_editor_api::Redo, _, cx| editor.redo(cx)))
                    .child(div().size_full().child(canvas))
                    .child(self.render_palette(window, cx))
                    .child(self.render_context_menu(cx))
                    .child(self.render_comment_editor(cx)),
            )
            .into_any_element()
    }

    fn render_context_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.menu.clone() else {
            return div().into_any_element();
        };
        let this = cx.entity();
        let mut list = div()
            .occlude()
            .w(px(220.0))
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_lg()
            .rounded(px(6.0))
            .py(px(4.0));
        for item in menu.items.iter().cloned() {
            list = list.children(Self::menu_entry(item, &menu, &this, cx));
        }
        deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(8.0))
                .anchor(Corner::TopLeft)
                .child(list.on_mouse_down_out(move |_, _, cx| {
                    this.update(cx, |editor, cx| {
                        editor.menu = None;
                        cx.notify();
                    });
                })),
        )
        .with_priority(2)
        .into_any_element()
    }

    /// One menu entry (and its children, flattened below it).
    fn menu_entry(
        item: graphy_gpui::MenuItem,
        menu: &CanvasMenu,
        this: &Entity<Self>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        if item.separator_before {
            out.push(
                div()
                    .my(px(4.0))
                    .mx(px(8.0))
                    .h(px(1.0))
                    .bg(cx.theme().border)
                    .into_any_element(),
            );
        }
        let foreground = if item.enabled {
            cx.theme().popover_foreground
        } else {
            cx.theme().muted_foreground
        };
        let id = item.id.clone();
        let mut row = div()
            .px(px(12.0))
            .py(px(6.0))
            .text_sm()
            .text_color(foreground)
            .child(item.label.clone());
        if item.enabled {
            let (this, menu) = (this.clone(), menu.clone());
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().accent.opacity(0.12)))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let (menu, id) = (menu.clone(), id.clone());
                    this.update(cx, |editor, cx| editor.run_menu_item(menu, &id, window, cx));
                });
        }
        out.push(row.into_any_element());
        for child in item.children {
            out.extend(Self::menu_entry(child, menu, this, cx));
        }
        out
    }

    fn render_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(palette) = &self.palette else {
            return div().into_any_element();
        };
        let query = self.palette_input.read(cx).text().to_string().to_lowercase();
        let mut list = v_flex().id("render-graph-palette-list").overflow_y_scroll().flex_1();
        let matches = palette
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.title.to_lowercase().contains(&query)
                    || entry.category.to_lowercase().contains(&query)
                    || entry.keywords.iter().any(|k| k.to_lowercase().contains(&query))
            })
            .take(80);
        for (index, entry) in matches.enumerate() {
            let definition = entry.definition.clone();
            list = list.child(
                h_flex()
                    .id(("palette-entry", index))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|row| row.bg(cx.theme().accent.opacity(0.12)))
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        editor.place_picked(&definition, cx);
                    }))
                    .child(div().flex_1().text_sm().truncate().child(entry.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(entry.category.clone()),
                    ),
            );
        }
        let at = palette.at;
        if palette.focus_pending {
            let handle = self.palette_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        if let Some(palette) = &mut self.palette {
            palette.focus_pending = false;
        }
        let this = cx.entity();
        deferred(
            anchored()
                .position(at)
                .snap_to_window_with_margin(px(8.0))
                .anchor(Corner::TopLeft)
                .child(
                    v_flex()
                        .occlude()
                        .w(px(320.0))
                        .h(px(480.0))
                        .shadow_lg()
                        .rounded(px(6.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .child(div().p_1().child(TextInput::new(&self.palette_input).text_sm()))
                        .child(list)
                        .on_mouse_down_out(move |_, _, cx| {
                            this.update(cx, |editor, cx| editor.close_palette(cx));
                        }),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// Title editor over the comment being renamed.
    fn render_comment_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(index), Some(canvas)) = (self.editing_comment, self.active_canvas()) else {
            return div().into_any_element();
        };
        let canvas = canvas.read(cx);
        let Some(comment) = canvas.graph().comments.get(index) else {
            return div().into_any_element();
        };
        let (_, zoom) = canvas.view();
        let title_h = (30.0 * zoom).clamp(18.0, 44.0);
        let title_w = (comment.size.0 as f32 * zoom - 24.0 * zoom).max(80.0);
        let origin = canvas.graph_to_window(graphy_render::Vec2::new(
            comment.position.x as f32,
            comment.position.y as f32,
        ));
        let position = Point::new(origin.x + px(12.0 * zoom), origin.y + px(6.0 * zoom));
        deferred(
            anchored()
                .position(position)
                .snap_to_window_with_margin(px(4.0))
                .anchor(Corner::TopLeft)
                .child(
                    div().occlude().w(px(title_w)).h(px(title_h * 0.68)).child(
                        TextInput::new(&self.comment_input)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false),
                    ),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl Render for RenderGraphEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.load_error.as_deref() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(
                    v_flex()
                        .max_w(px(720.0))
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
                                .child("Render graph could not be opened"),
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
        let body = match &self.workspace {
            Some(workspace) => div().size_full().child(workspace.clone()),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Loading render graph…"),
        };
        v_flex()
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .key_context("RenderGraphEditor")
            .track_focus(&self.focus_handle)
            .child(self.toolbar(cx))
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

fn separator(cx: &mut Context<RenderGraphEditor>) -> impl IntoElement {
    div()
        .h_6()
        .w_px()
        .mx_1()
        .bg(cx.theme().border.opacity(0.5))
        .flex_shrink_0()
}

// ── Panels over the editor ───────────────────────────────────────────────────

/// The dock panel holding the canvas area.
pub struct CanvasPanel {
    editor: WeakEntity<RenderGraphEditor>,
    focus_handle: FocusHandle,
}

impl CanvasPanel {
    pub fn new(editor: WeakEntity<RenderGraphEditor>, cx: &mut Context<Self>) -> Self {
        if let Some(entity) = editor.upgrade() {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        }
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for CanvasPanel {}

impl Focusable for CanvasPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CanvasPanel {
    fn panel_name(&self) -> &'static str {
        "render-graph-canvas"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn title(&self, _window: &Window, cx: &App) -> AnyElement {
        let title = self
            .editor
            .upgrade()
            .and_then(|editor| editor.read(cx).file.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Render Graph".to_string());
        title.into_any_element()
    }
}

impl Render for CanvasPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.editor.upgrade() {
            Some(editor) => editor.update(cx, |editor, cx| editor.render_center(window, cx)),
            None => div().into_any_element(),
        }
    }
}

/// The Properties panel: the selected node's parameters.
pub struct PropertiesPanel {
    editor: WeakEntity<RenderGraphEditor>,
    /// Text inputs by graph, node and parameter key.
    inputs: HashMap<String, Entity<InputState>>,
    focus_handle: FocusHandle,
}

impl PropertiesPanel {
    pub fn new(editor: WeakEntity<RenderGraphEditor>, cx: &mut Context<Self>) -> Self {
        if let Some(entity) = editor.upgrade() {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        }
        Self {
            editor,
            inputs: HashMap::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// The text input editing `key` of `node`, created on first use and kept in
    /// step with the node's value while it is not being typed in.
    fn input(
        &mut self,
        graph: &str,
        node: &str,
        key: &str,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let slot = format!("{graph}\u{1f}{node}\u{1f}{key}");
        if let Some(existing) = self.inputs.get(&slot) {
            let existing = existing.clone();
            let focused = existing.read(cx).focus_handle(cx).is_focused(window);
            if !focused && existing.read(cx).text().to_string() != current {
                existing.update(cx, |input, cx| input.set_value(current.to_string(), window, cx));
            }
            return existing;
        }
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(current.to_string(), window, cx);
            input
        });
        let (node, key) = (node.to_string(), key.to_string());
        cx.subscribe_in(
            &input,
            window,
            move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change | InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let text = state.read(cx).text().to_string();
                    if let Some(editor) = this.editor.upgrade() {
                        editor.update(cx, |editor, cx| editor.set_node_property(&node, &key, text, cx));
                    }
                }
            },
        )
        .detach();
        self.inputs.insert(slot, input.clone());
        input
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanel {}

impl Focusable for PropertiesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PropertiesPanel {
    fn panel_name(&self) -> &'static str {
        "render-graph-properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Properties".into_any_element()
    }
}

impl Render for PropertiesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let empty = |text: &'static str| {
            div()
                .size_full()
                .p_3()
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let Some(editor) = self.editor.upgrade() else {
            return empty("No editor");
        };
        // The first selected node of the active graph.
        let selected = {
            let editor = editor.read(cx);
            let Some(tab) = editor.active_tab() else {
                return empty("No graph");
            };
            let canvas = tab.canvas.read(cx);
            canvas.controller().selection.nodes.iter().next().and_then(|id| {
                let node = canvas.graph().nodes.get(id)?;
                let title = canvas.scene().node(id).map(|n| n.presentation.title.clone());
                Some((tab.id.clone(), node.clone(), title, editor.profile.clone()?))
            })
        };
        let Some((graph_id, node, title, profile)) = selected else {
            return empty("Select a node to edit its parameters.");
        };

        let registry = profile.registry().clone();
        let descriptor = registry.get(&node.node_type).map(|d| &**d);
        let values = model::effective_text(descriptor, &node);
        let mut rows = Vec::new();
        for field in model::parameter_fields(&node.node_type, &registry) {
            let value = values.get(&field.key).cloned().unwrap_or_default();
            let control: AnyElement = match &field.kind {
                ParamKind::Text => {
                    let input = self.input(&graph_id, &node.id, &field.key, &value, window, cx);
                    TextInput::new(&input).xsmall().into_any_element()
                }
                ParamKind::Bool => {
                    let on = value.eq_ignore_ascii_case("true");
                    let (node_id, key) = (node.id.clone(), field.key.clone());
                    let editor = editor.clone();
                    Button::new(SharedString::from(format!("param-{}-{}", node.id, field.key)))
                        .label(if on { "On" } else { "Off" })
                        .xsmall()
                        .when(on, |button| button.primary())
                        .on_click(move |_, _, cx| {
                            editor.update(cx, |editor, cx| {
                                editor.set_node_property(&node_id, &key, (!on).to_string(), cx)
                            });
                        })
                        .into_any_element()
                }
                ParamKind::Choice(options) => {
                    let options = options.clone();
                    let (node_id, key, current) = (node.id.clone(), field.key.clone(), value.clone());
                    let editor = editor.clone();
                    Button::new(SharedString::from(format!("param-{}-{}", node.id, field.key)))
                        .label(value.clone())
                        .xsmall()
                        .on_click(move |_, _, cx| {
                            let next = options
                                .iter()
                                .position(|option| *option == current)
                                .map_or(0, |index| (index + 1) % options.len());
                            if let Some(option) = options.get(next) {
                                let option = option.clone();
                                editor.update(cx, |editor, cx| {
                                    editor.set_node_property(&node_id, &key, option, cx)
                                });
                            }
                        })
                        .into_any_element()
                }
            };
            rows.push(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(80.0))
                            .flex_shrink_0()
                            .text_xs()
                            .font_medium()
                            .text_color(muted)
                            .child(field.label.clone()),
                    )
                    .child(div().flex_1().child(control))
                    .into_any_element(),
            );
        }

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child(title.unwrap_or_else(|| node.node_type.clone())),
                    )
                    .child(div().text_xs().text_color(muted).child(node.node_type.clone())),
            )
            .when(rows.is_empty(), |panel| {
                panel.child(div().text_sm().text_color(muted).child("This node has no parameters."))
            })
            .child(v_flex().gap_3().children(rows))
            .into_any_element()
    }
}

