//! Add Object Picker
//!
//! Compact, searchable popover that lists built-in object types and any engine
//! classes registered via `#[derive(EngineClass)]`.  Opens as a `Popover` when
//! the `+` button in the hierarchy header is clicked.

use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    dropdown::{SearchableList, SearchableListEvent},
    IconName,
};

use crate::level_editor::scene_database::{
    LightType, MeshType, ObjectType, SceneDb, SceneObjectData, Transform,
};
use crate::level_editor::ui::state::LevelEditorState;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ObjectSpawnedEvent;

#[derive(Clone)]
struct ObjectMenuItem {
    label: &'static str,
    icon: IconName,
    kind: ObjectMenuKind,
}

#[derive(Clone, Copy)]
enum ObjectMenuKind {
    Builtin(fn() -> ObjectType),
    EngineClass,
}

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
    searchable_list: Entity<SearchableList<ObjectMenuItem>>,
    _subscriptions: Vec<Subscription>,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
}

impl EventEmitter<DismissEvent> for AddObjectDialog {}
impl EventEmitter<ObjectSpawnedEvent> for AddObjectDialog {}

impl Focusable for AddObjectDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.searchable_list.read(cx).focus_handle(cx)
    }
}

impl AddObjectDialog {
    pub fn new(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut items: Vec<ObjectMenuItem> = BUILTIN_TYPES
            .iter()
            .map(|entry| ObjectMenuItem {
                label: entry.label,
                icon: entry.icon.clone(),
                kind: ObjectMenuKind::Builtin(entry.object_type),
            })
            .collect();

        let mut engine_classes = pulsar_reflection::REGISTRY.get_class_names();
        engine_classes.sort();
        items.extend(engine_classes.into_iter().map(|name| ObjectMenuItem {
            label: name,
            icon: IconName::Code,
            kind: ObjectMenuKind::EngineClass,
        }));

        let searchable_list = cx.new(|cx| {
            SearchableList::new(window, cx, items, |item| item.label.to_string())
                .with_empty_text("No results")
                .with_max_width(px(240.0))
                .with_max_height(px(380.0))
                .with_icon_getter(|item| item.icon.clone())
        });

        let subscriptions = vec![cx.subscribe(
            &searchable_list,
            |this, _, event: &SearchableListEvent<ObjectMenuItem>, cx| {
                let SearchableListEvent::Select(item) = event;
                match item.kind {
                    ObjectMenuKind::Builtin(make_type) => {
                        this.spawn_object(item.label, make_type(), cx);
                    }
                    ObjectMenuKind::EngineClass => {
                        this.spawn_object(item.label, ObjectType::Empty, cx);
                    }
                }
            },
        )];

        Self {
            searchable_list,
            _subscriptions: subscriptions,
            state_arc,
        }
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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.searchable_list.clone()
    }
}
