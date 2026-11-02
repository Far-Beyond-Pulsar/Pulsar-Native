//! Property editor for [`VoxelTerrainStack`]: the whole terrain layer stack
//! as one value, so a preset replaces all of it and undo is one step.
//!
//! The stack's layers and material rules are ordered lists (move up and
//! down, remove, add); every field of a layer, a rule, the caves, the
//! cliffs and the materials is edited by the editor registered for its
//! type, through reflection (the shared pieces in `reflected_list`).
//!
//! Built for stacks with many layers: the editor's own row is the preset
//! picker, and everything else (section headings, layer and rule cards, their
//! fields) is offered to the panel as [`VirtualRows`], so the panel's single
//! list draws only what is in view and no scroll area nests inside it.
//! Sections and cards start collapsed, a card's field editors are created only
//! when it is first opened, and the stack is validated when it changes, not
//! every frame.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, Entity, SharedString, Subscription, WeakEntity, Window, prelude::*, px,
};
use pulsar_reflection::{
    BoundPropertyEditor, EngineClass, PropertyEditorArgs, PropertyWriteBack, VirtualRows,
};
use ui::button::{Button, ButtonVariants as _};
use ui::dropdown::{Dropdown, DropdownEvent, DropdownState};
use ui::{ActiveTheme as _, IconName, Sizable as _, v_flex};

use super::reflected_list::{
    FieldRows, ListEdit, ListWake, apply_list_edit, collapsible_item_card, remap_expanded,
};
use super::{VoxelLayerKind, VoxelMaterialRule, VoxelTerrainLayer, VoxelTerrainStack};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum List {
    Layers,
    Rules,
}

/// A collapsible section of the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Layers,
    Materials,
    Rules,
    Caves,
    Cliffs,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Layers => "Layers (in order)",
            Section::Materials => "Materials",
            Section::Rules => "Material rules (Rules style, in order)",
            Section::Caves => "Caves",
            Section::Cliffs => "Cliffs",
        }
    }
}

/// A set of field rows: one part of the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Materials,
    Caves,
    Cliffs,
    Layer(usize),
    Rule(usize),
}

/// One row offered to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// Why the generator rejects the stack.
    Error,
    Heading(Section),
    /// The card header of a layer or rule.
    Header(List, usize),
    /// Field editor `k` of a slot.
    Field(Slot, usize),
    /// Row `ix` offered by field editor `k` of a slot (a nested list).
    Nested(Slot, usize, usize),
}

pub struct VoxelTerrainStackEditor {
    label: String,
    id: String,
    current: Arc<Mutex<VoxelTerrainStack>>,
    write_back: PropertyWriteBack,
    presets: Entity<DropdownState<Vec<SharedString>>>,
    /// Field rows, built when their section or card is first opened.
    materials: Option<FieldRows>,
    caves: Option<FieldRows>,
    cliffs: Option<FieldRows>,
    layers: Vec<Option<FieldRows>>,
    rules: Vec<Option<FieldRows>>,
    open_sections: HashSet<Section>,
    open_layers: HashSet<usize>,
    open_rules: HashSet<usize>,
    /// Why the generator would reject the stack; recomputed when it changes.
    error: Option<String>,
    /// The rows offered to the panel, in order.
    layout: Vec<Entry>,
    wake: ListWake,
    /// Set by the host when it re-read the value; with the address check it
    /// keeps `set_value` from comparing the whole stack every frame.
    stale: Arc<AtomicBool>,
    last_ptr: usize,
    this: WeakEntity<Self>,
    _subs: Vec<Subscription>,
}

impl VoxelTerrainStackEditor {
    fn new(
        args: &PropertyEditorArgs<'_>,
        wake: ListWake,
        stale: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let value = args
            .current_value
            .downcast_ref::<VoxelTerrainStack>()
            .cloned()
            .unwrap_or_default();
        let id = format!(
            "stack-{}-{}-{}",
            args.id_prefix, args.class_name, args.prop_name
        );
        let names: Vec<SharedString> = VoxelTerrainStack::PRESETS
            .iter()
            .map(|n| SharedString::from(*n))
            .collect();
        let presets = cx.new(|cx| DropdownState::new(names, None, window, cx));
        let subs = vec![cx.subscribe_in(
            &presets,
            window,
            |this: &mut Self, _, event: &DropdownEvent<Vec<SharedString>>, window, cx| {
                let DropdownEvent::Confirm(Some(name)) = event else {
                    return;
                };
                if let Some(stack) = VoxelTerrainStack::preset(name) {
                    this.write(stack, window, cx);
                }
                this.presets
                    .update(cx, |state, cx| state.set_selected_index(None, window, cx));
            },
        )];
        let mut editor = Self {
            label: args.display_name.to_string(),
            id,
            error: validation_error(&value),
            layers: (0..value.layers.len()).map(|_| None).collect(),
            rules: (0..value.rules.len()).map(|_| None).collect(),
            current: Arc::new(Mutex::new(value)),
            write_back: args.write_back.clone(),
            presets,
            materials: None,
            caves: None,
            cliffs: None,
            open_sections: [Section::Layers, Section::Rules].into_iter().collect(),
            open_layers: HashSet::new(),
            open_rules: HashSet::new(),
            layout: Vec::new(),
            wake,
            stale,
            last_ptr: args.current_value as *const dyn std::any::Any as *const () as usize,
            this: cx.weak_entity(),
            _subs: subs,
        };
        editor.relayout(cx);
        editor
    }

    /// Field rows of the part of the stack `get` reads and `set` writes.
    /// `set` returns true when it changed more than the edited field (a
    /// layer's new kind brings that kind's defaults): every row then shows
    /// the new values.
    #[allow(clippy::too_many_arguments)]
    fn rows<T: EngineClass + Clone>(
        id: &str,
        part: &str,
        this: &WeakEntity<Self>,
        current: &Arc<Mutex<VoxelTerrainStack>>,
        write_back: &PropertyWriteBack,
        get: impl Fn(&VoxelTerrainStack) -> Option<T> + Send + Sync + 'static,
        set: impl Fn(&mut VoxelTerrainStack, T) -> bool + Send + Sync + 'static,
        value: &T,
        window: &mut Window,
        cx: &mut App,
    ) -> FieldRows {
        let (read, write) = (current.clone(), current.clone());
        let write_back = write_back.clone();
        let this_for_write = this.clone();
        let rows = FieldRows::new(
            value,
            &format!("{id}-{part}"),
            "VoxelTerrainStack",
            |_, _| true,
            move || read.lock().ok().and_then(|stack| get(&stack)),
            move |v, window, cx| {
                let (stack, reshaped) = {
                    let Ok(mut stack) = write.lock() else { return };
                    let reshaped = set(&mut stack, v);
                    (stack.clone(), reshaped)
                };
                write_back(Box::new(stack), window, cx);
                if reshaped {
                    // After the editor that wrote returns: it is mid-event.
                    let this = this_for_write.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.refresh(window, cx);
                            this.changed(cx);
                        });
                    });
                }
            },
            window,
            cx,
        );
        // A nested list changing its own rows reshapes ours.
        let this = this.clone();
        let nested_changed: Arc<dyn Fn(&mut App) + Send + Sync> = Arc::new(move |cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                this.relayout(cx);
                this.changed(cx);
            });
        });
        for (_, editor) in &rows.editors {
            if let Some(nested) = &editor.rows {
                nested.on_change(nested_changed.clone());
            }
        }
        rows
    }

    /// The field rows of `slot`, if built.
    fn slot_rows(&self, slot: Slot) -> Option<&FieldRows> {
        match slot {
            Slot::Materials => self.materials.as_ref(),
            Slot::Caves => self.caves.as_ref(),
            Slot::Cliffs => self.cliffs.as_ref(),
            Slot::Layer(i) => self.layers.get(i)?.as_ref(),
            Slot::Rule(i) => self.rules.get(i)?.as_ref(),
        }
    }

    /// Build the field rows of `slot` if they are missing.
    fn ensure_rows(&mut self, slot: Slot, window: &mut Window, cx: &mut App) {
        if self.slot_rows(slot).is_some() {
            return;
        }
        let stack = self.current.lock().map(|s| s.clone()).unwrap_or_default();
        let (id, this, current, write_back) = (
            self.id.clone(),
            self.this.clone(),
            self.current.clone(),
            self.write_back.clone(),
        );
        let rows = match slot {
            Slot::Materials => Self::rows(
                &id,
                "materials",
                &this,
                &current,
                &write_back,
                |s| Some(s.clone()),
                |s, v| replace(s, v),
                &stack,
                window,
                cx,
            ),
            Slot::Caves => Self::rows(
                &id,
                "caves",
                &this,
                &current,
                &write_back,
                |s| Some(s.volumes.caves.clone()),
                |s, v| replace(&mut s.volumes.caves, v),
                &stack.volumes.caves,
                window,
                cx,
            ),
            Slot::Cliffs => Self::rows(
                &id,
                "cliffs",
                &this,
                &current,
                &write_back,
                |s| Some(s.volumes.cliffs.clone()),
                |s, v| replace(&mut s.volumes.cliffs, v),
                &stack.volumes.cliffs,
                window,
                cx,
            ),
            Slot::Layer(i) => {
                let Some(layer) = stack.layers.get(i) else { return };
                Self::rows(
                    &id,
                    &format!("layer{i}"),
                    &this,
                    &current,
                    &write_back,
                    move |s| s.layers.get(i).cloned(),
                    move |s, v| s.layers.get_mut(i).is_some_and(|layer| set_layer(layer, v)),
                    layer,
                    window,
                    cx,
                )
            }
            Slot::Rule(i) => {
                let Some(rule) = stack.rules.get(i) else { return };
                Self::rows(
                    &id,
                    &format!("rule{i}"),
                    &this,
                    &current,
                    &write_back,
                    move |s| s.rules.get(i).cloned(),
                    move |s, v| s.rules.get_mut(i).is_some_and(|rule| replace(rule, v)),
                    rule,
                    window,
                    cx,
                )
            }
        };
        match slot {
            Slot::Materials => self.materials = Some(rows),
            Slot::Caves => self.caves = Some(rows),
            Slot::Cliffs => self.cliffs = Some(rows),
            Slot::Layer(i) => self.layers[i] = Some(rows),
            Slot::Rule(i) => self.rules[i] = Some(rows),
        }
    }

    /// Build the rows of everything that is open and not built yet.
    fn ensure_open_rows(&mut self, window: &mut Window, cx: &mut App) {
        let mut slots = Vec::new();
        for (section, slot) in [
            (Section::Materials, Slot::Materials),
            (Section::Caves, Slot::Caves),
            (Section::Cliffs, Slot::Cliffs),
        ] {
            if self.open_sections.contains(&section) {
                slots.push(slot);
            }
        }
        if self.open_sections.contains(&Section::Layers) {
            slots.extend(self.open_layers.iter().map(|&i| Slot::Layer(i)));
        }
        if self.open_sections.contains(&Section::Rules) {
            slots.extend(self.open_rules.iter().map(|&i| Slot::Rule(i)));
        }
        for slot in slots {
            self.ensure_rows(slot, window, cx);
        }
    }

    /// Recompute the rows offered to the panel.
    fn relayout(&mut self, cx: &App) {
        let (n_layers, n_rules) = self
            .current
            .lock()
            .map(|s| (s.layers.len(), s.rules.len()))
            .unwrap_or((0, 0));
        let mut layout = Vec::new();
        let fields = |layout: &mut Vec<Entry>, slot: Slot| {
            let Some(rows) = self.slot_rows(slot) else {
                return;
            };
            for (k, (_, editor)) in rows.editors.iter().enumerate() {
                layout.push(Entry::Field(slot, k));
                if let Some(nested) = &editor.rows {
                    layout.extend((0..nested.row_count(cx)).map(|ix| Entry::Nested(slot, k, ix)));
                }
            }
        };
        if self.error.is_some() {
            layout.push(Entry::Error);
        }
        for section in [
            Section::Layers,
            Section::Materials,
            Section::Rules,
            Section::Caves,
            Section::Cliffs,
        ] {
            layout.push(Entry::Heading(section));
            if !self.open_sections.contains(&section) {
                continue;
            }
            match section {
                Section::Layers => {
                    for i in 0..n_layers {
                        layout.push(Entry::Header(List::Layers, i));
                        if self.open_layers.contains(&i) {
                            fields(&mut layout, Slot::Layer(i));
                        }
                    }
                }
                Section::Rules => {
                    for i in 0..n_rules {
                        layout.push(Entry::Header(List::Rules, i));
                        if self.open_rules.contains(&i) {
                            fields(&mut layout, Slot::Rule(i));
                        }
                    }
                }
                Section::Materials => fields(&mut layout, Slot::Materials),
                Section::Caves => fields(&mut layout, Slot::Caves),
                Section::Cliffs => fields(&mut layout, Slot::Cliffs),
            }
        }
        self.layout = layout;
    }

    /// Repaint, and wake the panel so it re-reads the row count.  Deferred:
    /// a change can arrive while the panel is mid-render.
    fn changed(&mut self, cx: &mut gpui::Context<Self>) {
        cx.notify();
        let wake = self.wake.lock().ok().and_then(|slot| slot.clone());
        if let Some(wake) = wake {
            cx.defer(move |cx| wake(cx));
        }
    }

    /// Match the per-item slots to the stack's lists after it changed, keep
    /// what is built when the lengths agree, and re-read the built rows.
    fn refresh(&mut self, window: &mut Window, cx: &mut App) {
        let (n_layers, n_rules, stack_error) = self
            .current
            .lock()
            .map(|s| (s.layers.len(), s.rules.len(), validation_error(&s)))
            .unwrap_or((0, 0, None));
        self.error = stack_error;
        if self.layers.len() != n_layers {
            self.layers = (0..n_layers).map(|_| None).collect();
        }
        if self.rules.len() != n_rules {
            self.rules = (0..n_rules).map(|_| None).collect();
        }
        self.ensure_open_rows(window, cx);
        for rows in [&self.materials, &self.caves, &self.cliffs]
            .into_iter()
            .chain(&self.layers)
            .chain(&self.rules)
            .flatten()
        {
            rows.refresh(window, cx);
        }
        self.relayout(cx);
    }

    /// Replace the whole stack (a preset, a list edit) and write it back.
    fn write(
        &mut self,
        stack: VoxelTerrainStack,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Ok(mut current) = self.current.lock() {
            *current = stack.clone();
        }
        self.refresh(window, cx);
        (self.write_back)(Box::new(stack), window, cx);
        self.changed(cx);
    }

    fn edit_list(
        &mut self,
        list: List,
        edit: ListEdit,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut stack = self.current.lock().map(|s| s.clone()).unwrap_or_default();
        let (before, open) = match list {
            List::Layers => (stack.layers.len(), &mut self.open_layers),
            List::Rules => (stack.rules.len(), &mut self.open_rules),
        };
        *open = remap_expanded(open, edit, before);
        match list {
            List::Layers => apply_list_edit(&mut stack.layers, edit, || {
                VoxelTerrainLayer::new(VoxelLayerKind::Hills)
            }),
            List::Rules => apply_list_edit(&mut stack.rules, edit, VoxelMaterialRule::default),
        }
        let after = match list {
            List::Layers => stack.layers.len(),
            List::Rules => stack.rules.len(),
        };
        if edit == ListEdit::Add && after > before {
            match list {
                List::Layers => self.open_layers.insert(after - 1),
                List::Rules => self.open_rules.insert(after - 1),
            };
        }
        // Rows follow indices: every item's rows start again.
        self.layers = (0..stack.layers.len()).map(|_| None).collect();
        self.rules = (0..stack.rules.len()).map(|_| None).collect();
        self.write(stack, window, cx);
    }

    fn toggle_item(
        &mut self,
        list: List,
        index: usize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let open = match list {
            List::Layers => &mut self.open_layers,
            List::Rules => &mut self.open_rules,
        };
        if !open.remove(&index) {
            open.insert(index);
        }
        self.ensure_open_rows(window, cx);
        self.relayout(cx);
        self.changed(cx);
    }

    fn toggle_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.open_sections.remove(&section) {
            self.open_sections.insert(section);
        }
        self.ensure_open_rows(window, cx);
        self.relayout(cx);
        self.changed(cx);
    }

    /// Accept a value that changed elsewhere (undo, a blueprint, a preset).
    fn set_value(
        &mut self,
        value: &VoxelTerrainStack,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let ptr = value as *const VoxelTerrainStack as usize;
        let stale = self.stale.swap(false, Ordering::AcqRel);
        if ptr == self.last_ptr && !stale {
            return;
        }
        self.last_ptr = ptr;
        let changed = self
            .current
            .lock()
            .map(|current| *current != *value)
            .unwrap_or(true);
        if !changed {
            return;
        }
        if let Ok(mut current) = self.current.lock() {
            *current = value.clone();
        }
        self.refresh(window, cx);
        self.changed(cx);
    }

    /// Row `ix` of the layout.
    fn render_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.layout.get(ix).copied() else {
            return gpui::div().into_any_element();
        };
        let indented = |el: AnyElement, by: f32| {
            gpui::div()
                .w_full()
                .pl(px(by))
                .child(el)
                .into_any_element()
        };
        let field_view = |view: gpui::AnyView| {
            indented(
                view.cached_auto_height(gpui::StyleRefinement::default().w_full().flex_shrink_0())
                    .into_any_element(),
                12.0,
            )
        };
        match entry {
            Entry::Error => gpui::div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(format!(
                    "Not generated: {}",
                    self.error.as_deref().unwrap_or_default()
                ))
                .into_any_element(),
            Entry::Heading(section) => {
                let open = self.open_sections.contains(&section);
                // Lists add their items from the heading row, top right.
                let list = match section {
                    Section::Layers => Some((List::Layers, "Add layer")),
                    Section::Rules => Some((List::Rules, "Add rule")),
                    _ => None,
                };
                let add = list.map(|(list, tooltip)| {
                    Button::new(SharedString::from(format!("{}-add-{section:?}", self.id)))
                        .icon(IconName::Plus)
                        .xsmall()
                        .ghost()
                        .tooltip(tooltip)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_list(list, ListEdit::Add, window, cx)
                        }))
                });
                gpui::div()
                    .id(SharedString::from(format!("{}-{section:?}", self.id)))
                    .pt_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_section(section, window, cx)
                    }))
                    .child(
                        ui::Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(gpui::div().flex_1().child(section.title()))
                    .children(add)
                    .into_any_element()
            }
            Entry::Header(list, i) => {
                // Read in place: cloning the stack to name one card would copy
                // every layer for each header drawn.
                let (title, open) = {
                    let stack = self.current.lock();
                    match (&stack, list) {
                        (Ok(stack), List::Layers) => (
                            stack.layers.get(i).map(|l| layer_title(i, l)),
                            self.open_layers.contains(&i),
                        ),
                        (Ok(stack), List::Rules) => (
                            stack.rules.get(i).map(|r| rule_title(i, r)),
                            self.open_rules.contains(&i),
                        ),
                        _ => (None, false),
                    }
                };
                let Some(title) = title else {
                    return gpui::div().into_any_element();
                };
                let tag = if list == List::Layers { "layer" } else { "rule" };
                collapsible_item_card(
                    &format!("{}-{tag}", self.id),
                    i,
                    title,
                    open,
                    std::iter::empty(),
                    move |this: &mut Self, edit, window, cx| this.edit_list(list, edit, window, cx),
                    move |this: &mut Self, index, window, cx| {
                        this.toggle_item(list, index, window, cx)
                    },
                    cx,
                )
            }
            Entry::Field(slot, k) => match self.slot_rows(slot).and_then(|r| r.editors.get(k)) {
                Some((_, editor)) => field_view(editor.view.clone()),
                None => gpui::div().into_any_element(),
            },
            Entry::Nested(slot, k, n) => {
                let nested = self
                    .slot_rows(slot)
                    .and_then(|r| r.editors.get(k))
                    .and_then(|(_, editor)| editor.rows.clone());
                match nested {
                    Some(nested) => indented(nested.render_row(n, window, cx), 12.0),
                    None => gpui::div().into_any_element(),
                }
            }
        }
    }
}

/// Why the generator would reject the stack (its own validation), if it
/// would: the planet does not build until it is fixed.
pub fn validation_error(stack: &VoxelTerrainStack) -> Option<String> {
    let json = serde_json::to_value(stack).ok()?;
    match serde_json::from_value::<helio_pass_voxel_planet::layers::TerrainLayers>(json) {
        Ok(layers) => layers.validate().err(),
        Err(error) => Some(error.to_string()),
    }
}

/// Store an edited value; nothing else changes.
fn replace<T>(slot: &mut T, value: T) -> bool {
    *slot = value;
    false
}

/// Store an edited layer. A new kind starts from that kind's defaults (the
/// old numbers mean something else, or nothing, for it), keeping whether
/// the layer is on and where it applies; returns true then.
pub fn set_layer(layer: &mut VoxelTerrainLayer, edited: VoxelTerrainLayer) -> bool {
    if edited.kind == layer.kind {
        *layer = edited;
        return false;
    }
    *layer = VoxelTerrainLayer {
        enabled: edited.enabled,
        mask: edited.mask,
        ..VoxelTerrainLayer::new(edited.kind)
    };
    true
}

/// A layer's or rule's title: its position and kind or material.
fn layer_title(index: usize, layer: &VoxelTerrainLayer) -> String {
    format!(
        "{}. {:?}{}",
        index + 1,
        layer.kind,
        if layer.enabled { "" } else { " (off)" }
    )
}

fn rule_title(index: usize, rule: &VoxelMaterialRule) -> String {
    format!("{}. {:?}", index + 1, rule.material)
}

/// The editor's own row: the label and the preset picker.  Everything else is
/// the panel's rows ([`VirtualRows`]).
impl gpui::Render for VoxelTerrainStackEditor {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        v_flex().w_full().child(pulsar_reflection::prims::editor_row(
            &self.label,
            Dropdown::new(&self.presets)
                .xsmall()
                .w(px(140.0))
                .placeholder("Apply preset"),
            cx,
        ))
    }
}

/// The panel's handle on the stack's rows.
struct StackRows {
    editor: Entity<VoxelTerrainStackEditor>,
    wake: ListWake,
    stale: Arc<AtomicBool>,
}

impl VirtualRows for StackRows {
    fn row_count(&self, cx: &App) -> usize {
        self.editor.read(cx).layout.len()
    }

    fn render_row(&self, index: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        self.editor
            .update(cx, |editor, cx| editor.render_entry(index, window, cx))
    }

    fn on_change(&self, wake: Arc<dyn Fn(&mut App) + Send + Sync>) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }

    fn mark_stale(&self) {
        self.stale.store(true, Ordering::Release);
    }
}

fn voxel_terrain_stack_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let wake: ListWake = Default::default();
    let stale = Arc::new(AtomicBool::new(false));
    let entity = cx.new(|cx| {
        VoxelTerrainStackEditor::new(args, wake.clone(), stale.clone(), window, cx)
    });
    BoundPropertyEditor::new(
        entity.clone(),
        |editor: &mut VoxelTerrainStackEditor, value: &VoxelTerrainStack, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
    .with_rows(Arc::new(StackRows {
        editor: entity,
        wake,
        stale,
    }))
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<VoxelTerrainStack>(),
        fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(voxel_terrain_stack_editor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_stacks_explain_themselves() {
        assert_eq!(validation_error(&VoxelTerrainStack::earth()), None);
        let mut stack = VoxelTerrainStack::earth();
        stack.layers.swap(0, 1);
        assert!(
            validation_error(&stack).is_some_and(|e| e.contains("Warp")),
            "{:?}",
            validation_error(&stack)
        );
    }
}
