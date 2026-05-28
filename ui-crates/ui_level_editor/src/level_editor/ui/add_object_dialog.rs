//! Add Object Picker
//!
//! Compact popover used by the hierarchy `+` button.
//!
//! New objects are always spawned as empty objects. Behavior is authored via
//! attached components rather than object type presets.

use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    dropdown::{SearchableList, SearchableListEvent},
    IconName,
};

use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
use crate::level_editor::ui::state::LevelEditorState;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ObjectSpawnedEvent;

#[derive(Clone)]
struct ObjectMenuItem {
    label: &'static str,
    icon: IconName,
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AddObjectDialog {
    searchable_list: Entity<SearchableList<ObjectMenuItem>>,
    _subscriptions: Vec<Subscription>,
    state_arc: crate::level_editor::StateEntity,
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
        state_arc: crate::level_editor::StateEntity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let items = vec![ObjectMenuItem {
            label: "Empty Object",
            icon: IconName::Circle,
        }];

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
                if let SearchableListEvent::Select(item) = event {
                    let _ = item;
                    this.spawn_object(cx);
                }
            },
        )];

        Self {
            searchable_list,
            _subscriptions: subscriptions,
            state_arc,
        }
    }

    fn spawn_object(&self, cx: &mut Context<Self>) {
        let objects_count = self.state_arc.read(cx).scene_objects().len();
        let new_object = SceneObjectData {
            id: format!("object_{}", objects_count + 1),
            name: "New Object".to_string(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
        };
        self.state_arc
            .read(cx)
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
