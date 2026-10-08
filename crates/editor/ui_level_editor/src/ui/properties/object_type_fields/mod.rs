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
use engine_backend::scene::SharedScene;
use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyMetadata, REGISTRY};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::dropdown::{SearchableList, SearchableListEvent};
use ui::{v_flex, ActiveTheme};
use ui_common::{MeshAssetPicker, PropertyStateManager};

use crate::state::LevelEditorState;

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
    pub(super) scene_db: SharedScene,
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
    /// The instance entity behind each mounted live card, so a value the
    /// object's subscription delivers (`apply_update`) lands on its card.
    pub(super) card_entities: HashMap<pulsar_scenedb::Entity, (String, usize)>,
    /// Classes with no `World`-registered component id at all (the legacy
    /// JSON-only classes). Permanently un-subscribable until the card set
    /// structurally changes; remembered so the registry lookup isn't paid
    /// every render for a card that can never have a live value.
    pub(super) unsubscribable_classes: HashSet<String>,
    /// Scene rebuild generation (`SceneDomain::rebuild_epoch`) the cards
    /// were bound in. Undo/redo rebuilds the whole `World`, so the cards
    /// must be bound to the rebuilt world's instance entities; a mismatch
    /// rebinds.
    pub(super) subs_epoch: u64,

    // ── Class instances (#921) ──────────────────────────────────────────────
    /// The project's classes, scanned once per section (selection).
    pub(super) class_registry: Option<pulsar_class::ClassRegistry>,
    /// Class defaults of this object's components that were built from a
    /// class slot, by component index; drives the override markers and
    /// "revert to class default" on those cards.
    pub(super) slot_defaults: HashMap<usize, crate::scene_edit::classes::SlotDefault>,
    /// Per card, each property's class default (aligned with the card's
    /// cached property metadata), read once from `slot_defaults`.
    pub(super) slot_default_values: HashMap<usize, Arc<Vec<Option<Box<dyn Any>>>>>,
    /// The class variables card data, when this object is a class root.
    pub(super) class_view: Option<crate::scene_edit::classes::ClassInstanceView>,
    /// Re-read `slot_defaults` / `class_view` on the next render.
    pub(super) class_cache_dirty: bool,
    /// `classes::class_defs_generation()` the class caches were read at.
    pub(super) class_defs_generation: u64,
    /// World revision the class variables card was read at (undo/redo of a
    /// variable edit changes it without a property-change record).
    pub(super) class_view_revision: u64,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SharedScene,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // `ClassInstance` only comes from placing a class, never from here.
        let mut items: Vec<String> = REGISTRY
            .get_class_names()
            .into_iter()
            .filter(|name| *name != pulsar_class::CLASS_INSTANCE)
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

        let object_id_for_add = object_id.clone();
        cx.subscribe(
            &component_list,
            move |this, _, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(class_name) = event {
                    Self::add_component(&this.state_arc, &object_id_for_add, class_name);
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
            property_state: PropertyStateManager::new().with_cached_rows(),
            icon_asset_picker: None,
            collapsed_property_categories: HashSet::new(),
            expanded_property_categories: HashSet::new(),
            property_metadata_cache: HashMap::new(),
            cached_component_count: 0,
            world_value_cache: HashMap::new(),
            dirty_classes: HashSet::new(),
            card_entities: HashMap::new(),
            unsubscribable_classes: HashSet::new(),
            subs_epoch: 0, // corrected against the live epoch on first render
            class_registry: None,
            slot_defaults: HashMap::new(),
            slot_default_values: HashMap::new(),
            class_view: None,
            class_cache_dirty: true,
            class_defs_generation: 0,
            class_view_revision: 0,
        }
    }

    /// Invalidate ALL per-card state -- next render rebinds and re-pulls
    /// everything. For undo/redo's wholesale `World` rebuild and structural
    /// card-set changes.
    fn reset_world_subscription_state(&mut self) {
        self.card_entities.clear();
        self.world_value_cache.clear();
        self.dirty_classes.clear();
        self.unsubscribable_classes.clear();
        self.subs_epoch = self.state_arc.read().scene.rebuild_epoch;
        self.class_cache_dirty = true;
    }

    /// Re-read the class caches (slot defaults, class variables card) when
    /// something that feeds them changed.
    fn refresh_class_caches(&mut self) {
        let generation = crate::scene_edit::classes::class_defs_generation();
        let revision = self.state_arc.read().scene.world_revision();
        if self.class_view.is_some() && revision != self.class_view_revision {
            self.class_view_revision = revision;
            let registry = self
                .class_registry
                .get_or_insert_with(crate::scene_edit::classes::project_registry);
            let world = self.scene_db.read();
            self.class_view = crate::scene_edit::classes::class_instance_view(
                &world.world,
                &self.object_id,
                registry,
            );
        }
        if !self.class_cache_dirty && generation == self.class_defs_generation {
            return;
        }
        self.class_view_revision = revision;
        self.class_cache_dirty = false;
        self.class_defs_generation = generation;
        self.slot_default_values.clear();
        let registry = self
            .class_registry
            .get_or_insert_with(crate::scene_edit::classes::project_registry);
        let world = self.scene_db.read();
        self.slot_defaults =
            crate::scene_edit::classes::slot_defaults(&world.world, &self.object_id, registry);
        self.class_view = crate::scene_edit::classes::class_instance_view(
            &world.world,
            &self.object_id,
            registry,
        );
    }

    /// Add Component: the class default, attached through the command
    /// layer so it is one undo step. A class this build cannot attach to a
    /// scene object (a plugin-only class) is refused there.
    fn add_component(
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        object_id: &str,
        class_name: &str,
    ) {
        let result = crate::commands::execute_command(
            &mut state_arc.write(),
            crate::commands::SceneCommand::AddComponent {
                id: object_id.to_string(),
                class_name: class_name.to_string(),
                value: None,
            },
        );
        if !result.changed {
            tracing::warn!(
                "Could not add {class_name} to '{object_id}': {}",
                result.no_op_reason
            );
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

impl ObjectTypeFieldsSection {
    /// Apply one change the object's subscription delivered. A live card's
    /// new value replaces its cached values directly; a change to the card
    /// set (an instance added, removed, reordered or toggled) or to an
    /// unresolved payload rebinds; a class-variable change re-reads the
    /// class caches.
    pub fn apply_update(&mut self, delta: &pulsar_world_registry::ObjectDelta, cx: &mut Context<Self>) {
        use engine_backend::scene::attachments::{
            ComponentAttachments, ComponentMeta, ComponentOwner, UnresolvedComponent,
        };
        use pulsar_scenedb::{component_id, ComponentChangeKind};
        if delta.component == component_id::<pulsar_class::ClassInstance>() {
            self.class_cache_dirty = true;
            cx.notify();
            return;
        }
        let structural = [
            component_id::<ComponentMeta>(),
            component_id::<ComponentOwner>(),
            component_id::<ComponentAttachments>(),
            component_id::<UnresolvedComponent>(),
        ]
        .contains(&delta.component);
        match self.card_entities.get(&delta.entity).cloned() {
            Some(card) if !structural && delta.kind != ComponentChangeKind::Removed => {
                let Some(value) = delta.value.as_deref() else {
                    self.dirty_classes.insert(card);
                    cx.notify();
                    return;
                };
                let Some(entry) = self.property_metadata_cache.get(&card.0).cloned() else {
                    return;
                };
                let Some(class) = pulsar_world_registry::value_engine_class(&card.0, value) else {
                    return;
                };
                let values = entry.properties.iter().map(|prop| (prop.getter)(class)).collect();
                self.world_value_cache.insert(card.clone(), values);
                self.dirty_classes.remove(&card);
                cx.notify();
            }
            Some(_) => {
                self.property_metadata_cache.clear();
                self.reset_world_subscription_state();
                cx.notify();
            }
            None if structural => {
                self.property_metadata_cache.clear();
                self.reset_world_subscription_state();
                cx.notify();
            }
            None => {}
        }
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::ComponentHierarchyPanel;
        use ui::popover::Popover;
        use ui::{IconName, Sizable as _};

        // ── Detect structural changes without full get_components() ────────
        let current_count = {
            let world = self.scene_db.read();
            crate::scene_edit::components::component_count(&world.world, &self.object_id)
        };
        let count_changed = current_count != self.cached_component_count;
        self.cached_component_count = current_count;

        // Clear cached values for structurally-changed objects so stale
        // entries from removed/renamed components don't persist.
        if count_changed {
            self.property_metadata_cache.clear();
            // The card set itself changed: every binding/cache entry is
            // suspect (a removed card's binding must go; a re-added one must
            // be bound to its possibly-new entity).
            self.reset_world_subscription_state();
        }

        // Undo/redo rebuilt the whole `World`: rebind every card to the
        // rebuilt world's instance entities -- see
        // `SceneDomain::rebuild_epoch`.
        if self.subs_epoch != self.state_arc.read().scene.rebuild_epoch {
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
        let attached = {
            let world = self.scene_db.read();
            crate::scene_edit::components::get_components_metadata(&world.world, &self.object_id)
        };

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&attached, &state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();
        drop(state);

        // ── Diagnostic banner (no components / registry mismatch) ──────────
        let diag_card = self.render_diag_card(&attached, cx);

        // ── Per-component property cards ───────────────────────────────────
        //
        // Cards are kept current by the object's subscription: the panel
        // forwards each delivered change to `apply_update`, so nothing is
        // re-read here.
        self.refresh_class_caches();

        let component_sections = self.render_component_sections(&attached, window, cx);

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
