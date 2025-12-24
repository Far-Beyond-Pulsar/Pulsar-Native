use gpui::{prelude::*, InteractiveElement as _, StatefulInteractiveElement as _, *};
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, IconName, Icon, Sizable, button::{Button, ButtonVariants}, input::{InputState, TextInput}};
use ui_types_common::{StructField, TypeRef, Visibility};

/// Component for editing a single struct field
pub struct FieldEditorView {
    pub field: StructField,
    pub index: usize,

    // Input states
    name_input: Entity<InputState>,
    doc_input: Entity<InputState>,

    // Editing state
    editing_name: bool,
    editing_doc: bool,
    
    // Subscriptions
    _subscriptions: Vec<gpui::Subscription>,
}

#[derive(Clone, Debug)]
pub enum FieldEditorEvent {
    FieldChanged(usize, StructField),
    RemoveRequested(usize),
    TypePickerRequested(usize),
}

impl FieldEditorView {
    pub fn new(field: StructField, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("field_name"));
        let doc_input = cx.new(|cx| InputState::new(window, cx).placeholder("Field documentation..."));

        // Initialize inputs
        name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &field.name, window, cx);
        });

        if let Some(doc) = &field.doc {
            doc_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, doc, window, cx);
            });
        }

        // Subscribe to input events
        let sub1 = cx.subscribe_in(&name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    if this.editing_name {
                        this.name_input.update(cx, |input, _cx| {
                            this.field.name = input.text().to_string();
                        });
                        cx.emit(FieldEditorEvent::FieldChanged(this.index, this.field.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_name {
                        this.editing_name = false;
                        this.name_input.update(cx, |input, _cx| {
                            this.field.name = input.text().to_string();
                        });
                        cx.emit(FieldEditorEvent::FieldChanged(this.index, this.field.clone()));
                        cx.notify();
                    }
                }
                _ => {}
            }
        });

        let sub2 = cx.subscribe_in(&doc_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    if this.editing_doc {
                        this.doc_input.update(cx, |input, _cx| {
                            let doc = input.text().to_string();
                            this.field.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(FieldEditorEvent::FieldChanged(this.index, this.field.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_doc {
                        this.editing_doc = false;
                        this.doc_input.update(cx, |input, _cx| {
                            let doc = input.text().to_string();
                            this.field.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(FieldEditorEvent::FieldChanged(this.index, this.field.clone()));
                        cx.notify();
                    }
                }
                _ => {}
            }
        });

        Self {
            field,
            index,
            name_input,
            doc_input,
            editing_name: false,
            editing_doc: false,
            _subscriptions: vec![sub1, sub2],
        }
    }

    pub fn update_field(&mut self, field: StructField, cx: &mut Context<Self>) {
        self.field = field.clone();
        cx.notify();
    }

    fn toggle_visibility(&mut self, cx: &mut Context<Self>) {
        self.field.visibility = match self.field.visibility {
            Visibility::Public => Visibility::Private,
            Visibility::Private => Visibility::Crate,
            Visibility::Crate => Visibility::Super,
            Visibility::Super => Visibility::Public,
        };
        cx.emit(FieldEditorEvent::FieldChanged(self.index, self.field.clone()));
        cx.notify();
    }

    fn type_ref_to_string(type_ref: &TypeRef) -> String {
        match type_ref {
            TypeRef::Primitive { name } => name.clone(),
            TypeRef::Path { path } => path.clone(),
            TypeRef::AliasRef { alias } => alias.clone(),
        }
    }
}

impl EventEmitter<FieldEditorEvent> for FieldEditorView {}

impl Render for FieldEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let index = self.index;
        let type_str = Self::type_ref_to_string(&self.field.type_ref);

        v_flex()
            .w_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().secondary.opacity(0.4))
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.0))
            .child(
                // Header row with name and actions
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        // Field name - editable inline
                        if self.editing_name {
                            TextInput::new(&self.name_input)
                                .flex_1()
                                .into_any_element()
                        } else {
                            h_flex()
                                .flex_1()
                                .items_center()
                                .child(
                                    div()
                                        .text_base()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(self.field.name.clone())
                                )
                                .child(
                                    Button::new(("edit-name", index))
                                        .ghost()
                                        .with_size(ui::Size::XSmall)
                                        .icon(IconName::Edit)
                                        .ml_2()
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.editing_name = true;
                                            cx.notify();
                                        }))
                                )
                                .into_any_element()
                        }
                    )
                    .child(
                        // Visibility badge
                        Button::new(("visibility", index))
                            .ghost()
                            .with_size(ui::Size::Small)
                            .label(match self.field.visibility {
                                Visibility::Public => "pub",
                                Visibility::Private => "priv",
                                Visibility::Crate => "crate",
                                Visibility::Super => "super",
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.toggle_visibility(cx);
                            }))
                    )
                    .child(
                        // Remove button
                        Button::new(("remove", index))
                            .ghost()
                            .with_size(ui::Size::Small)
                            .icon(IconName::Delete)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.emit(FieldEditorEvent::RemoveRequested(index));
                            }))
                    )
            )
            .child(
                // Type row - clickable to open type picker
                Button::new(("field-type-picker", index))
                    .w_full()
                    .ghost()
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        cx.emit(FieldEditorEvent::TypePickerRequested(index));
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .rounded(px(4.0))
                            .bg(cx.theme().secondary.opacity(0.3))
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Type:")
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(cx.theme().accent)
                                    .child(type_str)
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .text_color(cx.theme().muted_foreground)
                                    .size_3p5()
                            )
                    )
            )
            .when(self.field.doc.is_some() || self.editing_doc, |this| {
                this.child(
                    // Documentation row
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Documentation")
                                )
                                .when(!self.editing_doc, |this| {
                                    this.child(
                                        Button::new(("edit-doc", index))
                                            .ghost()
                                            .with_size(ui::Size::XSmall)
                                            .icon(IconName::Edit)
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.editing_doc = true;
                                                cx.notify();
                                            }))
                                    )
                                })
                        )
                        .child(
                            if self.editing_doc {
                                TextInput::new(&self.doc_input)
                                    .into_any_element()
                            } else {
                                div()
                                    .p_2()
                                    .rounded(px(4.0))
                                    .bg(cx.theme().secondary.opacity(0.2))
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("/// {}", self.field.doc.as_ref().unwrap_or(&String::from("Click to add documentation"))))
                                    .into_any_element()
                            }
                        )
                )
            })
    }
}
