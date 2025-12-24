use gpui::{prelude::*, InteractiveElement as _, StatefulInteractiveElement as _, *};
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, IconName, Icon, Sizable, button::{Button, ButtonVariants}, input::{InputState, TextInput}};
use ui_types_common::{EnumVariant, TypeRef, VariantPayload, StructField, Visibility};

/// Component for editing a single enum variant
pub struct VariantEditorView {
    pub variant: EnumVariant,
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
pub enum VariantEditorEvent {
    VariantChanged(usize, EnumVariant),
    RemoveRequested(usize),
    TypePickerRequested(usize),
    AddFieldRequested(usize),
}

impl VariantEditorView {
    pub fn new(variant: EnumVariant, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("variant_name"));
        let doc_input = cx.new(|cx| InputState::new(window, cx).placeholder("Variant documentation..."));

        // Initialize inputs
        name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &variant.name, window, cx);
        });

        if let Some(doc) = &variant.doc {
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
                            this.variant.name = input.text().to_string();
                        });
                        cx.emit(VariantEditorEvent::VariantChanged(this.index, this.variant.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_name {
                        this.editing_name = false;
                        this.name_input.update(cx, |input, _cx| {
                            this.variant.name = input.text().to_string();
                        });
                        cx.emit(VariantEditorEvent::VariantChanged(this.index, this.variant.clone()));
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
                            this.variant.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(VariantEditorEvent::VariantChanged(this.index, this.variant.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_doc {
                        this.editing_doc = false;
                        this.doc_input.update(cx, |input, _cx| {
                            let doc = input.text().to_string();
                            this.variant.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(VariantEditorEvent::VariantChanged(this.index, this.variant.clone()));
                        cx.notify();
                    }
                }
                _ => {}
            }
        });

        Self {
            variant,
            index,
            name_input,
            doc_input,
            editing_name: false,
            editing_doc: false,
            _subscriptions: vec![sub1, sub2],
        }
    }

    pub fn update_variant(&mut self, variant: EnumVariant, cx: &mut Context<Self>) {
        self.variant = variant.clone();
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

impl EventEmitter<VariantEditorEvent> for VariantEditorView {}

impl Render for VariantEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let index = self.index;

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
                        // Variant name - editable inline
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
                                        .child(self.variant.name.clone())
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
                        // Remove button
                        Button::new(("remove", index))
                            .ghost()
                            .with_size(ui::Size::Small)
                            .icon(IconName::Delete)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.emit(VariantEditorEvent::RemoveRequested(index));
                            }))
                    )
            )
            // Render payload based on type
            .child(
                match &self.variant.payload {
                    VariantPayload::Unit => {
                        // Unit variant - show buttons to add data
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("add-tuple-payload", index))
                                    .label("Add Tuple Data")
                                    .icon(IconName::Plus)
                                    .with_size(ui::Size::Small)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.variant.payload = VariantPayload::Single(TypeRef::Primitive { name: "String".to_string() });
                                        cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new(("add-struct-payload", index))
                                    .label("Add Struct Data")
                                    .icon(IconName::Plus)
                                    .with_size(ui::Size::Small)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.variant.payload = VariantPayload::Struct(vec![
                                            StructField {
                                                name: "field1".to_string(),
                                                type_ref: TypeRef::Primitive { name: "String".to_string() },
                                                visibility: Visibility::Public,
                                                doc: None,
                                            }
                                        ]);
                                        cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                        cx.notify();
                                    }))
                            )
                            .into_any_element()
                    }
                    VariantPayload::Single(type_ref) => {
                        // Tuple variant - show type editor
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Tuple Payload")
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new(("convert-to-struct", index))
                                                    .ghost()
                                                    .with_size(ui::Size::XSmall)
                                                    .label("→ Struct")
                                                    .on_click(cx.listener({
                                                        let type_ref = type_ref.clone();
                                                        move |this, _, _window, cx| {
                                                            this.variant.payload = VariantPayload::Struct(vec![
                                                                StructField {
                                                                    name: "value".to_string(),
                                                                    type_ref: type_ref.clone(),
                                                                    visibility: Visibility::Public,
                                                                    doc: None,
                                                                }
                                                            ]);
                                                            cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                                            cx.notify();
                                                        }
                                                    }))
                                            )
                                            .child(
                                                Button::new(("remove-payload", index))
                                                    .ghost()
                                                    .with_size(ui::Size::XSmall)
                                                    .icon(IconName::Close)
                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                        this.variant.payload = VariantPayload::Unit;
                                                        cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                                        cx.notify();
                                                    }))
                                            )
                                    )
                            )
                            .child(
                                Button::new(("variant-type-picker", index))
                                    .w_full()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        cx.emit(VariantEditorEvent::TypePickerRequested(index));
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
                                                    .child(Self::type_ref_to_string(type_ref))
                                            )
                                            .child(
                                                Icon::new(IconName::ChevronRight)
                                                    .text_color(cx.theme().muted_foreground)
                                                    .size_3p5()
                                            )
                                    )
                            )
                            .into_any_element()
                    }
                    VariantPayload::Struct(fields) => {
                        // Struct variant - show field list
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("Struct Payload ({} fields)", fields.len()))
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new(("add-field", index))
                                                    .ghost()
                                                    .with_size(ui::Size::XSmall)
                                                    .icon(IconName::Plus)
                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                        if let VariantPayload::Struct(ref mut fields) = this.variant.payload {
                                                            fields.push(StructField {
                                                                name: format!("field{}", fields.len() + 1),
                                                                type_ref: TypeRef::Primitive { name: "String".to_string() },
                                                                visibility: Visibility::Public,
                                                                doc: None,
                                                            });
                                                            cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                                            cx.notify();
                                                        }
                                                    }))
                                            )
                                            .child(
                                                Button::new(("remove-payload", index))
                                                    .ghost()
                                                    .with_size(ui::Size::XSmall)
                                                    .icon(IconName::Close)
                                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                                        this.variant.payload = VariantPayload::Unit;
                                                        cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                                        cx.notify();
                                                    }))
                                            )
                                    )
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .children(
                                        fields.iter().enumerate().map(|(field_idx, field)| {
                                            h_flex()
                                                .gap_2()
                                                .p_2()
                                                .rounded(px(4.0))
                                                .bg(cx.theme().secondary.opacity(0.2))
                                                .border_1()
                                                .border_color(cx.theme().border.opacity(0.3))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_sm()
                                                        .text_color(cx.theme().foreground)
                                                        .child(format!("{}: {}", field.name, Self::type_ref_to_string(&field.type_ref)))
                                                )
                                                .child(
                                                    Button::new(SharedString::from(format!("remove-field-{}-{}", index, field_idx)))
                                                        .ghost()
                                                        .with_size(ui::Size::XSmall)
                                                        .icon(IconName::Close)
                                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                                            if let VariantPayload::Struct(ref mut fields) = this.variant.payload {
                                                                if field_idx < fields.len() {
                                                                    fields.remove(field_idx);
                                                                    cx.emit(VariantEditorEvent::VariantChanged(index, this.variant.clone()));
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                )
                                        })
                                    )
                            )
                            .when(fields.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p_3()
                                        .text_center()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("No fields - click + to add")
                                )
                            })
                            .into_any_element()
                    }
                }
            )
            .when(self.variant.doc.is_some() || self.editing_doc, |this| {
                this.child(
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
                                    .child(format!("/// {}", self.variant.doc.as_ref().unwrap_or(&String::from("Click to add documentation"))))
                                    .into_any_element()
                            }
                        )
                )
            })
    }
}
