use gpui::{*, prelude::FluentBuilder, actions, InteractiveElement as _, StatefulInteractiveElement as _};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, IconName, Disableable,
    dock::{Panel, PanelEvent},
    button::{Button, ButtonVariants},
    divider::Divider,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, ResizableState},
};
use ui_types_common::{StructAsset, StructField, TypeRef, Visibility, TypeKind};
use std::path::PathBuf;
use crate::field_editor::{FieldEditorView, FieldEditorEvent};

actions!(struct_editor, [
    Save,
    AddField,
    TogglePreview,
]);

#[derive(Clone, Debug)]
pub enum StructEditorEvent {
    Modified,
    Saved,
}

pub struct StructEditor {
    file_path: Option<PathBuf>,
    asset: StructAsset,
    error_message: Option<String>,
    focus_handle: FocusHandle,

    // Input states for properties
    name_input: Entity<InputState>,
    display_name_input: Entity<InputState>,
    description_input: Entity<InputState>,

    // Field editors
    field_editors: Vec<Entity<FieldEditorView>>,

    // Code preview
    code_preview_input: Entity<InputState>,
    show_preview: bool,
    preview_needs_update: bool,

    // Resizable state
    horizontal_resizable_state: Entity<ResizableState>,

    // Modified flag
    modified: bool,
}

impl StructEditor {
    pub fn new_with_file(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the struct data
        let (asset, error_message) = match std::fs::read_to_string(&file_path) {
            Ok(json_content) => {
                match serde_json::from_str::<StructAsset>(&json_content) {
                    Ok(asset) => (asset, None),
                    Err(e) => (
                        Self::create_empty_asset(),
                        Some(format!("Failed to parse struct: {}", e))
                    ),
                }
            }
            Err(_) => (Self::create_empty_asset(), None),
        };

        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("StructName"));
        let display_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Display Name"));
        let description_input = cx.new(|cx| InputState::new(window, cx).placeholder("Struct description..."));

        let horizontal_resizable_state = ResizableState::new(cx);

        // Create code preview input with syntax highlighting
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

        // Create field editors
        let mut field_editors = Vec::new();
        for (index, field) in asset.fields.iter().enumerate() {
            let editor = cx.new(|cx| FieldEditorView::new(field.clone(), index, window, cx));

            // Subscribe to field editor events
            cx.subscribe(&editor, |this: &mut Self, _, event: &FieldEditorEvent, cx| {
                match event {
                    FieldEditorEvent::FieldChanged(index, field) => {
                        if *index < this.asset.fields.len() {
                            this.asset.fields[*index] = field.clone();
                            this.modified = true;
                            this.preview_needs_update = true;
                            cx.notify();
                        }
                    }
                    FieldEditorEvent::RemoveRequested(index) => {
                        // Handle removal - mark for removal
                        if *index < this.asset.fields.len() {
                            this.asset.fields.remove(*index);
                            this.field_editors.remove(*index);

                            // Update indices for remaining field editors
                            for (i, editor) in this.field_editors.iter().enumerate() {
                                editor.update(cx, |ed, _cx| {
                                    ed.index = i;
                                });
                            }

                            this.modified = true;
                            this.preview_needs_update = true;
                            cx.emit(StructEditorEvent::Modified);
                            cx.notify();
                        }
                    }
                    FieldEditorEvent::TypePickerRequested(index) => {
                        // TODO: Open type picker dialog
                        eprintln!("Type picker requested for field {}", index);
                    }
                }
            }).detach();

            field_editors.push(editor);
        }

        // Subscribe to input changes for the main properties
        cx.subscribe_in(&name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    this.preview_needs_update = true;
                    cx.emit(StructEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        cx.subscribe_in(&display_name_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    cx.emit(StructEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        cx.subscribe_in(&description_input, window, |this, _state, event: &ui::input::InputEvent, _window, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.modified = true;
                    this.preview_needs_update = true;
                    cx.emit(StructEditorEvent::Modified);
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

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
            horizontal_resizable_state,
            modified: false,
            field_editors,
        };

        // Initialize preview
        editor.update_preview(window, cx);
        editor.preview_needs_update = false;

        editor
    }

    fn create_empty_asset() -> StructAsset {
        StructAsset {
            schema_version: 1,
            type_kind: TypeKind::Struct,
            name: String::from("NewStruct"),
            display_name: String::from("New Struct"),
            description: None,
            fields: Vec::new(),
            visibility: Visibility::Public,
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
                    if let Err(e) = std::fs::write(file_path, json) {
                        self.error_message = Some(format!("Failed to save: {}", e));
                    } else {
                        self.error_message = None;
                        self.modified = false;
                        eprintln!("✅ Saved struct to {:?}", file_path);

                        // Update preview
                        self.update_preview(window, cx);
                        cx.emit(StructEditorEvent::Saved);
                    }
                }
                Err(e) => {
                    self.error_message = Some(format!("Failed to serialize: {}", e));
                }
            }
        }
        cx.notify();
    }

    fn add_field(&mut self, _: &AddField, window: &mut Window, cx: &mut Context<Self>) {
        let new_field = StructField {
            name: format!("field_{}", self.asset.fields.len()),
            type_ref: TypeRef::Primitive { name: "String".to_string() },
            visibility: Visibility::Public,
            doc: None,
        };

        let index = self.asset.fields.len();
        self.asset.fields.push(new_field.clone());

        // Create field editor for the new field
        let editor = cx.new(|cx| FieldEditorView::new(new_field, index, window, cx));

        // Subscribe to field editor events
        cx.subscribe(&editor, |this: &mut Self, _, event: &FieldEditorEvent, cx| {
            match event {
                FieldEditorEvent::FieldChanged(index, field) => {
                    if *index < this.asset.fields.len() {
                        this.asset.fields[*index] = field.clone();
                        this.modified = true;
                        this.preview_needs_update = true;
                        cx.notify();
                    }
                }
                FieldEditorEvent::RemoveRequested(index) => {
                    // Handle removal directly
                    if *index < this.asset.fields.len() {
                        this.asset.fields.remove(*index);
                        this.field_editors.remove(*index);

                        // Update indices for remaining field editors
                        for (i, editor) in this.field_editors.iter().enumerate() {
                            editor.update(cx, |ed, _cx| {
                                ed.index = i;
                            });
                        }

                        this.modified = true;
                        this.preview_needs_update = true;
                        cx.emit(StructEditorEvent::Modified);
                        cx.notify();
                    }
                }
                FieldEditorEvent::TypePickerRequested(index) => {
                    eprintln!("Type picker requested for field {}", index);
                }
            }
        }).detach();

        self.field_editors.push(editor);

        self.modified = true;
        self.preview_needs_update = true;
        cx.emit(StructEditorEvent::Modified);
        cx.notify();
    }

    fn remove_field(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.asset.fields.len() {
            self.asset.fields.remove(index);
            self.field_editors.remove(index);

            // Update indices for remaining field editors
            for (i, editor) in self.field_editors.iter().enumerate() {
                editor.update(cx, |ed, cx| {
                    ed.index = i;
                    if i < self.asset.fields.len() {
                        ed.update_field(self.asset.fields[i].clone(), cx);
                    }
                });
            }

            self.modified = true;
            self.preview_needs_update = true;
            cx.emit(StructEditorEvent::Modified);
            cx.notify();
        }
    }

    fn toggle_preview(&mut self, _: &TogglePreview, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_preview = !self.show_preview;
        cx.notify();
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

        // Add derives
        code.push_str("#[derive(Debug, Clone)]\n");

        // Visibility
        let visibility = match self.asset.visibility {
            Visibility::Public => "pub ",
            Visibility::Private => "",
            Visibility::Crate => "pub(crate) ",
            Visibility::Super => "pub(super) ",
        };

        // Struct declaration
        code.push_str(&format!("{}struct {} {{\n", visibility, self.asset.name));

        // Fields
        for field in &self.asset.fields {
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
                    .icon(IconName::FloppyDisk)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save(&Save, window, cx);
                    }))
                    .when(!self.modified, |this| this.disabled(true))
            )
            .child(
                Button::new("add-field-btn")
                    .label("Add Field")
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_field(&AddField, window, cx);
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

    fn render_properties_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                                    .when(self.asset.visibility == Visibility::Public, |this| this.primary())
                                    .label("Public")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.asset.visibility = Visibility::Public;
                                        this.modified = true;
                                        this.preview_needs_update = true;
                                        cx.emit(StructEditorEvent::Modified);
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("visibility-private")
                                    .when(self.asset.visibility == Visibility::Private, |this| this.primary())
                                    .label("Private")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.asset.visibility = Visibility::Private;
                                        this.modified = true;
                                        this.preview_needs_update = true;
                                        cx.emit(StructEditorEvent::Modified);
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("visibility-crate")
                                    .when(self.asset.visibility == Visibility::Crate, |this| this.primary())
                                    .label("Crate")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.asset.visibility = Visibility::Crate;
                                        this.modified = true;
                                        this.preview_needs_update = true;
                                        cx.emit(StructEditorEvent::Modified);
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("visibility-super")
                                    .when(self.asset.visibility == Visibility::Super, |this| this.primary())
                                    .label("Super")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.asset.visibility = Visibility::Super;
                                        this.modified = true;
                                        this.preview_needs_update = true;
                                        cx.emit(StructEditorEvent::Modified);
                                        cx.notify();
                                    }))
                            )
                    )
            )
    }

    fn render_fields_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
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
                            .child(format!("Fields ({})", self.asset.fields.len()))
                    )
                    .child(
                        Button::new("add-field-inline")
                            .label("Add")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_field(&AddField, window, cx);
                            }))
                    )
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_3()
                    .scrollable(gpui::Axis::Vertical)
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
                                        .text_3xl()
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
                                        .child("Click 'Add Field' to get started")
                                )
                        )
                    })
            )
    }

    fn render_code_preview(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
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
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        TextInput::new(&self.code_preview_input)
                            .disabled(true)
                    )
            )
    }
}

impl EventEmitter<StructEditorEvent> for StructEditor {}
impl EventEmitter<PanelEvent> for StructEditor {}

impl Render for StructEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update preview if needed
        if self.preview_needs_update {
            self.update_preview(window, cx);
            self.preview_needs_update = false;
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .key_context("StructEditor")
            .on_action(cx.listener(|this, action, window, cx| {
                this.save(action, window, cx);
            }))
            .on_action(cx.listener(|this, action, window, cx| {
                this.add_field(action, window, cx);
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
                                    .child("📦")
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
                            .p_4()
                            .gap_4()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .scrollable(gpui::Axis::Vertical)
                            .child(
                                div()
                                    .text_base()
                                    .font_bold()
                                    .text_color(cx.theme().foreground)
                                    .child("Properties")
                            )
                            .child(Divider::horizontal())
                            .child(self.render_properties_panel(window, cx))
                    )
                    .child(
                        // Center panel - Fields
                        v_flex()
                            .flex_1()
                            .h_full()
                            .p_4()
                            .overflow_hidden()
                            .when(self.show_preview, |this| {
                                this.border_r_1()
                                    .border_color(cx.theme().border)
                            })
                            .child(self.render_fields_panel(window, cx))
                    )
                    .when(self.show_preview, |this| {
                        this.child(
                            // Right panel - Code Preview
                            v_flex()
                                .w(px(500.0))
                                .h_full()
                                .p_4()
                                .gap_3()
                                .overflow_hidden()
                                .child(self.render_code_preview(window, cx))
                        )
                    })
            )
    }
}

impl Focusable for StructEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for StructEditor {
    fn panel_name(&self) -> &'static str {
        "Struct Editor"
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
