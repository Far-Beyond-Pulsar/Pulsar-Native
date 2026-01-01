use gpui::{*, prelude::FluentBuilder};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, IconName,
    dock::{Panel, PanelEvent},
    button::{Button, ButtonVariants},
    divider::Divider,
    input::{InputState, TextInput},
};
use ui_types_common::{TraitAsset, TraitMethod, MethodSignature, MethodParam, TypeRef};
use std::sync::Arc;
use crate::method_editor::{MethodEditorView, MethodEditorEvent};

/// Properties Panel - Edit trait metadata
pub struct PropertiesPanel {
    asset: Arc<parking_lot::RwLock<TraitAsset>>,
    name_input: Entity<InputState>,
    display_name_input: Entity<InputState>,
    description_input: Entity<InputState>,
    focus_handle: FocusHandle,
    on_modified: Arc<parking_lot::Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

impl PropertiesPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<TraitAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("TraitName"));
        let display_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Display Name"));
        let description_input = cx.new(|cx| InputState::new(window, cx).placeholder("Optional description"));

        // Initialize input states with asset data
        let asset_read = asset.read();
        name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &asset_read.name, window, cx);
        });
        display_name_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &asset_read.display_name, window, cx);
        });
        if let Some(desc) = &asset_read.description {
            description_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, desc, window, cx);
            });
        }
        drop(asset_read);

        // Subscribe to input changes
        cx.subscribe_in(&name_input, window, |this: &mut Self, _state, event: &ui::input::InputEvent, window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.name_input.update(cx, |input, _cx| {
                        this.asset.write().name = input.text().to_string();
                    });
                    this.notify_modified();
                    cx.emit(PanelEvent::LayoutChanged);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        cx.subscribe_in(&display_name_input, window, |this: &mut Self, _state, event: &ui::input::InputEvent, window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.display_name_input.update(cx, |input, _cx| {
                        this.asset.write().display_name = input.text().to_string();
                    });
                    this.notify_modified();
                    cx.emit(PanelEvent::LayoutChanged);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        cx.subscribe_in(&description_input, window, |this: &mut Self, _state, event: &ui::input::InputEvent, window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.description_input.update(cx, |input, _cx| {
                        let desc = input.text().to_string();
                        this.asset.write().description = if desc.is_empty() { None } else { Some(desc) };
                    });
                    this.notify_modified();
                    cx.emit(PanelEvent::LayoutChanged);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        Self {
            asset,
            name_input,
            display_name_input,
            description_input,
            focus_handle: cx.focus_handle(),
            on_modified: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    fn notify_modified(&self) {
        if let Some(callback) = self.on_modified.lock().as_ref() {
            callback();
        }
    }

    pub fn set_on_modified<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_modified.lock() = Some(Box::new(callback));
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanel {}

impl Render for PropertiesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .bg(cx.theme().sidebar)
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
                    .child(TextInput::new(&self.name_input))
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
                    .child(TextInput::new(&self.display_name_input))
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
                    .child(TextInput::new(&self.description_input))
            )
    }
}

impl Focusable for PropertiesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PropertiesPanel {
    fn panel_name(&self) -> &'static str {
        "Properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        "Properties".into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

/// Methods Panel - Manage trait methods
pub struct MethodsPanel {
    asset: Arc<parking_lot::RwLock<TraitAsset>>,
    method_editors: Vec<Entity<MethodEditorView>>,
    focus_handle: FocusHandle,
    on_modified: Arc<parking_lot::Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

impl MethodsPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<TraitAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let asset_read = asset.read();
        let mut method_editors = Vec::new();

        for (index, method) in asset_read.methods.iter().enumerate() {
            let editor = cx.new(|cx| MethodEditorView::new(method.clone(), index, window, cx));

            // Subscribe to method editor events
            cx.subscribe(&editor, |this: &mut Self, _, event: &MethodEditorEvent, cx| {
                match event {
                    MethodEditorEvent::MethodChanged(index, method) => {
                        let mut asset = this.asset.write();
                        if *index < asset.methods.len() {
                            asset.methods[*index] = method.clone();
                            drop(asset);
                            this.notify_modified();
                            cx.emit(PanelEvent::LayoutChanged);
                            cx.notify();
                        }
                    }
                    MethodEditorEvent::RemoveRequested(index) => {
                        this.remove_method(*index, cx);
                    }
                    MethodEditorEvent::TypePickerRequested(index) => {
                        tracing::info!("Type picker requested for method {}", index);
                    }
                    MethodEditorEvent::AddParameterRequested(index) => {
                        tracing::info!("Add parameter requested for method {}", index);
                    }
                }
            }).detach();

            method_editors.push(editor);
        }
        drop(asset_read);

        Self {
            asset,
            method_editors,
            focus_handle: cx.focus_handle(),
            on_modified: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    fn add_method(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new_method = TraitMethod {
            name: format!("method{}", self.asset.read().methods.len() + 1),
            signature: MethodSignature {
                params: Vec::new(),
                return_type: TypeRef::Primitive { name: "()".to_string() },
            },
            default_body: None,
            doc: None,
        };

        let index = {
            let mut asset = self.asset.write();
            let idx = asset.methods.len();
            asset.methods.push(new_method.clone());
            idx
        };

        let editor = cx.new(|cx| MethodEditorView::new(new_method, index, window, cx));

        // Subscribe to method editor events
        cx.subscribe(&editor, |this: &mut Self, _, event: &MethodEditorEvent, cx| {
            match event {
                MethodEditorEvent::MethodChanged(index, method) => {
                    let mut asset = this.asset.write();
                    if *index < asset.methods.len() {
                        asset.methods[*index] = method.clone();
                        drop(asset);
                        this.notify_modified();
                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                }
                MethodEditorEvent::RemoveRequested(index) => {
                    this.remove_method(*index, cx);
                }
                MethodEditorEvent::TypePickerRequested(index) => {
                    tracing::info!("Type picker requested for method {}", index);
                }
                MethodEditorEvent::AddParameterRequested(index) => {
                    tracing::info!("Add parameter requested for method {}", index);
                }
            }
        }).detach();

        self.method_editors.push(editor);

        self.notify_modified();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn remove_method(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.method_editors.len() {
            self.asset.write().methods.remove(index);
            self.method_editors.remove(index);

            // Update indices for remaining method editors
            for (i, editor) in self.method_editors.iter().enumerate() {
                editor.update(cx, |ed, _cx| {
                    ed.index = i;
                });
            }

            self.notify_modified();
            cx.emit(PanelEvent::LayoutChanged);
            cx.notify();
        }
    }

    fn notify_modified(&self) {
        if let Some(callback) = self.on_modified.lock().as_ref() {
            callback();
        }
    }

    pub fn set_on_modified<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_modified.lock() = Some(Box::new(callback));
    }
}

impl EventEmitter<PanelEvent> for MethodsPanel {}

impl Render for MethodsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let methods_count = self.asset.read().methods.len();

        v_flex()
            .size_full()
            .gap_3()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .p_3()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(format!("Methods ({})", methods_count))
                    )
                    .child(
                        Button::new("add-method")
                            .label("Add")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_method(window, cx);
                            }))
                    )
            )
            .child(
                v_flex()
                    .id("trait-methods-content")
                    .px_3()
                    .gap_2()
                    .flex_1()
                    .overflow_scroll()
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
                                        .text_size(rems(2.0))
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
                                        .child("Click 'Add' to create a method")
                                )
                        )
                    })
            )
    }
}

impl Focusable for MethodsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MethodsPanel {
    fn panel_name(&self) -> &'static str {
        "Methods"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        "Methods".into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

/// Code Preview Panel - Display generated Rust code
pub struct CodePreviewPanel {
    asset: Arc<parking_lot::RwLock<TraitAsset>>,
    code_input: Entity<InputState>,
    focus_handle: FocusHandle,
    needs_update: Arc<parking_lot::Mutex<bool>>,
}

impl CodePreviewPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<TraitAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let code_input = cx.new(|cx| {
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

        let panel = Self {
            asset,
            code_input,
            focus_handle: cx.focus_handle(),
            needs_update: Arc::new(parking_lot::Mutex::new(true)),
        };

        // Generate initial code
        panel.update_code(window, cx);

        panel
    }

    fn update_code(&self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.generate_rust_code();
        self.code_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &code, window, cx);
        });
        *self.needs_update.lock() = false;
    }

    fn generate_rust_code(&self) -> String {
        let asset = self.asset.read();
        let mut code = String::new();

        // Add documentation if present
        if let Some(desc) = &asset.description {
            code.push_str(&format!("/// {}\n", desc));
        }

        // Trait declaration
        code.push_str(&format!("pub trait {} {{\n", asset.name));

        // Methods
        for method in &asset.methods {
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
                code.push_str(&Self::type_ref_to_string(&param.type_ref));
            }

            code.push_str(")");

            // Return type
            let return_type = Self::type_ref_to_string(&method.signature.return_type);
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

    fn type_ref_to_string(type_ref: &TypeRef) -> String {
        match type_ref {
            TypeRef::Primitive { name } => name.clone(),
            TypeRef::Path { path } => path.clone(),
            TypeRef::AliasRef { alias } => alias.clone(),
        }
    }

    pub fn mark_needs_update(&self) {
        *self.needs_update.lock() = true;
    }
}

impl EventEmitter<PanelEvent> for CodePreviewPanel {}

impl Render for CodePreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update code if needed
        if *self.needs_update.lock() {
            self.update_code(window, cx);
        }

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .h(px(40.0))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Generated Code")
                    )
            )
            .child(
                TextInput::new(&self.code_input)
                    .w_full()
                    .flex_1()
            )
    }
}

impl Focusable for CodePreviewPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CodePreviewPanel {
    fn panel_name(&self) -> &'static str {
        "Code Preview"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        "Code Preview".into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}
