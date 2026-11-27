//! The editor of an [`InputModeName`] value (the Set Input Mode pin): a
//! `Game` / `UI` dropdown.

use gpui::{prelude::*, *};
use pulsar_reflection::{
    BoundPropertyEditor, PropertyEditorArgs, PropertyEditorFactory, PropertyWriteBack,
};
use pulsar_world_registry::input_mode::InputModeName;
use ui::{button::Button, h_flex, menu::PopupMenuItem, ActiveTheme, Sizable};

struct InputModeEditor {
    label: String,
    id: SharedString,
    value: String,
    write_back: PropertyWriteBack,
}

impl Render for InputModeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let selected = self.value.clone();
        let muted = cx.theme().muted_foreground;
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .py_1()
            .child(div().text_sm().text_color(muted).child(self.label.clone()))
            .child(
                Button::new(self.id.clone())
                    .label(selected.clone())
                    .dropdown_caret(true)
                    .outline()
                    .small()
                    .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
                        InputModeName::CHOICES.iter().fold(menu, |menu, choice| {
                            let view = view.clone();
                            menu.item(
                                PopupMenuItem::new(*choice)
                                    .checked(choice.eq_ignore_ascii_case(&selected))
                                    .on_click(move |_, window, cx| {
                                        view.update(cx, |this, cx| {
                                            this.value = (*choice).to_owned();
                                            (this.write_back)(
                                                Box::new(InputModeName((*choice).to_owned())),
                                                window,
                                                cx,
                                            );
                                            cx.notify();
                                        });
                                    }),
                            )
                        })
                    }),
            )
    }
}

fn input_mode_editor(
    args: &PropertyEditorArgs<'_>,
    _window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let value = args
        .current_value
        .downcast_ref::<InputModeName>()
        .cloned()
        .unwrap_or_default();
    let editor = InputModeEditor {
        label: args.display_name.to_owned(),
        id: format!("input-mode-{}-{}", args.id_prefix, args.prop_name).into(),
        value: value.0,
        write_back: args.write_back.clone(),
    };
    let entity = cx.new(|_| editor);
    BoundPropertyEditor::new(
        entity,
        |editor: &mut InputModeEditor, value: &InputModeName, _window, cx| {
            if editor.value != value.0 {
                editor.value = value.0.clone();
                cx.notify();
            }
        },
    )
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::UiPropertyEditorHint {
        type_id: std::any::TypeId::of::<InputModeName>(),
        fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(
            input_mode_editor as PropertyEditorFactory
        ),
    }
}
