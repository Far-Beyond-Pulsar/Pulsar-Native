//! Add Object Picker
//!
//! Compact, searchable popover that lists built-in object types and any engine
//! classes registered via `#[derive(EngineClass)]`.  Opens as a `Popover` when
//! the `+` button in the hierarchy header is clicked.

use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

use crate::level_editor::scene_database::{
    LightType, MeshType, ObjectType, SceneDb, SceneObjectData, Transform,
};
use crate::level_editor::ui::state::LevelEditorState;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ObjectSpawnedEvent;

// ── Built-in types ────────────────────────────────────────────────────────────

struct BuiltinEntry {
    label: &'static str,
    icon: IconName,
    object_type: fn() -> ObjectType,
}

static BUILTIN_TYPES: &[BuiltinEntry] = &[
    BuiltinEntry {
        label: "Empty",
        icon: IconName::Circle,
        object_type: || ObjectType::Empty,
    },
    BuiltinEntry {
        label: "Camera",
        icon: IconName::Camera,
        object_type: || ObjectType::Camera,
    },
    BuiltinEntry {
        label: "Directional Light",
        icon: IconName::Sun,
        object_type: || ObjectType::Light(LightType::Directional),
    },
    BuiltinEntry {
        label: "Point Light",
        icon: IconName::LightBulb,
        object_type: || ObjectType::Light(LightType::Point),
    },
    BuiltinEntry {
        label: "Spot Light",
        icon: IconName::Flash,
        object_type: || ObjectType::Light(LightType::Spot),
    },
    BuiltinEntry {
        label: "Area Light",
        icon: IconName::SunLight,
        object_type: || ObjectType::Light(LightType::Area),
    },
    BuiltinEntry {
        label: "Cube",
        icon: IconName::Cube,
        object_type: || ObjectType::Mesh(MeshType::Cube),
    },
    BuiltinEntry {
        label: "Sphere",
        icon: IconName::Sphere,
        object_type: || ObjectType::Mesh(MeshType::Sphere),
    },
    BuiltinEntry {
        label: "Cylinder",
        icon: IconName::Cylinder,
        object_type: || ObjectType::Mesh(MeshType::Cylinder),
    },
    BuiltinEntry {
        label: "Plane",
        icon: IconName::Square,
        object_type: || ObjectType::Mesh(MeshType::Plane),
    },
    BuiltinEntry {
        label: "Particle System",
        icon: IconName::Sparks,
        object_type: || ObjectType::ParticleSystem,
    },
    BuiltinEntry {
        label: "Audio Source",
        icon: IconName::MusicNote,
        object_type: || ObjectType::AudioSource,
    },
];

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddObjectDialog {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    /// Engine class names from `pulsar_reflection::REGISTRY`.
    engine_classes: Vec<&'static str>,
}

impl EventEmitter<DismissEvent> for AddObjectDialog {}
impl EventEmitter<ObjectSpawnedEvent> for AddObjectDialog {}

impl Focusable for AddObjectDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddObjectDialog {
    pub fn new(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        let engine_classes = pulsar_reflection::REGISTRY.get_class_names();

        let focus_handle = cx.focus_handle();

        Self {
            focus_handle,
            search_input,
            state_arc,
            engine_classes,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_lowercase()
    }

    fn spawn_object(&self, name: &str, object_type: ObjectType, cx: &mut Context<Self>) {
        let objects_count = self.state_arc.read().scene_objects().len();
        let new_object = SceneObjectData {
            id: format!("object_{}", objects_count + 1),
            name: name.to_string(),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            components: vec![],
            scene_path: String::new(),
        };
        self.state_arc
            .read()
            .scene_database
            .add_object(new_object, None);
        cx.emit(ObjectSpawnedEvent);
        cx.emit(DismissEvent);
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AddObjectDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);

        // Filter built-ins
        let builtins: Vec<usize> = BUILTIN_TYPES
            .iter()
            .enumerate()
            .filter(|(_, e)| query.is_empty() || e.label.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();

        // Filter engine classes
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
            .max_h(px(380.0))
            .p_1()
            .gap_1()
            .track_focus(&self.focus_handle)
            // ── Search ──────────────────────────────────────────────────────
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(TextInput::new(&self.search_input).w_full().xsmall()),
            )
            // ── Scrollable content ──────────────────────────────────────────
            .child(
                div().flex_1().w_full().overflow_hidden().child(
                    div().size_full().scrollable(ScrollbarAxis::Vertical).child(
                        v_flex()
                            .w_full()
                            // ── Built-in section ────────────────────────────────────────────
                            .when(!builtins.is_empty(), |el| {
                                el.child(
                                    div()
                                        .px_2()
                                        .pt_1()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Built-in"),
                                )
                                .children(
                                    builtins.into_iter().map(|i| {
                                        let entry = &BUILTIN_TYPES[i];
                                        let label = entry.label;
                                        let icon = entry.icon.clone();
                                        let make_type = entry.object_type;
                                        let theme = cx.theme().clone();
                                        row_style(div())
                                            .id(ElementId::Name(label.into()))
                                            .hover(move |s| s.bg(theme.accent.opacity(0.12)))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.spawn_object(label, make_type(), cx);
                                                }),
                                            )
                                            .child(
                                                Icon::new(icon)
                                                    .size(px(13.0))
                                                    .text_color(cx.theme().muted_foreground),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().foreground)
                                                    .child(label),
                                            )
                                    }),
                                )
                            })
                            // ── Engine classes section ───────────────────────────────────────
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
                                            cx.listener(move |this, _, _, cx| {
                                                this.spawn_object(name, ObjectType::Empty, cx);
                                            }),
                                        )
                                        .child(
                                            Icon::new(IconName::Code)
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
                            // ── Empty state ──────────────────────────────────────────────────
                            .when(
                                builtins_empty_and_classes_empty(&query, &self.engine_classes),
                                |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .justify_center()
                                            .py_4()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("No results"),
                                    )
                                },
                            ),
                    ),
                ),
            )
    }
}

fn builtins_empty_and_classes_empty(query: &str, engine_classes: &[&str]) -> bool {
    if query.is_empty() {
        return false;
    }
    let q = query.to_lowercase();
    let no_builtin = BUILTIN_TYPES
        .iter()
        .all(|e| !e.label.to_lowercase().contains(&q));
    let no_class = engine_classes
        .iter()
        .all(|n| !n.to_lowercase().contains(&q));
    no_builtin && no_class
}
