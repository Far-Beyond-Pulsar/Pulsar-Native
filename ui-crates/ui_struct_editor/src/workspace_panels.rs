/// Workspace panel wrappers for Struct Editor dock system integration
use gpui::{*, prelude::FluentBuilder};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, IconName,
    dock::{Panel, PanelEvent},
    divider::Divider,
    button::{Button, ButtonVariants},
    input::{InputState, TextInput},
};
use ui_types_common::{StructAsset, Visibility};
use std::sync::Arc;
use crate::field_editor::FieldEditorView;

/// Properties Panel - Edit struct metadata (name, display name, description, visibility)
pub struct PropertiesPanel {
    asset: Arc<parking_lot::RwLock<StructAsset>>,
    name_input: Entity<InputState>,
    display_name_input: Entity<InputState>,
    description_input: Entity<InputState>,
    focus_handle: FocusHandle,
    on_modified: Arc<parking_lot::Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

impl PropertiesPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<StructAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("StructName"));
        let display_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Display Name"));
        let description_input = cx.new(|cx| InputState::new(window, cx).placeholder("Struct description..."));

        // Initialize inputs with current asset values
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

        Self {
            asset,
            name_input,
            display_name_input,
            description_input,
            focus_handle: cx.focus_handle(),
            on_modified: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    pub fn set_on_modified<F>(&mut self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_modified.lock() = Some(Box::new(callback));
    }

    fn notify_modified(&self) {
        if let Some(ref callback) = *self.on_modified.lock() {
            callback();
        }
    }

    fn sync_inputs_to_asset(&self, cx: &App) {
        let name = self.name_input.read(cx).text().to_string();
        let display_name = self.display_name_input.read(cx).text().to_string();
        let description = self.description_input.read(cx).text().to_string();

        let mut asset = self.asset.write();
        asset.name = name;
        asset.display_name = display_name;
        asset.description = Some(description);
    }

    fn set_visibility(&mut self, visibility: Visibility, cx: &mut Context<Self>) {
        self.sync_inputs_to_asset(cx);
        self.asset.write().visibility = visibility;
        self.notify_modified();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanel {}

impl Render for PropertiesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let asset = self.asset.read();

        v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .bg(cx.theme().sidebar)
            // Name
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
            // Display Name
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
            // Description
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
            // Visibility
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Visibility")
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("visibility-public")
                                    .when(asset.visibility == Visibility::Public, |this| this.primary())
                                    .label("Public")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_visibility(Visibility::Public, cx);
                                    }))
                            )
                            .child(
                                Button::new("visibility-private")
                                    .when(asset.visibility == Visibility::Private, |this| this.primary())
                                    .label("Private")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_visibility(Visibility::Private, cx);
                                    }))
                            )
                            .child(
                                Button::new("visibility-crate")
                                    .when(asset.visibility == Visibility::Crate, |this| this.primary())
                                    .label("Crate")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_visibility(Visibility::Crate, cx);
                                    }))
                            )
                            .child(
                                Button::new("visibility-super")
                                    .when(asset.visibility == Visibility::Super, |this| this.primary())
                                    .label("Super")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_visibility(Visibility::Super, cx);
                                    }))
                            )
                    )
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
        "struct_properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Properties".into_any_element()
    }
}

/// Fields Panel - Manage struct fields (add, remove, edit)
pub struct FieldsPanel {
    asset: Arc<parking_lot::RwLock<StructAsset>>,
    field_editors: Vec<Entity<FieldEditorView>>,
    focus_handle: FocusHandle,
    on_modified: Arc<parking_lot::Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

impl FieldsPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<StructAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let asset_read = asset.read();
        let mut field_editors = Vec::new();

        for (index, field) in asset_read.fields.iter().enumerate() {
            let editor = cx.new(|cx| FieldEditorView::new(field.clone(), index, window, cx));

            // Subscribe to field editor events
            cx.subscribe(&editor, |this: &mut Self, _, event: &crate::field_editor::FieldEditorEvent, cx| {
                match event {
                    crate::field_editor::FieldEditorEvent::FieldChanged(index, field) => {
                        let mut asset = this.asset.write();
                        if *index < asset.fields.len() {
                            asset.fields[*index] = field.clone();
                            drop(asset);
                            this.notify_modified();
                            cx.emit(PanelEvent::LayoutChanged);
                            cx.notify();
                        }
                    }
                    crate::field_editor::FieldEditorEvent::RemoveRequested(index) => {
                        this.remove_field(*index, cx);
                    }
                    crate::field_editor::FieldEditorEvent::TypePickerRequested(index) => {
                        tracing::info!("Type picker requested for field {}", index);
                    }
                }
            }).detach();

            field_editors.push(editor);
        }
        drop(asset_read);

        Self {
            asset,
            field_editors,
            focus_handle: cx.focus_handle(),
            on_modified: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    pub fn set_on_modified<F>(&mut self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_modified.lock() = Some(Box::new(callback));
    }

    fn notify_modified(&self) {
        if let Some(ref callback) = *self.on_modified.lock() {
            callback();
        }
    }

    fn add_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use ui_types_common::{StructField, TypeRef};

        let new_field = StructField {
            name: format!("field_{}", self.field_editors.len()),
            type_ref: TypeRef::Primitive { name: "String".to_string() },
            visibility: Visibility::Public,
            doc: None,
        };

        let index = self.field_editors.len();
        let editor = cx.new(|cx| FieldEditorView::new(new_field.clone(), index, window, cx));

        // Subscribe to the new editor's events
        cx.subscribe(&editor, |this: &mut Self, _, event: &crate::field_editor::FieldEditorEvent, cx| {
            match event {
                crate::field_editor::FieldEditorEvent::FieldChanged(index, field) => {
                    let mut asset = this.asset.write();
                    if *index < asset.fields.len() {
                        asset.fields[*index] = field.clone();
                        drop(asset);
                        this.notify_modified();
                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                }
                crate::field_editor::FieldEditorEvent::RemoveRequested(index) => {
                    this.remove_field(*index, cx);
                }
                crate::field_editor::FieldEditorEvent::TypePickerRequested(index) => {
                    tracing::info!("Type picker requested for field {}", index);
                }
            }
        }).detach();

        self.field_editors.push(editor);
        self.asset.write().fields.push(new_field);
        self.notify_modified();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn remove_field(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.field_editors.len() {
            // Remove from asset
            self.asset.write().fields.remove(index);

            // Remove editor
            self.field_editors.remove(index);

            // Update indices for remaining editors
            for (i, editor) in self.field_editors.iter().enumerate() {
                editor.update(cx, |ed, _cx| {
                    ed.index = i;
                });
            }

            self.notify_modified();
            cx.emit(PanelEvent::LayoutChanged);
            cx.notify();
        }
    }
}

impl EventEmitter<PanelEvent> for FieldsPanel {}

impl Render for FieldsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let asset = self.asset.read();

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
                            .child(format!("Fields ({})", asset.fields.len()))
                    )
                    .child(
                        Button::new("add-field")
                            .label("Add")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_field(window, cx);
                            }))
                    )
            )
            .child(
                v_flex()
                    .id("struct-fields-content")
                    .px_3()
                    .gap_2()
                    .flex_1()
                    .overflow_scroll()
                    .children(
                        self.field_editors.iter().map(|editor| editor.clone())
                    )
                    .when(self.field_editors.is_empty(), |this| {
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
                                        .child("📦")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No fields yet")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("Click 'Add' to create a field")
                                )
                        )
                    })
            )
    }
}

impl Focusable for FieldsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FieldsPanel {
    fn panel_name(&self) -> &'static str {
        "struct_fields"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Fields".into_any_element()
    }
}

/// Code Preview Panel - Display generated Rust code with syntax highlighting
pub struct CodePreviewPanel {
    asset: Arc<parking_lot::RwLock<StructAsset>>,
    code_input: Entity<InputState>,
    focus_handle: FocusHandle,
    needs_update: Arc<parking_lot::Mutex<bool>>,
}

impl CodePreviewPanel {
    pub fn new(
        asset: Arc<parking_lot::RwLock<StructAsset>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use ui::input::TabSize;

        let code_input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("rust")
                .line_number(true)
                .minimap(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
        });

        Self {
            asset,
            code_input,
            focus_handle: cx.focus_handle(),
            needs_update: Arc::new(parking_lot::Mutex::new(true)),
        }
    }

    pub fn request_update(&self) {
        *self.needs_update.lock() = true;
    }

    fn update_code_preview(&self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.generate_rust_code();
        self.code_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, &code, window, cx);
        });
        *self.needs_update.lock() = false;
    }

    fn generate_rust_code(&self) -> String {
        let asset = self.asset.read();
        let mut code = String::new();

        if let Some(desc) = &asset.description {
            code.push_str(&format!("/// {}\n", desc));
        }
        code.push_str("#[derive(Debug, Clone)]\n");

        let visibility = match asset.visibility {
            Visibility::Public => "pub ",
            Visibility::Private => "",
            Visibility::Crate => "pub(crate) ",
            Visibility::Super => "pub(super) ",
        };

        code.push_str(&format!("{}struct {} {{\n", visibility, asset.name));

        for field in &asset.fields {
            if let Some(doc) = &field.doc {
                code.push_str(&format!("    /// {}\n", doc));
            }
            let field_visibility = match field.visibility {
                Visibility::Public => "pub ",
                Visibility::Private => "",
                Visibility::Crate => "pub(crate) ",
                Visibility::Super => "pub(super) ",
            };
            let type_str = self.type_ref_to_string(&field.type_ref);
            code.push_str(&format!("    {}{}: {},\n", field_visibility, field.name, type_str));
        }

        code.push_str("}\n");
        code
    }

    fn type_ref_to_string(&self, type_ref: &ui_types_common::TypeRef) -> String {
        match type_ref {
            ui_types_common::TypeRef::Primitive { name } => name.clone(),
            ui_types_common::TypeRef::Path { path } => path.clone(),
            ui_types_common::TypeRef::AliasRef { alias } => alias.clone(),
        }
    }
}

impl EventEmitter<PanelEvent> for CodePreviewPanel {}

impl Render for CodePreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if *self.needs_update.lock() {
            self.update_code_preview(window, cx);
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
        "struct_code_preview"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Code Preview".into_any_element()
    }
}
