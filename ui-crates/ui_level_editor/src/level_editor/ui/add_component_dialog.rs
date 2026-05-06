//! Add Component Picker
//!
//! Compact searchable popover listing all engine classes registered via
//! `#[derive(EngineClass)]`.  Emits `ComponentAddedEvent` with the chosen
//! class name when the user clicks an entry.

use gpui::{prelude::*, *};
use ui::{input::{InputState, TextInput}, v_flex, ActiveTheme, Icon, IconName, Sizable};

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ComponentAddedEvent {
    pub class_name: String,
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddComponentDialog {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    /// All registered engine class names, captured at construction time.
    engine_classes: Vec<&'static str>,
}

impl EventEmitter<DismissEvent> for AddComponentDialog {}
impl EventEmitter<ComponentAddedEvent> for AddComponentDialog {}

impl Focusable for AddComponentDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddComponentDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search components…"));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        let mut engine_classes = pulsar_reflection::REGISTRY.get_class_names();
        engine_classes.sort();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            engine_classes,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_lowercase()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AddComponentDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);

        let classes: Vec<&'static str> = self
            .engine_classes
            .iter()
            .copied()
            .filter(|n| query.is_empty() || n.to_lowercase().contains(&query))
            .collect();

        let row_style = |el: Div| {
            el.flex()
                .flex_row()
                .w_full()
                .h(px(28.0))
                .px_2()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .rounded(px(4.0))
        };

        v_flex()
            .w(px(240.0))
            .max_h(px(320.0))
            .p_1()
            .gap_1()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(TextInput::new(&self.search_input).w_full().xsmall()),
            )
            .when(classes.is_empty(), |el| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No components found"),
                )
            })
            .when(!classes.is_empty(), |el| {
                el.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child("Engine Classes"),
                )
                .children(classes.into_iter().map(|name| {
                    let theme = cx.theme().clone();
                    row_style(div())
                        .id(ElementId::Name(name.into()))
                        .hover(move |s| s.bg(theme.accent.opacity(0.12)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ComponentAddedEvent {
                                    class_name: name.to_string(),
                                });
                                cx.emit(DismissEvent);
                            }),
                        )
                        .child(
                            Icon::new(IconName::Component)
                                .size(px(13.0))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(name),
                        )
                }))
            })
    }
}
