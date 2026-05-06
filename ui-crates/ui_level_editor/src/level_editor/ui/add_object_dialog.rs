//! Add Object Dialog
//!
//! Virtualized, searchable list of all engine classes registered via
//! `#[derive(EngineClass)]`.  Opens as a modal when the `+` button is clicked
//! in the hierarchy header.

use std::{rc::Rc, sync::Arc};

use gpui::{prelude::*, *};
use ui::{
    h_flex,
    input::{InputState, TextInput},
    v_flex, v_virtual_list, ActiveTheme, VirtualListScrollHandle,
};

use crate::level_editor::scene_database::{ObjectType, SceneObjectData, SceneDb, Transform};
use crate::level_editor::ui::state::LevelEditorState;

pub struct AddObjectDialog {
    search_input: Entity<InputState>,
    scroll_handle: VirtualListScrollHandle,
    /// All class names from the global registry (sorted, stable).
    all_classes: Vec<&'static str>,
    /// Categories per class (same index as `all_classes`).
    all_categories: Vec<Option<&'static str>>,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
}

impl AddObjectDialog {
    pub fn new(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search engine classes…")
        });

        // React to search input changes and notify so the list re-renders.
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        let registry = &*pulsar_reflection::REGISTRY;
        let all_classes: Vec<&'static str> = registry.get_class_names();
        let all_categories: Vec<Option<&'static str>> = all_classes
            .iter()
            .map(|name| {
                // Re-derive categories by scanning registry entries.
                // `get_class_names_by_category` isn't suitable here;
                // instead iterate and match by name.
                registry
                    .get_categories()
                    .into_iter()
                    .find(|cat| registry.get_class_names_by_category(cat).contains(name))
            })
            .collect();

        Self {
            search_input,
            scroll_handle: VirtualListScrollHandle::new(),
            all_classes,
            all_categories,
            state_arc,
        }
    }

    /// Returns the class names visible given the current search query.
    fn filtered_classes(&self, cx: &App) -> Vec<(&'static str, Option<&'static str>)> {
        let query = self.search_input.read(cx).value().to_lowercase();
        self.all_classes
            .iter()
            .zip(self.all_categories.iter())
            .filter(|(name, _)| query.is_empty() || name.to_lowercase().contains(&query))
            .map(|(name, cat)| (*name, *cat))
            .collect()
    }

    fn spawn_class(&self, class_name: &str, window: &mut Window, cx: &mut App) {
        use ui::ContextModal as _;

        let objects_count = self.state_arc.read().scene_objects().len();
        let id = format!("object_{}", objects_count + 1);

        let new_object = SceneObjectData {
            id: id.clone(),
            name: class_name.to_string(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            components: vec![],
            scene_path: String::new(),
        };

        self.state_arc.read().scene_database.add_object(new_object, None);

        // Close the modal after spawning.
        window.close_modal(cx);
    }
}

impl Render for AddObjectDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let visible: Vec<(&'static str, Option<&'static str>)> = self.filtered_classes(cx);
        let row_count = visible.len();

        let row_h = px(32.0);
        let item_sizes = Rc::new(vec![size(px(0.0), row_h); row_count]);

        let view = cx.entity().clone();
        let visible_rc = Rc::new(visible);

        v_flex()
            .w_full()
            .gap_2()
            // ── Search bar ─────────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .child(TextInput::new(&self.search_input).w_full()),
            )
            // ── Row count badge ────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(if row_count == self.all_classes.len() {
                                format!("{} classes", row_count)
                            } else {
                                format!("{} / {} classes", row_count, self.all_classes.len())
                            }),
                    ),
            )
            // ── Virtualized class list ─────────────────────────────────────
            .child(
                div()
                    .w_full()
                    .h(px(360.0))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child({
                        let visible_for_list = visible_rc.clone();
                        v_virtual_list(view.clone(), "add-object-list", item_sizes, {
                            let visible_for_list = visible_for_list.clone();
                            move |this, range, _window, cx| {
                                let theme = cx.theme().clone();
                                range
                                    .map(|i| {
                                        let (name, cat) = visible_for_list[i];
                                        let view_clone = cx.entity().clone();
                                        let name_s = name;

                                        h_flex()
                                            .w_full()
                                            .h(px(32.0))
                                            .px_3()
                                            .gap_2()
                                            .items_center()
                                            .cursor_pointer()
                                            .hover(|s| {
                                                s.bg(theme.accent.opacity(0.15))
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _ev, window, cx| {
                                                    this.spawn_class(name_s, window, cx);
                                                }),
                                            )
                                            // Class name
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme.foreground)
                                                    .child(name),
                                            )
                                            // Optional category badge
                                            .when_some(cat, |el, category| {
                                                el.child(
                                                    div()
                                                        .px_2()
                                                        .py(px(2.0))
                                                        .rounded_full()
                                                        .bg(theme.accent.opacity(0.2))
                                                        .text_xs()
                                                        .text_color(theme.accent)
                                                        .child(category),
                                                )
                                            })
                                    })
                                    .collect()
                            }
                        })
                        .track_scroll(&self.scroll_handle)
                    }),
            )
    }
}
