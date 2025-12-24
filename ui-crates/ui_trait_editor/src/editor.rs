use gpui::{*, prelude::FluentBuilder, actions, InteractiveElement as _, StatefulInteractiveElement as _};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, IconName, Disableable,
    dock::{Panel, PanelEvent},
    button::{Button, ButtonVariants},
    divider::Divider,
    input::{InputState, TextInput},
};
use ui_types_common::{TraitAsset, TraitMethod, MethodSignature, MethodParam, TypeRef, TypeKind};
use std::path::PathBuf;
use crate::method_editor::{MethodEditorView, MethodEditorEvent};

actions!(trait_editor, [
    Save,
    AddMethod,
    TogglePreview,
]);

#[derive(Clone, Debug)]
pub enum TraitEditorEvent {
    Modified,
    Saved,
}

pub struct TraitEditor {
    file_path: Option<PathBuf>,
    asset: TraitAsset,
    error_message: Option<String>,
    focus_handle: FocusHandle,

    // Input states for properties
    name_input: Entity<InputState>,
    display_name_input: Entity<InputState>,
    description_input: Entity<InputState>,

    // Method editors
    method_editors: Vec<Entity<MethodEditorView>>,

    // Code preview
    code_preview_input: Entity<InputState>,
    show_preview: bool,
    preview_needs_update: bool,

    // Modified flag
    modified: bool,
    
    // Subscriptions to keep them alive
    _subscriptions: Vec<gpui::Subscription>,
}

impl TraitEditor {
    pub fn new_with_file(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the trait data
        let (asset, error_message) = match std::fs::read_to_string(&file_path) {
            Ok(json_content) => {
                match serde_json::from_str::<TraitAsset>(&json_content) {
                    Ok(asset) => (asset, None),
                    Err(e) => (
                        Self::create_empty_asset(),
                        Some(format!("Failed to parse trait: {}", e))
                    ),
                }
            }
            Err(e) => (
                Self::create_empty_asset(),
                Some(format!("Failed to read file: {}", e))
            ),
        };

        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("TraitName"));
        let display_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Display Name"));
        let description_input = cx.new(|cx| InputState::new(window, cx).placeholder("Optional description"));
        let code_preview_input = cx.new(|cx| {
            use ui::input::TabSize;
            InputState::new(window, cx)
                .code_editor("rust")
                .line_number(true)
                .minimap(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
        });

        // Initialize input states with asset data
        name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &asset.name, window, cx);
        });
        display_name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &asset.display_name, window, cx);
        });
        if let Some(desc) = &asset.description {
            description_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, desc, window, cx);
            });
        }

        // Create method editors
        let mut method_editors = Vec::new();
        for (index, method) in asset.methods.iter().enumerate() {
            let editor = cx.new(|cx| MethodEditorView::new(method.clone(), index, window, cx));

            // Subscribe to method editor events
            cx.subscribe(&editor, |this: &mut Self, _, event: &MethodEditorEvent, cx| {
                match event {
                    MethodEditorEvent::MethodChanged(index, method) => {
                        if *index < this.asset.methods.len() {
                            this.asset.methods[*index] = method.clone();
                            this.modified = true;
                            this.preview_needs_update = true;
                            cx.notify();
                        }
                    }
                    MethodEditorEvent::RemoveRequested(index) => {
                        if *index < this.asset.methods.len() {
                            this.asset.methods.remove(*index);
                            this.method_editors.remove(*index);

                            // Update indices for remaining method editors
                            for (i, editor) in this.method_editors.iter().enumerate() {
                                editor.update(cx, |ed, _cx| {
                                    ed.index = i;
                                });
                            }

                            this.modified = true;
                            this.preview_needs_update = true;
                            cx.emit(TraitEditorEvent::Modified);
                            cx.notify();
                        }
                    }
                    MethodEditorEvent::TypePickerRequested(index) => {
                        eprintln!("Type picker requested for method {}", index);
                    }
                    MethodEditorEvent::AddParameterRequested(index) => {
                        eprintln!("Add parameter requested for method {}", index);
                    }
                }
            }).detach();

            method_editors.push(editor);
        }

        // Subscribe to input changes for the main properties
        let sub1 = cx.subscribe_in(&name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    this.preview_needs_update = true;
                    cx.emit(TraitEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        });

        let sub2 = cx.subscribe_in(&display_name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    cx.emit(TraitEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        });

        let sub3 = cx.subscribe_in(&description_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    this.preview_needs_update = true;
                    cx.emit(TraitEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        });

        let mut editor = Self {
            file_path: Some(file_path),
            asset,
            error_message,
            focus_handle: cx.focus_handle(),
            name_input,
            display_name_input,
            description_input,
            code_preview_input,
            show_preview: true,
            preview_needs_update: true,
            modified: false,
            method_editors,
            _subscriptions: vec![sub1, sub2, sub3],
        };

        // Initialize preview
        editor.update_preview(window, cx);
        editor.preview_needs_update = false;

        editor
    }

    fn create_empty_asset() -> TraitAsset {
        TraitAsset {
            schema_version: 1,
            type_kind: TypeKind::Trait,
            name: String::from("NewTrait"),
            display_name: String::from("New Trait"),
            description: None,
            methods: Vec::new(),
            meta: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        // Update asset from input states
        self.name_input.update(cx, |input, _cx| {
            self.asset.name = input.text().to_string();
        });
        self.display_name_input.update(cx, |input, _cx| {
            self.asset.display_name = input.text().to_string();
        });
        self.description_input.update(cx, |input, _cx| {
            let desc = input.text().to_string();
            self.asset.description = if desc.is_empty() { None } else { Some(desc) };
        });

        if let Some(file_path) = &self.file_path {
            match serde_json::to_string_pretty(&self.asset) {
                Ok(json) => {
                    match std::fs::write(file_path, json) {
                        Ok(_) => {
                            self.modified = false;
                            self.error_message = None;
                            cx.emit(TraitEditorEvent::Saved);
                            cx.notify();
                        }
                        Err(e) => {
                            self.error_message = Some(format!("Failed to save: {}", e));
                            cx.notify();
                        }
                    }
                }
                Err(e) => {
                    self.error_message = Some(format!("Failed to serialize: {}", e));
                    cx.notify();
                }
            }
        }
    }

    fn add_method(&mut self, _: &AddMethod, window: &mut Window, cx: &mut Context<Self>) {
        let new_method = TraitMethod {
            name: format!("method{}", self.asset.methods.len() + 1),
            signature: MethodSignature {
                params: Vec::new(),
                return_type: TypeRef::Primitive { name: "()".to_string() },
            },
            default_body: None,
            doc: None,
        };

        let index = self.asset.methods.len();
        self.asset.methods.push(new_method.clone());

        let editor = cx.new(|cx| MethodEditorView::new(new_method, index, window, cx));

        // Subscribe to method editor events
        cx.subscribe(&editor, |this: &mut Self, _, event: &MethodEditorEvent, cx| {
            match event {
                MethodEditorEvent::MethodChanged(index, method) => {
                    if *index < this.asset.methods.len() {
                        this.asset.methods[*index] = method.clone();
                        this.modified = true;
                        this.preview_needs_update = true;
                        cx.notify();
                    }
                }
                MethodEditorEvent::RemoveRequested(index) => {
                    this.remove_method(*index, cx);
                }
                MethodEditorEvent::TypePickerRequested(index) => {
                    eprintln!("Type picker requested for method {}", index);
                }
                MethodEditorEvent::AddParameterRequested(index) => {
                    eprintln!("Add parameter requested for method {}", index);
                }
            }
        }).detach();

        self.method_editors.push(editor);

        self.modified = true;
        self.preview_needs_update = true;
        cx.emit(TraitEditorEvent::Modified);
        cx.notify();
    }

    fn remove_method(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.asset.methods.len() {
            self.asset.methods.remove(index);
            self.method_editors.remove(index);

            // Update indices for remaining method editors
            for (i, editor) in self.method_editors.iter().enumerate() {
                editor.update(cx, |ed, cx| {
                    ed.index = i;
                    if i < self.asset.methods.len() {
                        ed.update_method(self.asset.methods[i].clone(), cx);
                    }
                });
            }

            self.modified = true;
            self.preview_needs_update = true;
            cx.emit(TraitEditorEvent::Modified);
            cx.notify();
        }
    }

    fn toggle_preview(&mut self, _: &TogglePreview, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_preview = !self.show_preview;
        cx.notify();
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Sync current input values to asset before generating code
        self.name_input.update(cx, |input, _cx| {
            self.asset.name = input.text().to_string();
        });
        self.display_name_input.update(cx, |input, _cx| {
            self.asset.display_name = input.text().to_string();
        });
        self.description_input.update(cx, |input, _cx| {
            let desc = input.text().to_string();
            self.asset.description = if desc.is_empty() { None } else { Some(desc) };
        });
        
        let code = self.generate_rust_code();
        self.code_preview_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &code, window, cx);
        });
    }

    fn generate_rust_code(&self) -> String {
        let mut code = String::new();

        // Add documentation if present
        if let Some(desc) = &self.asset.description {
            code.push_str(&format!("/// {}\n", desc));
        }

        // Trait declaration
        code.push_str(&format!("pub trait {} {{\n", self.asset.name));

        // Methods
        for method in &self.asset.methods {
            if let Some(doc) = &method.doc {
                code.push_str(&format!("    /// {}\n", doc));
            }

            // Method signature
            code.push_str("    fn ");
            code.push_str(&method.name);
            code.push_str("(&self");

            // Parameters
            for param in &method.signature.params {
                code.push_str(", ");
                code.push_str(&param.name);
                code.push_str(": ");
                code.push_str(&self.type_ref_to_string(&param.type_ref));
            }

            code.push_str(")");

            // Return type
            let return_type = self.type_ref_to_string(&method.signature.return_type);
            if return_type != "()" {
                code.push_str(" -> ");
                code.push_str(&return_type);
            }

            // Default implementation
            if let Some(body) = &method.default_body {
                code.push_str(" {\n");
                for line in body.lines() {
                    code.push_str(&format!("        {}\n", line));
                }
                code.push_str("    }\n");
            } else {
                code.push_str(";\n");
            }

            code.push('\n');
        }

        code.push_str("}\n");
        code
    }

    fn type_ref_to_string(&self, type_ref: &TypeRef) -> String {
        match type_ref {
            TypeRef::Primitive { name } => name.clone(),
            TypeRef::Path { path } => path.clone(),
            TypeRef::AliasRef { alias } => alias.clone(),
        }
    }

    fn render_toolbar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("save-btn")
                    .primary()
                    .label("Save")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save(&Save, window, cx);
                    }))
                    .when(!self.modified, |this| this.disabled(true))
            )
            .child(
                Button::new("add-method-btn")
                    .label("Add Method")
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_method(&AddMethod, window, cx);
                    }))
            )
            .child(
                Button::new("toggle-preview-btn")
                    .label(if self.show_preview { "Hide Preview" } else { "Show Preview" })
                    .icon(IconName::Eye)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_preview(&TogglePreview, window, cx);
                    }))
            )
            .when(self.modified, |this| {
                this.child(
                    div()
                        .ml_auto()
                        .px_2()
                        .py_1()
                        .rounded(px(4.0))
                        .bg(cx.theme().accent.opacity(0.2))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().accent)
                                .child("Modified")
                        )
                )
            })
    }

    fn render_properties_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Name")
                    )
                    .child(
                        TextInput::new(&self.name_input)
                    )
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Display Name")
                    )
                    .child(
                        TextInput::new(&self.display_name_input)
                    )
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Description")
                    )
                    .child(
                        TextInput::new(&self.description_input)
                    )
            )
    }

    fn render_methods_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(format!("Methods ({})", self.asset.methods.len()))
                    )
                    .child(
                        Button::new("add-method-inline")
                            .label("Add")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_method(&AddMethod, window, cx);
                            }))
                    )
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .gap_3()
                            .scrollable(gpui::Axis::Vertical)
                            .children(
                                self.method_editors.iter().map(|editor| editor.clone())
                            )
                            .when(self.method_editors.is_empty(), |this| {
                                this.child(
                                    div()
                                        .w_full()
                                        .p_8()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_3xl()
                                                .child("🔧")
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No methods yet")
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                                .child("Click 'Add Method' to get started")
                                        )
                                )
                            })
                    )
            )
    }

    fn render_code_preview(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Generated Code")
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_1()
                            .rounded(px(3.0))
                            .bg(cx.theme().secondary.opacity(0.5))
                            .text_color(cx.theme().muted_foreground)
                            .child("Read-only")
                    )
            )
            .child(Divider::horizontal())
            .child(
                TextInput::new(&self.code_preview_input)
                    .flex_1()
                    .disabled(true)
            )
    }
}

impl EventEmitter<TraitEditorEvent> for TraitEditor {}
impl EventEmitter<PanelEvent> for TraitEditor {}

impl Render for TraitEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update preview if needed
        if self.preview_needs_update {
            self.update_preview(window, cx);
            self.preview_needs_update = false;
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .key_context("TraitEditor")
            .on_action(cx.listener(|this, action, window, cx| {
                this.save(action, window, cx);
            }))
            .on_action(cx.listener(|this, action, window, cx| {
                this.add_method(action, window, cx);
            }))
            .on_action(cx.listener(|this, action, window, cx| {
                this.toggle_preview(action, window, cx);
            }))
            .child(
                // Header with toolbar
                v_flex()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .bg(cx.theme().secondary.opacity(0.5))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_2xl()
                                    .child("🔧")
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(self.asset.display_name.clone())
                            )
                    )
                    .child(self.render_toolbar(window, cx))
            )
            .when(self.error_message.is_some(), |this| {
                let error = self.error_message.as_ref().unwrap().clone();
                this.child(
                    div()
                        .p_4()
                        .bg(hsla(0.0, 0.8, 0.5, 0.1))
                        .border_b_1()
                        .border_color(hsla(0.0, 0.8, 0.5, 1.0))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .child("⚠")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(hsla(0.0, 0.8, 0.5, 1.0))
                                        .child(error)
                                )
                        )
                )
            })
            .child(
                // Main content
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        // Left panel - Properties
                        v_flex()
                            .w(px(300.0))
                            .h_full()
                            .gap_4()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(
                                v_flex()
                                    .p_4()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_bold()
                                            .text_color(cx.theme().foreground)
                                            .child("Properties")
                                    )
                                    .child(Divider::horizontal())
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .px_4()
                                    .overflow_hidden()
                                    .child(
                                        v_flex()
                                            .scrollable(gpui::Axis::Vertical)
                                            .child(self.render_properties_panel(window, cx))
                                    )
                            )
                    )
                    .child(
                        // Center panel - Methods
                        v_flex()
                            .flex_1()
                            .h_full()
                            .p_4()
                            .gap_3()
                            .overflow_hidden()
                            .when(self.show_preview, |this| {
                                this.border_r_1()
                                    .border_color(cx.theme().border)
                            })
                            .child(self.render_methods_panel(window, cx))
                    )
                    .when(self.show_preview, |this| {
                        this.child(
                            // Right panel - Code Preview
                            v_flex()
                                .w(px(500.0))
                                .h_full()
                                .p_4()
                                .overflow_hidden()
                                .child(self.render_code_preview(window, cx))
                        )
                    })
            )
    }
}

impl Focusable for TraitEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TraitEditor {
    fn panel_name(&self) -> &'static str {
        "Trait Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        format!(
            "{}{}",
            self.asset.display_name,
            if self.modified { " •" } else { "" }
        )
        .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        let info = self.file_path.as_ref().map(|p| {
            serde_json::json!({
                "file_path": p.to_string_lossy().to_string()
            })
        }).unwrap_or(serde_json::Value::Null);

        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            info: ui::dock::PanelInfo::Panel(info),
            ..Default::default()
        }
    }
}
