//! Property editor for [`VoxelGeneratorRef`]: a searchable picker of the
//! registered voxel terrain generators.
//!
//! Choosing a generator sets its id and version together. A generator that
//! is not registered in this build (a plugin that is not loaded) stays
//! selected and is listed as missing, so opening a level never rewrites it.

use gpui::{Entity, SharedString, Subscription, Window, prelude::*, px};
use helio_pass_voxel_planet::terrain::{GeneratorInfo, generators};

use super::VoxelGeneratorRef;
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyWriteBack};
use ui::dropdown::{Dropdown, DropdownEvent, DropdownItem, DropdownState, SearchableVec};
use ui::{IndexPath, Sizable};

/// One entry of the picker.
#[derive(Clone, Debug)]
pub struct GeneratorItem {
    value: VoxelGeneratorRef,
    title: SharedString,
    /// Lower-case name, id and description, for search.
    search: String,
}

impl GeneratorItem {
    fn registered(choice: &GeneratorInfo, versions: usize) -> Self {
        let title = if versions > 1 {
            format!("{} (v{})", choice.name, choice.version)
        } else {
            choice.name.clone()
        };
        Self {
            value: VoxelGeneratorRef::new(choice.id.clone(), choice.version),
            title: title.into(),
            search: format!("{} {} {}", choice.name, choice.id, choice.description).to_lowercase(),
        }
    }

    fn missing(value: &VoxelGeneratorRef) -> Self {
        let title = if value.id.is_empty() {
            "None (external data)".to_string()
        } else {
            format!("{} v{} (not loaded)", value.id, value.version)
        };
        Self {
            value: value.clone(),
            title: title.into(),
            search: value.id.to_lowercase(),
        }
    }
}

impl DropdownItem for GeneratorItem {
    type Value = VoxelGeneratorRef;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }

    fn matches(&self, query: &str) -> bool {
        self.search.contains(&query.to_lowercase())
    }
}

/// The picker's entries: every registered generator by name, preceded by
/// the current value when it is not registered.
pub fn generator_items(
    current: &VoxelGeneratorRef,
    choices: &[GeneratorInfo],
) -> Vec<GeneratorItem> {
    let mut items: Vec<_> = choices
        .iter()
        .map(|choice| {
            GeneratorItem::registered(choice, choices.iter().filter(|c| c.id == choice.id).count())
        })
        .collect();
    if !items.iter().any(|item| item.value == *current) {
        items.insert(0, GeneratorItem::missing(current));
    }
    items
}

type Items = SearchableVec<GeneratorItem>;

pub struct VoxelGeneratorEditor {
    label: String,
    value: VoxelGeneratorRef,
    state: Entity<DropdownState<Items>>,
    write_back: PropertyWriteBack,
    _subs: Vec<Subscription>,
}

impl VoxelGeneratorEditor {
    fn new(
        args: &PropertyEditorArgs<'_>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let value = args
            .current_value
            .downcast_ref::<VoxelGeneratorRef>()
            .cloned()
            .unwrap_or_default();
        let state = Self::dropdown(&value, window, cx);
        let subs = vec![Self::subscribe(&state, window, cx)];
        Self {
            label: args.display_name.to_string(),
            value,
            state,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    fn dropdown(
        value: &VoxelGeneratorRef,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Entity<DropdownState<Items>> {
        let items = generator_items(value, &generators());
        let selected = items
            .iter()
            .position(|item| item.value == *value)
            .map(|row| IndexPath::default().row(row));
        cx.new(|cx| DropdownState::new(SearchableVec::new(items), selected, window, cx))
    }

    fn subscribe(
        state: &Entity<DropdownState<Items>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            state,
            window,
            |this: &mut Self, _, event: &DropdownEvent<Items>, window, cx| {
                let DropdownEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if *value == this.value {
                    return;
                }
                this.value = value.clone();
                (this.write_back)(Box::new(value.clone()), window, cx);
            },
        )
    }

    /// Accept a value that changed elsewhere (undo, another editor).
    fn set_value(
        &mut self,
        value: &VoxelGeneratorRef,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.value == *value {
            return;
        }
        self.value = value.clone();
        // A value outside the current list (a missing generator) needs a new list.
        self.state = Self::dropdown(value, window, cx);
        self._subs = vec![Self::subscribe(&self.state, window, cx)];
        cx.notify();
    }
}

impl gpui::Render for VoxelGeneratorEditor {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        pulsar_reflection::prims::editor_row(
            &self.label,
            Dropdown::new(&self.state)
                .xsmall()
                .w(px(180.0))
                .menu_width(px(280.0)),
            cx,
        )
    }
}

fn voxel_generator_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| VoxelGeneratorEditor::new(args, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut VoxelGeneratorEditor, value: &VoxelGeneratorRef, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<VoxelGeneratorRef>(),
        fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(voxel_generator_editor),
    }
}
