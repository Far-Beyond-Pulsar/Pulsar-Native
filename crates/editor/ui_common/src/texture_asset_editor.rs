//! Reflected texture asset editor backed by the shared searchable asset picker.

use crate::asset_picker::{AssetPickedEvent, AssetQuery, MeshAssetPicker};
use gpui::{prelude::*, *};
use pulsar_reflection::{
    BoundPropertyEditor, PropertyEditorArgs, PropertyEditorFactory, TextureSrc,
};
use std::path::PathBuf;
use ui::button::{Button, ButtonVariants as _};
use ui::{h_flex, popover::Popover, ActiveTheme, Sizable};

struct TextureSrcEditor {
    label: String,
    id_prefix: String,
    prop_name: String,
    picker: Entity<MeshAssetPicker>,
    path: String,
    write_back: pulsar_reflection::PropertyWriteBack,
    _subscriptions: Vec<Subscription>,
}

impl TextureSrcEditor {
    fn new(args: &PropertyEditorArgs<'_>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = args
            .current_value
            .downcast_ref::<TextureSrc>()
            .map(|value| value.0.clone())
            .unwrap_or_default();
        let project_root = engine_state::get_project_path().map(PathBuf::from);
        let picker = cx.new(|cx| {
            MeshAssetPicker::new(
                path.clone(),
                Vec::new(),
                project_root,
                AssetQuery::texture_images(),
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
                this.path = selected.clone();
                (this.write_back)(Box::new(TextureSrc::new(selected)), window, cx);
                cx.notify();
            },
        )];

        Self {
            label: args.display_name.to_owned(),
            id_prefix: args.id_prefix.to_owned(),
            prop_name: args.prop_name.to_owned(),
            picker,
            path,
            write_back: args.write_back.clone(),
            _subscriptions: subscriptions,
        }
    }

    fn set_value(&mut self, value: &TextureSrc, cx: &mut Context<Self>) {
        if self.path == value.0 {
            return;
        }
        self.path = value.0.clone();
        self.picker
            .update(cx, |picker, _| picker.set_selected_path(value.0.clone()));
        cx.notify();
    }
}

impl Render for TextureSrcEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display = if self.path.is_empty() {
            "No texture selected".to_owned()
        } else {
            std::path::Path::new(&self.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&self.path)
                .to_owned()
        };
        let picker = self.picker.clone();
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone()),
            )
            .child(
                Popover::<MeshAssetPicker>::new(format!(
                    "texture-asset-picker-{}-{}",
                    self.id_prefix, self.prop_name
                ))
                .anchor(Corner::BottomRight)
                .trigger(
                    Button::new(format!(
                        "texture-asset-picker-btn-{}-{}",
                        self.id_prefix, self.prop_name
                    ))
                    .label(display)
                    .small()
                    .ghost()
                    .dropdown_caret(true),
                )
                .content(move |_window, _cx| picker.clone()),
            )
    }
}

fn texture_src_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| TextureSrcEditor::new(args, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut TextureSrcEditor, value: &TextureSrc, _window, cx| {
            editor.set_value(value, cx)
        },
    )
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<TextureSrc>(),
        fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(
            texture_src_editor as PropertyEditorFactory
        ),
    }
}
