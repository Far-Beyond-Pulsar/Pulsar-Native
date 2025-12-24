use gpui::{prelude::*, InteractiveElement as _, StatefulInteractiveElement as _, *};
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, IconName, Icon, Sizable, button::{Button, ButtonVariants}, input::{InputState, TextInput}};
use ui_types_common::{TraitMethod, MethodSignature, MethodParam, TypeRef};

/// Component for editing a single trait method
pub struct MethodEditorView {
    pub method: TraitMethod,
    pub index: usize,

    // Input states
    name_input: Entity<InputState>,
    doc_input: Entity<InputState>,
    return_type_input: Entity<InputState>,

    // Editing state
    editing_name: bool,
    editing_doc: bool,
    editing_return_type: bool,
    
    // Subscriptions
    _subscriptions: Vec<gpui::Subscription>,
}

#[derive(Clone, Debug)]
pub enum MethodEditorEvent {
    MethodChanged(usize, TraitMethod),
    RemoveRequested(usize),
    AddParameterRequested(usize),
    TypePickerRequested(usize),
}

impl MethodEditorView {
    pub fn new(method: TraitMethod, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("method_name"));
        let doc_input = cx.new(|cx| InputState::new(window, cx).placeholder("Method documentation..."));
        let return_type_input = cx.new(|cx| InputState::new(window, cx).placeholder("ReturnType"));

        // Initialize inputs
        name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &method.name, window, cx);
        });

        if let Some(doc) = &method.doc {
            doc_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, doc, window, cx);
            });
        }

        let return_type_str = Self::type_ref_to_string(&method.signature.return_type);
        return_type_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &return_type_str, window, cx);
        });

        // Subscribe to input events
        let sub1 = cx.subscribe_in(&name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    if this.editing_name {
                        this.name_input.update(cx, |input, _cx| {
                            this.method.name = input.text().to_string();
                        });
                        cx.emit(MethodEditorEvent::MethodChanged(this.index, this.method.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_name {
                        this.editing_name = false;
                        this.name_input.update(cx, |input, _cx| {
                            this.method.name = input.text().to_string();
                        });
                        cx.emit(MethodEditorEvent::MethodChanged(this.index, this.method.clone()));
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
                            this.method.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(MethodEditorEvent::MethodChanged(this.index, this.method.clone()));
                        cx.notify();
                    }
                }
                ui::input::InputEvent::Blur => {
                    if this.editing_doc {
                        this.editing_doc = false;
                        this.doc_input.update(cx, |input, _cx| {
                            let doc = input.text().to_string();
                            this.method.doc = if doc.is_empty() { None } else { Some(doc) };
                        });
                        cx.emit(MethodEditorEvent::MethodChanged(this.index, this.method.clone()));
                        cx.notify();
                    }
                }
                _ => {}
            }
        });

        Self {
            method,
            index,
            name_input,
            doc_input,
            return_type_input,
            editing_name: false,
            editing_doc: false,
            editing_return_type: false,
            _subscriptions: vec![sub1, sub2],
        }
    }

    pub fn update_method(&mut self, method: TraitMethod, cx: &mut Context<Self>) {
        self.method = method.clone();
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

impl EventEmitter<MethodEditorEvent> for MethodEditorView {}

impl Render for MethodEditorView {
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
                        div()
                            .text_xs()
                            .px_2()
                            .py_1()
                            .rounded(px(3.0))
                            .bg(cx.theme().accent.opacity(0.2))
                            .text_color(cx.theme().accent)
                            .child("fn")
                    )
                    .child(
                        // Method name - editable inline
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
                                        .child(self.method.name.clone())
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
                                cx.emit(MethodEditorEvent::RemoveRequested(index));
                            }))
                    )
            )
            // Parameters section
            .child(
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
                                    .child(format!("Parameters ({})", self.method.signature.params.len()))
                            )
                            .child(
                                Button::new(("add-param", index))
                                    .ghost()
                                    .with_size(ui::Size::XSmall)
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.method.signature.params.push(MethodParam {
                                            name: format!("param{}", this.method.signature.params.len() + 1),
                                            type_ref: TypeRef::Primitive { name: "String".to_string() },
                                        });
                                        cx.emit(MethodEditorEvent::MethodChanged(index, this.method.clone()));
                                        cx.notify();
                                    }))
                            )
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .children(
                                self.method.signature.params.iter().enumerate().map(|(param_idx, param)| {
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
                                                .child(format!("{}: {}", param.name, Self::type_ref_to_string(&param.type_ref)))
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("remove-param-{}-{}", index, param_idx)))
                                                .ghost()
                                                .with_size(ui::Size::XSmall)
                                                .icon(IconName::Close)
                                                .on_click(cx.listener(move |this, _, _window, cx| {
                                                    if param_idx < this.method.signature.params.len() {
                                                        this.method.signature.params.remove(param_idx);
                                                        cx.emit(MethodEditorEvent::MethodChanged(index, this.method.clone()));
                                                        cx.notify();
                                                    }
                                                }))
                                        )
                                })
                            )
                            .when(self.method.signature.params.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p_3()
                                        .text_center()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("No parameters - click + to add")
                                )
                            })
                    )
            )
            // Return type section
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Return Type")
                    )
                    .child(
                        Button::new(("return-type-picker", index))
                            .w_full()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.emit(MethodEditorEvent::TypePickerRequested(index));
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
                                            .child("→")
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .text_color(cx.theme().accent)
                                            .child(Self::type_ref_to_string(&self.method.signature.return_type))
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronRight)
                                            .text_color(cx.theme().muted_foreground)
                                            .size_3p5()
                                    )
                            )
                    )
            )
            // Default body section (optional)
            .when(self.method.default_body.is_some(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Default Implementation")
                        )
                        .child(
                            div()
                                .p_2()
                                .rounded(px(4.0))
                                .bg(cx.theme().secondary.opacity(0.2))
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.method.default_body.as_ref().unwrap().clone())
                        )
                )
            })
            // Documentation section
            .when(self.method.doc.is_some() || self.editing_doc, |this| {
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
                                    .child(format!("/// {}", self.method.doc.as_ref().unwrap_or(&String::from("Click to add documentation"))))
                                    .into_any_element()
                            }
                        )
                )
            })
    }
}
