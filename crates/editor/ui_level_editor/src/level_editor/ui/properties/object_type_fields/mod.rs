//! Inspector section for a selected scene object.
//!
//! Each concern lives in its own sub-module:
//!
//! | Module               | Responsibility                                              |
//! |----------------------|-------------------------------------------------------------|
//! | [`icon_picker`]      | Object-level icon-asset picker (stored as a plain prop).   |
//! | [`property_renderer`]| Per-component property cards from the reflection registry. |
//! | [`category_section`] | Collapsible category group headers and row layout.         |
//!
//! The legacy "Object Type" card that hard-coded `ObjectType` enum variants
//! has been removed.  Component behaviour now drives all object logic.

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyMetadata, REGISTRY, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::SubscriptionId;
use serde_json::Value;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::dropdown::{SearchableList, SearchableListEvent};
use ui::{v_flex, ActiveTheme};
use ui_common::{MeshAssetPicker, PropertyStateManager};

use crate::level_editor::scene_database::SceneDatabase;
use crate::level_editor::state::LevelEditorState;

mod category_section;
mod icon_picker;
mod property_renderer;

/// Cached property metadata + default instance for a single component class.
/// Populated lazily on first encounter and reused across frames to avoid the
/// per-frame `create_instance()` + `get_properties()` allocation.
pub(super) struct PropertyMetadataCacheEntry {
    pub properties: Arc<Vec<PropertyMetadata>>,
    /// Throwaway default instance, kept alive so the getter closures
    /// (which reference data inside this instance) remain valid.
    pub _default_instance: Box<dyn pulsar_reflection::EngineClass>,
}

pub struct ObjectTypeFieldsSection {
    pub(super) object_id: String,
    pub(super) scene_db: SceneDatabase,
    /// Currently selected component index (reserved for future highlight use).
    pub(super) selected_component: Option<usize>,
    /// Searchable component list for the add-component popover.
    pub(super) component_list: Entity<SearchableList<String>>,
    /// Shared level-editor state (expand/collapse, selection).
    pub(super) state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    /// Shared property widget state (numeric inputs, colour pickers, asset pickers).
    pub(super) property_state: PropertyStateManager,
    /// Asset picker for the object-level icon prop.
    pub(super) icon_asset_picker: Option<Entity<MeshAssetPicker>>,
    /// Categories the user has explicitly collapsed this session.
    pub(super) collapsed_property_categories: HashSet<(String, String)>,
    /// Categories the user has explicitly expanded, overriding the default-collapsed flag.
    pub(super) expanded_property_categories: HashSet<(String, String)>,

    // ── Performance caches ─────────────────────────────────────────────────
    /// Per-class property metadata cache.  Populated lazily on first encounter
    /// and reused across frames — avoids the per-frame `create_instance()`
    /// + `get_properties()` allocation that was the single biggest cost in the
    /// property rendering path.
    pub(super) property_metadata_cache: HashMap<String, Arc<PropertyMetadataCacheEntry>>,
    /// Number of components from the last render — used to detect structural
    /// changes without calling `get_components()` (which clones JSON).
    pub(super) cached_component_count: usize,

    // ── Live-value subscription caches (Pulsar-Native#575, SceneDB#47) ────
    //
    // Before subscriptions existed, every render pass re-pulled every
    // property of every mounted card straight from `World`, unconditionally,
    // because nothing could say whether the underlying data had moved. Now:
    // each mounted card arms ONE World subscription (per `(entity, class)`
    // pair), keeps its latest pulled values here, and re-pulls only when a
    // signal fires -- a subscription event for its own card, a legacy JSON-
    // path write recorded in the property change set, or a structural /
    // store-swap invalidation.
    /// Latest known live values per mounted component card, keyed by
    /// `(class_name, component_index)` -- the index is what keeps N
    /// instances of the same class distinct (Pulsar-Native#519). Borrowed
    /// during row building, never consumed -- rows only read.
    pub(super) world_value_cache: HashMap<(String, usize), Vec<Box<dyn Any>>>,
    /// Cards whose cached values are stale and must be re-pulled on the
    /// next render pass.
    pub(super) dirty_classes: HashSet<(String, usize)>,
    /// Armed World subscriptions per mounted card
    /// (`(class_name, component_index) -> SubscriptionId`). Only the
    /// live-typed instance of a class gets one. Events arrive tagged with
    /// this id; this map is also what `Drop` uses to disarm everything when
    /// the section is torn down (selection change).
    pub(super) world_subs: HashMap<(String, usize), SubscriptionId>,
    /// Classes with no `World`-registered component id at all (the legacy
    /// JSON-only classes). Permanently un-subscribable until the card set
    /// structurally changes; remembered so the registry lookup isn't paid
    /// every render for a card that can never have a live value.
    pub(super) unsubscribable_classes: HashSet<String>,
    /// Store-generation (`SceneDatabase::subscriptions_epoch`) these
    /// subscriptions were armed in. Undo/redo swaps the whole `World`,
    /// killing every outstanding subscription without any event ever firing;
    /// an epoch mismatch means drop everything and re-arm from scratch.
    pub(super) subs_epoch: u64,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut items: Vec<String> = REGISTRY
            .get_class_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        if let Some(pm) = plugin_manager::global() {
            let pm = pm.read();
            let plugin_defs = pm.get_all_component_definitions();
            for def in &plugin_defs {
                if !items.contains(&def.id) {
                    items.push(def.id.clone());
                }
            }
        }
        items.sort();

        let component_list = cx.new(|cx| {
            SearchableList::new(window, cx, items, |name| name.clone())
                .with_empty_text("No components found")
                .with_max_width(px(240.0))
                .with_max_height(px(320.0))
                .with_icon_getter(|_| ui::IconName::Component)
        });

        let scene_db_for_add = scene_db.clone();
        let object_id_for_add = object_id.clone();
        cx.subscribe(
            &component_list,
            move |_this, _, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(class_name) = event {
                    Self::add_component(
                        &scene_db_for_add,
                        &object_id_for_add,
                        class_name,
                        cx,
                    );
                    cx.notify();
                }
            },
        )
        .detach();

        Self {
            object_id,
            scene_db,
            selected_component: None,
            component_list,
            state_arc,
            property_state: PropertyStateManager::new(),
            icon_asset_picker: None,
            collapsed_property_categories: HashSet::new(),
            expanded_property_categories: HashSet::new(),
            property_metadata_cache: HashMap::new(),
            cached_component_count: 0,
            world_value_cache: HashMap::new(),
            dirty_classes: HashSet::new(),
            world_subs: HashMap::new(),
            unsubscribable_classes: HashSet::new(),
            subs_epoch: 0, // corrected against the live epoch on first render
        }
    }

    /// Disarm every World subscription this section armed. Called from
    /// `Drop` (section teardown on selection change) and from the
    /// structural/store-swap invalidation paths.
    fn release_world_subscriptions(&mut self) {
        for (_, sub) in self.world_subs.drain() {
            self.scene_db.unsubscribe_component(sub);
        }
    }

    /// Invalidate ALL per-card subscription state -- next render re-arms and
    /// re-pulls everything. For undo/redo's wholesale `World` swap (whose
    /// outstanding subscriptions die silently, no events) and structural
    /// card-set changes.
    fn reset_world_subscription_state(&mut self) {
        self.release_world_subscriptions();
        self.world_value_cache.clear();
        self.dirty_classes.clear();
        self.unsubscribable_classes.clear();
        self.subs_epoch = self.scene_db.subscriptions_epoch();
    }

    fn add_component(
        scene_db: &SceneDatabase,
        object_id: &String,
        class_name: &str,
        _cx: &mut Context<Self>,
    ) {
        let class_name = class_name.to_string();
        if REGISTRY.has_class(&class_name) {
            if let Some(mut instance) = REGISTRY.create_instance(&class_name) {
                let props = instance.get_properties();
                let mut map = serde_json::Map::new();
                for prop in &props {
                    let v = (prop.getter)(instance.as_ref());
                    let json_value = RUNTIME_TYPE_REGISTRY
                        .serialize_json_for_any(v.as_ref())
                        .unwrap_or(serde_json::json!(null));
                    map.insert(prop.name.to_string(), json_value);
                }
                scene_db.add_component(object_id, class_name, Value::Object(map));
            }
        } else if let Some(instance) = engine_backend::EngineBackend::global().and_then(|b| {
            let guard = b.read();
            guard.plugin_components().create_instance(&class_name)
        }) {
            let props = instance.get_properties();
            let mut map = serde_json::Map::new();
            for prop in &props {
                let v = (prop.getter)(instance.as_ref());
                let json_value = RUNTIME_TYPE_REGISTRY
                    .serialize_json_for_any(v.as_ref())
                    .unwrap_or(serde_json::json!(null));
                map.insert(prop.name.to_string(), json_value);
            }
            scene_db.add_component(object_id, class_name, Value::Object(map));
        }
    }

    /// Returns a diagnostic banner element when no components are attached or
    /// none of the attached components can be found in the reflection registry.
    fn render_diag_card(
        &self,
        attached: &[ComponentInstance],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if attached.is_empty() {
            Some(self.diag_card_element("⚠ No components attached", cx))
        } else if attached
            .iter()
            .all(|c| !REGISTRY.has_class(c.class_name.as_str()))
        {
            Some(self.diag_card_element("⚠ Components not found in registry", cx))
        } else {
            None
        }
    }

    fn diag_card_element(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_1()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(message.to_string()),
            )
            .into_any_element()
    }
}

impl Drop for ObjectTypeFieldsSection {
    fn drop(&mut self) {
        // Selection changed / panel closed: stop watching this object's
        // components so a long-lived editor doesn't accumulate dead
        // subscriptions for every object that was ever inspected.
        self.release_world_subscriptions();
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::ComponentHierarchyPanel;
        use ui::popover::Popover;
        use ui::{IconName, Sizable as _};

        // ── Drain change set (once per frame) ──────────────────────────────
        let property_changes = self.scene_db.drain_property_changes();
        let structural = property_changes.components_added_or_removed();

        // ── Detect structural changes without full get_components() ────────
        let current_count = self.scene_db.component_count(
            &self.object_id,
        );
        let count_changed = current_count != self.cached_component_count;
        self.cached_component_count = current_count;

        // Clear cached values for structurally-changed objects so stale
        // entries from removed/renamed components don't persist.
        if structural || count_changed {
            self.property_metadata_cache.clear();
            // The card set itself changed: every subscription/cache entry is
            // suspect (a removed card's sub must be disarmed; a re-added one
            // needs a fresh arm against the possibly-new entity).
            self.reset_world_subscription_state();
        }

        // Undo/redo swapped the whole `World` out from under us: every
        // outstanding subscription died without an event ever firing. The
        // epoch check is the only signal -- see
        // `SceneDatabase::subscriptions_epoch`.
        if self.subs_epoch != self.scene_db.subscriptions_epoch() {
            self.reset_world_subscription_state();
        }

        // ── Object icon picker row ─────────────────────────────────────────
        let icon_row = self.render_icon_row(window, cx);

        // ── Component hierarchy panel (tree + add-component button) ────────
        // The hierarchy panel needs the full ComponentInstance list for its
        // tree view (class names + enabled status).  This is the one place
        // where get_components() is still required.
        let list = self.component_list.clone();
        let add_popover = Popover::<SearchableList<String>>::new("add-component-picker")
            .anchor(Corner::TopRight)
            .trigger(
                ui::button::Button::new("add-component-btn")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost(),
            )
            .content(move |_window, _cx| list.clone())
            .into_any_element();

        // Metadata-only read: class names/order/enabled/parent indices for
        // the tree and diagnostics. Live values are NOT needed here — the
        // property cards below batch-read straight from World — so paying
        // `get_components`' per-component `to_json()` serialization on every
        // render would only make this panel's complexity set the framerate.
        let attached = self.scene_db.get_components_metadata(&self.object_id);

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone(), self.scene_db.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&attached, &state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();
        drop(state);

        // ── Diagnostic banner (no components / registry mismatch) ──────────
        let diag_card = self.render_diag_card(&attached, cx);

        // ── Per-component property cards ───────────────────────────────────
        //
        // Mark cards dirty from the two signals that can have fired since
        // last render, BEFORE building them:
        //
        // 1. World subscription events (SceneDB#47) -- the push signal for
        //    every World-registered card: real inserts/mutations/removals of
        //    exactly the `(entity, component)` pairs these cards display,
        //    whether the write came from this panel, an AI tool, or a
        //    renderer-side sync. Events are matched back to their card by
        //    SubscriptionId.
        // 2. The legacy JSON change set -- covers the handful of
        //    not-World-registered classes whose only write path still goes
        //    through `metadata_db` (they can never fire a World event).
        //
        // SINGLE-DRAINER CONTRACT: `take_world_component_events` empties a
        // queue shared by every subscriber in the process; this section is
        // the one consumer today. See
        // `SceneDatabase::take_world_component_events`'s doc before adding a
        // second drainer.
        //
        // Guarded on having armed at least one subscription: draining takes
        // the store's WRITE lock, and an all-legacy-cards panel (nothing
        // subscribable) must not pay that on every render pass. SceneDB's
        // own cap bounds the undrained queue meanwhile.
        if !self.world_subs.is_empty() {
            for event in self.scene_db.take_world_component_events() {
                for (card, sub) in &self.world_subs {
                    if *sub == event.subscription {
                        self.dirty_classes.insert(card.clone());
                    }
                }
            }
        }
        if !property_changes.is_empty() {
            // Legacy JSON-path writes are recorded per (object, class,
            // property) without an instance index -- mark every card of a
            // touched class dirty; each re-pulls from its OWN source, so
            // over-invalidation costs a read, never a wrong value.
            for (idx, comp) in attached.iter().enumerate() {
                if property_changes.class_changed(&self.object_id, &comp.class_name) {
                    self.dirty_classes.insert((comp.class_name.clone(), idx));
                }
            }
        }

        let component_sections =
            self.render_component_sections(&attached, window, cx);

        v_flex()
            .w_full()
            .gap_3()
            .child(icon_row)
            .child(component_panel)
            .children(diag_card)
            .children(component_sections)
            .into_any_element()
    }
}
