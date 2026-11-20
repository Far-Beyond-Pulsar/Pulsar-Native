//! The reflected editor for a property that names an asset by project-relative
//! path: the shared searchable picker ([`AssetPicker`]) restricted to some file
//! extensions, in a popover, with a clear button.
//!
//! A path property is a transparent newtype over `String` (so saved data does
//! not change) that implements [`AssetPathValue`]; its type registers
//! [`asset_path_editor`] as its `#[pulsar_type(editor = ...)]`, as the mesh
//! component's `MeshAssetPath` does. A typed reference (`AssetRef<K>`) uses
//! [`crate::asset_ref_editor`] instead.

use crate::asset_picker::{AssetPickedEvent, AssetPicker, AssetQuery};
use gpui::{prelude::*, *};
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs};
use std::path::{Path, PathBuf};
use ui::button::{Button, ButtonVariants as _};
use ui::{h_flex, popover::Popover, ActiveTheme, Sizable};

/// A property value that is a project-relative asset path.
pub trait AssetPathValue: Clone + Send + Sync + 'static {
    fn from_path(path: String) -> Self;
    fn path(&self) -> &str;
}

struct AssetPathEditor<P: AssetPathValue> {
    label: String,
    id_prefix: String,
    prop_name: String,
    empty_text: &'static str,
    picker: Entity<AssetPicker>,
    path: String,
    write_back: pulsar_reflection::PropertyWriteBack,
    _subscriptions: Vec<Subscription>,
    _value: std::marker::PhantomData<fn() -> P>,
}

fn project_root() -> Option<PathBuf> {
    engine_state::get_project_path().map(PathBuf::from)
}

impl<P: AssetPathValue> AssetPathEditor<P> {
    fn new(
        args: &PropertyEditorArgs<'_>,
        empty_text: &'static str,
        extensions: &'static [&'static str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = args
            .current_value
            .downcast_ref::<P>()
            .map(|value| value.path().to_owned())
            .unwrap_or_default();
        let queries = extensions
            .iter()
            .map(|extension| AssetQuery::extension(*extension))
            .collect();
        let picker = cx.new(|cx| {
            AssetPicker::new(path.clone(), Vec::new(), project_root(), queries, window, cx)
        });
        let subscriptions = vec![cx.subscribe_in(
            &picker,
            window,
            |this: &mut Self, picker, _event: &AssetPickedEvent, window, cx| {
                let selected = picker.read(cx).selected_path().to_owned();
                if this.path == selected {
                    return;
                }
                this.path = selected.clone();
                (this.write_back)(Box::new(P::from_path(selected)), window, cx);
                cx.notify();
            },
        )];
        Self {
            label: args.display_name.to_owned(),
            id_prefix: args.id_prefix.to_owned(),
            prop_name: args.prop_name.to_owned(),
            empty_text,
            picker,
            path,
            write_back: args.write_back.clone(),
            _subscriptions: subscriptions,
            _value: std::marker::PhantomData,
        }
    }

    fn set_value(&mut self, value: &P, cx: &mut Context<Self>) {
        if self.path == value.path() {
            return;
        }
        self.path = value.path().to_owned();
        let path = self.path.clone();
        self.picker
            .update(cx, |picker, _| picker.set_selected_path(path));
        cx.notify();
    }
}

impl<P: AssetPathValue> Render for AssetPathEditor<P> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display = if self.path.is_empty() {
            self.empty_text.to_owned()
        } else {
            Path::new(&self.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&self.path)
                .to_owned()
        };
        let picker = self.picker.clone();
        let picker_for_clear = self.picker.clone();
        let clear_enabled = !self.path.is_empty();
        let id = format!("asset-path-{}-{}", self.id_prefix, self.prop_name);
        let clear_id = format!("{id}-clear");
        let write_back = self.write_back.clone();
        let entity = cx.entity();
        let muted = cx.theme().muted_foreground;

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .py_1()
            .child(div().text_sm().text_color(muted).child(self.label.clone()))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Popover::<AssetPicker>::new(format!("{id}-picker"))
                            .anchor(Corner::BottomRight)
                            .trigger(
                                Button::new(format!("{id}-btn"))
                                    .label(display)
                                    .small()
                                    .ghost()
                                    .dropdown_caret(true),
                            )
                            .content(move |_window, _cx| picker.clone()),
                    )
                    .when(clear_enabled, |row| {
                        row.child(
                            Button::new(clear_id)
                                .label("Clear")
                                .small()
                                .ghost()
                                .tooltip("Remove the path")
                                .on_click(move |_, window, cx| {
                                    picker_for_clear
                                        .update(cx, |picker, _| picker.set_selected_path(String::new()));
                                    entity.update(cx, |this, cx| {
                                        this.path.clear();
                                        cx.notify();
                                    });
                                    write_back(Box::new(P::from_path(String::new())), window, cx);
                                }),
                        )
                    }),
            )
    }
}

/// A property editor for a path property `P`: the searchable picker restricted
/// to `extensions` (without dots), with a clear button.
pub fn asset_path_editor<P: AssetPathValue>(
    args: &PropertyEditorArgs<'_>,
    empty_text: &'static str,
    extensions: &'static [&'static str],
    window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| AssetPathEditor::<P>::new(args, empty_text, extensions, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut AssetPathEditor<P>, value: &P, _window, cx| editor.set_value(value, cx),
    )
}
