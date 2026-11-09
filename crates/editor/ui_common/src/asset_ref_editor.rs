//! The reflected editor for a typed asset reference (`Option<AssetRef<K>>`):
//! the shared searchable picker restricted to the kind's file extension, a
//! clear button, and what the kind's editor says about the chosen file
//! ([`Describe`]). Each kind's editor attaches it to its reference types
//! (the animation plugin's Skeletal Mesh and Animation Graph slots).

use crate::asset_picker::{AssetPickedEvent, AssetPicker, AssetQuery};
use gpui::{prelude::*, *};
use pulsar_reflection::{AssetKind, AssetRef, BoundPropertyEditor, PropertyEditorArgs};
use std::path::{Path, PathBuf};
use ui::button::{Button, ButtonVariants as _};
use ui::{h_flex, popover::Popover, v_flex, ActiveTheme, Sizable};

/// What an editor says about the chosen asset, beyond its name: rows of
/// (label, value) from the chosen file and the project root.
pub type Describe = fn(&Path, &Path) -> Vec<(&'static str, String)>;

struct AssetRefEditor<K: AssetKind> {
    label: String,
    id_prefix: String,
    prop_name: String,
    empty_text: &'static str,
    picker: Entity<AssetPicker>,
    path: String,
    describe: Describe,
    write_back: pulsar_reflection::PropertyWriteBack,
    _subscriptions: Vec<Subscription>,
    _kind: std::marker::PhantomData<fn() -> K>,
}

fn project_root() -> Option<PathBuf> {
    engine_state::get_project_path().map(PathBuf::from)
}

fn path_of<K: AssetKind>(value: &Option<AssetRef<K>>) -> String {
    let (Some(reference), Some(root)) = (value.as_ref(), project_root()) else {
        return String::new();
    };
    AssetPicker::resolve_asset_ref_path(&root, reference)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl<K: AssetKind> AssetRefEditor<K> {
    fn new(
        args: &PropertyEditorArgs<'_>,
        empty_text: &'static str,
        describe: Describe,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value = args
            .current_value
            .downcast_ref::<Option<AssetRef<K>>>()
            .cloned()
            .flatten();
        let path = path_of(&value);
        let picker = cx.new(|cx| {
            AssetPicker::new(
                path.clone(),
                Vec::new(),
                project_root(),
                AssetQuery::for_kind::<K>(),
                window,
                cx,
            )
        });
        let subscriptions = vec![cx.subscribe_in(
            &picker,
            window,
            |this: &mut Self, picker, _event: &AssetPickedEvent, window, cx| {
                let selected = picker.read(cx).selected_path().to_owned();
                if this.path == selected {
                    return;
                }
                let Some(reference) = picker.read(cx).selected_or_create_asset_ref::<K>() else {
                    // a file of another kind, or one the project index cannot id
                    picker.update(cx, |picker, _| picker.set_selected_path(this.path.clone()));
                    return;
                };
                this.path = selected;
                (this.write_back)(Box::new(Some(reference)), window, cx);
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
            describe,
            write_back: args.write_back.clone(),
            _subscriptions: subscriptions,
            _kind: std::marker::PhantomData,
        }
    }

    fn set_value(&mut self, value: &Option<AssetRef<K>>, cx: &mut Context<Self>) {
        let path = path_of(value);
        if self.path == path {
            return;
        }
        self.path = path.clone();
        self.picker
            .update(cx, |picker, _| picker.set_selected_path(path));
        cx.notify();
    }
}

impl<K: AssetKind> Render for AssetRefEditor<K> {
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
        let details = match (project_root(), self.path.is_empty()) {
            (Some(root), false) => (self.describe)(Path::new(&self.path), &root),
            _ => Vec::new(),
        };
        let picker = self.picker.clone();
        let clear_enabled = !self.path.is_empty();
        let id = format!("{}-{}-{}", K::KIND, self.id_prefix, self.prop_name);
        let clear_id = format!("{id}-clear");
        let picker_for_clear = self.picker.clone();
        let write_back = self.write_back.clone();
        let entity = cx.entity();
        let muted = cx.theme().muted_foreground;

        v_flex()
            .w_full()
            .gap_0p5()
            .py_1()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
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
                                        .tooltip("Remove the reference")
                                        .on_click(move |_, window, cx| {
                                            picker_for_clear.update(cx, |picker, _| {
                                                picker.set_selected_path(String::new())
                                            });
                                            entity.update(cx, |this, cx| {
                                                this.path.clear();
                                                cx.notify();
                                            });
                                            write_back(Box::new(None::<AssetRef<K>>), window, cx);
                                        }),
                                )
                            }),
                    ),
            )
            .children(details.into_iter().map(|(name, value)| {
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child(name))
                    .child(div().text_xs().text_color(muted).child(value))
            }))
    }
}

/// No rows beyond the asset's name.
pub fn no_details(_: &Path, _: &Path) -> Vec<(&'static str, String)> {
    Vec::new()
}

/// A property editor for an `Option<AssetRef<K>>` property: the searchable
/// picker restricted to `K`'s files, a clear button and, below, what
/// `describe` says about the chosen file. A kind's editor registers it for
/// its reference types (`pulsar_reflection::UiPropertyEditorHint`).
pub fn asset_ref_editor<K: AssetKind>(
    args: &PropertyEditorArgs<'_>,
    empty_text: &'static str,
    describe: Describe,
    window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| AssetRefEditor::<K>::new(args, empty_text, describe, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut AssetRefEditor<K>, value: &Option<AssetRef<K>>, _window, cx| {
            editor.set_value(value, cx)
        },
    )
}
