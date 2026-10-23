//! The Buffers panel: every buffer the render graph can hold, as a variable. A
//! buffer is only a name for visual cleanliness; each row adds a Get or Set node
//! for it, as many as the author wants, so a wire never has to cross the graph.

use blueprint_editor_plugin::BlueprintEditorPanel;
use gpui::*;
use helio_graph_asset::{buffers, core_nodes, PinType};
use ui::input::{InputEvent, InputState, TextInput};
use ui::dock::{Panel, PanelEvent};
use ui::{ActiveTheme as _, StyledExt as _};

use crate::model;

pub struct BuffersPanel {
    editor: WeakEntity<BlueprintEditorPanel>,
    search: Entity<InputState>,
    query: String,
    focus_handle: FocusHandle,
}

impl BuffersPanel {
    pub fn new(
        editor: WeakEntity<BlueprintEditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search buffers…"));
        cx.subscribe_in(&search, window, |this, input, _event: &InputEvent, _window, cx| {
            this.query = input.read(cx).text().to_string().to_lowercase();
            cx.notify();
        })
        .detach();
        Self { editor, search, query: String::new(), focus_handle: cx.focus_handle() }
    }

    fn add(&self, definition: &'static str, entry: String, cx: &mut Context<Self>) {
        let _ = self.editor.update(cx, |editor, cx| {
            editor.add_profile_node(definition, vec![("buffer".to_string(), entry)], cx);
        });
    }

    /// The heading a kind of buffer is listed under.
    fn heading(ty: PinType) -> &'static str {
        match ty.prefix() {
            Some("tex") => "Textures",
            Some("ptex") => "Persistent textures",
            Some("depth") => "Depth",
            Some("buf") => "Buffers",
            Some("pbuf") => "Persistent buffers",
            _ => "Other",
        }
    }
}

impl EventEmitter<PanelEvent> for BuffersPanel {}

impl Focusable for BuffersPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BuffersPanel {
    fn panel_name(&self) -> &'static str {
        "render-graph-buffers"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Buffers".into_any_element()
    }
}

impl Render for BuffersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let known = buffers::known_buffers(model::registry());
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().muted.opacity(0.4);
        let accent = cx.theme().accent;

        let mut list = div().id("buffers-list").flex().flex_col().gap_0p5().overflow_y_scroll().flex_1();
        let mut heading: Option<&'static str> = None;
        for (index, (ty, name)) in known.iter().enumerate() {
            if !self.query.is_empty() && !name.to_lowercase().contains(&self.query) {
                continue;
            }
            let Some(entry) = buffers::entry(*ty, name) else { continue };
            let group = Self::heading(*ty);
            if heading != Some(group) {
                heading = Some(group);
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(group),
                );
            }
            let get_entry = entry.clone();
            let set_entry = entry;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .hover(|style| style.bg(hover))
                    .child(div().flex_1().text_sm().truncate().child(name.clone()))
                    .child(
                        pill(("get", index), "Get", accent).on_click(cx.listener(move |this, _, _, cx| {
                            this.add(core_nodes::BUFFER_GET, get_entry.clone(), cx);
                        })),
                    )
                    .child(
                        pill(("set", index), "Set", accent).on_click(cx.listener(move |this, _, _, cx| {
                            this.add(core_nodes::BUFFER_SET, set_entry.clone(), cx);
                        })),
                    ),
            );
        }
        if known.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(muted)
                    .child("Buffers appear once the pass pins are known."),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .child(TextInput::new(&self.search).text_sm())
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(muted)
                    .child("Buffers are variables. Add as many Get and Set nodes as keep the graph tidy."),
            )
            .child(list)
    }
}

/// A small button that adds a node.
fn pill(id: (&'static str, usize), label: &'static str, accent: Hsla) -> Stateful<Div> {
    div()
        .id(ElementId::from(id))
        .px_2()
        .py_0p5()
        .rounded_sm()
        .text_xs()
        .cursor_pointer()
        .bg(accent.opacity(0.25))
        .hover(|style| style.bg(accent.opacity(0.5)))
        .child(label)
}
