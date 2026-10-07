use std::path::PathBuf;

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent, PanelState},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    v_flex, ActiveTheme, Disableable,
};
use ui_types_common::{
    validate_name, MethodParam, MethodSignature, TraitAsset, TraitMethod, TypeKind, TypeRef,
};

struct ParameterDraft {
    name: Entity<InputState>,
    type_ref: Entity<InputState>,
}

struct MethodDraft {
    name: Entity<InputState>,
    return_type: Entity<InputState>,
    doc: Entity<InputState>,
    default_body: Entity<InputState>,
    params: Vec<ParameterDraft>,
}

impl MethodDraft {
    fn from_asset(
        method: &TraitMethod,
        window: &mut Window,
        cx: &mut Context<TraitEditor>,
    ) -> Self {
        Self {
            name: field_input(window, cx, &method.name, "method_name"),
            return_type: field_input(
                window,
                cx,
                &format_type_ref(&method.signature.return_type),
                "return type (default: ())",
            ),
            doc: multiline_input(
                window,
                cx,
                method.doc.as_deref().unwrap_or_default(),
                "Method documentation",
            ),
            default_body: multiline_input(
                window,
                cx,
                method.default_body.as_deref().unwrap_or_default(),
                "Optional default body",
            ),
            params: method
                .signature
                .params
                .iter()
                .map(|param| ParameterDraft::from_asset(param, window, cx))
                .collect(),
        }
    }
}

impl ParameterDraft {
    fn from_asset(param: &MethodParam, window: &mut Window, cx: &mut Context<TraitEditor>) -> Self {
        Self {
            name: field_input(window, cx, &param.name, "parameter"),
            type_ref: field_input(
                window,
                cx,
                &format_type_ref(&param.type_ref),
                "type (primitive, path, alias:Name)",
            ),
        }
    }
}

pub struct TraitEditor {
    file_path: PathBuf,
    focus_handle: FocusHandle,
    name: Entity<InputState>,
    display_name: Entity<InputState>,
    description: Entity<InputState>,
    methods: Vec<MethodDraft>,
    dirty: bool,
    load_error: Option<String>,
    save_status: Option<Result<(), String>>,
}

impl TraitEditor {
    pub fn open(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (asset, load_error) = match crate::load_trait(&file_path) {
            Ok(asset) => (asset, None),
            Err(error) => (empty_asset(file_path.as_path()), Some(error.to_string())),
        };
        Self::from_asset(file_path, asset, load_error, window, cx)
    }

    fn from_asset(
        file_path: PathBuf,
        asset: TraitAsset,
        load_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            file_path,
            focus_handle: cx.focus_handle(),
            name: field_input(window, cx, &asset.name, "Rust type name"),
            display_name: field_input(window, cx, &asset.display_name, "Display name"),
            description: multiline_input(
                window,
                cx,
                asset.description.as_deref().unwrap_or_default(),
                "Describe the contract this trait provides",
            ),
            methods: asset
                .methods
                .iter()
                .map(|method| MethodDraft::from_asset(method, window, cx))
                .collect(),
            dirty: false,
            load_error,
            save_status: None,
        }
    }

    fn add_method(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let method = TraitMethod {
            name: "new_method".to_owned(),
            signature: MethodSignature {
                params: Vec::new(),
                return_type: TypeRef::primitive("()"),
            },
            default_body: None,
            doc: None,
        };
        self.methods
            .push(MethodDraft::from_asset(&method, window, cx));
        self.mark_dirty(cx);
    }

    fn add_parameter(&mut self, method_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(method) = self.methods.get_mut(method_index) {
            let parameter = MethodParam {
                name: "value".to_owned(),
                type_ref: TypeRef::primitive("String"),
            };
            method
                .params
                .push(ParameterDraft::from_asset(&parameter, window, cx));
            self.mark_dirty(cx);
        }
    }

    fn remove_method(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.methods.len() {
            self.methods.remove(index);
            self.mark_dirty(cx);
        }
    }

    fn remove_parameter(
        &mut self,
        method_index: usize,
        parameter_index: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(method) = self.methods.get_mut(method_index) {
            if parameter_index < method.params.len() {
                method.params.remove(parameter_index);
                self.mark_dirty(cx);
            }
        }
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.save_status = None;
        cx.notify();
    }

    fn build_asset(&self, cx: &App) -> Result<TraitAsset, String> {
        let name = input_value(&self.name, cx).trim().to_owned();
        let display_name = input_value(&self.display_name, cx).trim().to_owned();
        let description = input_value(&self.description, cx).trim().to_owned();
        validate_name(&name).map_err(|error| error.to_string())?;
        if display_name.is_empty() {
            return Err("Display name cannot be empty".to_owned());
        }

        let mut method_names = std::collections::HashSet::new();
        let mut methods = Vec::with_capacity(self.methods.len());
        for (method_index, method) in self.methods.iter().enumerate() {
            let method_name = input_value(&method.name, cx).trim().to_owned();
            validate_name(&method_name)
                .map_err(|error| format!("Method {}: {}", method_index + 1, error))?;
            if !method_names.insert(method_name.clone()) {
                return Err(format!("Method name '{}' is duplicated", method_name));
            }

            let mut parameter_names = std::collections::HashSet::new();
            let mut params = Vec::with_capacity(method.params.len());
            for (param_index, parameter) in method.params.iter().enumerate() {
                let parameter_name = input_value(&parameter.name, cx).trim().to_owned();
                validate_name(&parameter_name).map_err(|error| {
                    format!(
                        "Method '{}', parameter {}: {}",
                        method_name,
                        param_index + 1,
                        error
                    )
                })?;
                if !parameter_names.insert(parameter_name.clone()) {
                    return Err(format!(
                        "Parameter '{}' is duplicated in method '{}'",
                        parameter_name, method_name
                    ));
                }
                let type_ref =
                    parse_type_ref(&input_value(&parameter.type_ref, cx)).map_err(|error| {
                        format!(
                            "Method '{}', parameter '{}': {}",
                            method_name, parameter_name, error
                        )
                    })?;
                params.push(MethodParam {
                    name: parameter_name,
                    type_ref,
                });
            }

            let return_type = parse_type_ref(&input_value(&method.return_type, cx))
                .map_err(|error| format!("Method '{}': {}", method_name, error))?;
            let doc = non_empty(input_value(&method.doc, cx));
            let default_body = non_empty(input_value(&method.default_body, cx));
            methods.push(TraitMethod {
                name: method_name,
                signature: MethodSignature {
                    params,
                    return_type,
                },
                default_body,
                doc,
            });
        }

        let asset = TraitAsset {
            schema_version: 1,
            type_kind: TypeKind::Trait,
            name,
            display_name,
            description: non_empty(description),
            methods,
            meta: serde_json::Value::Object(Default::default()),
        };
        let index = load_type_index_for_asset(&self.file_path)?;
        ui_types_common::validate_trait(&asset, &index)
            .map_err(|error| format!("Trait definition is invalid: {error}"))?;
        Ok(asset)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = &self.load_error {
            self.save_status = Some(Err(format!(
                "The source file failed to load. Choose ‘Start with an empty trait’ before saving: {error}"
            )));
            cx.notify();
            return;
        }
        let asset = match self.build_asset(cx) {
            Ok(asset) => asset,
            Err(error) => {
                self.save_status = Some(Err(error));
                cx.notify();
                return;
            }
        };

        let result = serde_json::to_vec_pretty(&asset)
            .map_err(|error| format!("Could not serialize trait: {error}"))
            .and_then(|bytes| {
                engine_fs::virtual_fs::write_file(&self.file_path, &bytes)
                    .map_err(|error| format!("Could not save trait: {error}"))
            });
        if result.is_ok() {
            self.dirty = false;
            self.load_error = None;
        }
        self.save_status = Some(result);
        cx.notify();
    }

    fn reset_to_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asset = empty_asset(&self.file_path);
        let file_path = self.file_path.clone();
        *self = Self::from_asset(file_path, asset, None, window, cx);
        self.dirty = true;
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for TraitEditor {}

impl Focusable for TraitEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TraitEditor {
    fn panel_name(&self) -> &'static str {
        "trait-editor"
    }

    fn panel_file_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.file_path.clone())
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        self.file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Trait Editor")
            .to_owned()
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState {
            panel_name: self.panel_name().to_owned(),
            ..Default::default()
        }
    }
}

impl Render for TraitEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let save_button = Button::new("save-trait")
            .label(if self.dirty { "Save Changes" } else { "Save" })
            .disabled(self.load_error.is_some())
            .primary()
            .on_click(cx.listener(|this, _, _, cx| this.save(cx)));

        let load_error = self.load_error.clone();
        let save_status = self.save_status.clone();
        let editor = v_flex()
            .size_full()
            .gap_4()
            .p_4()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Trait Definition"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(self.file_path.to_string_lossy().to_string()),
                            ),
                    )
                    .child(save_button),
            )
            .when_some(load_error, |element, error| {
                element.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .bg(theme.danger.opacity(0.12))
                        .child(div().text_sm().text_color(theme.danger).child(format!("Could not load trait: {error}")))
                        .child(
                            Button::new("reset-trait")
                                .label("Start with an empty trait")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reset_to_empty(window, cx)
                                })),
                        ),
                )
            })
            .child(section("Identity"))
            .child(labeled_input("Rust name", TextInput::new(&self.name).w_full()))
            .child(labeled_input(
                "Display name",
                TextInput::new(&self.display_name).w_full(),
            ))
            .child(labeled_input(
                "Description",
                TextInput::new(&self.description).w_full(),
            ))
            .child(section("Required method contracts"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Add required methods and parameters. Use alias:TypeName to reference a project type alias; other type text is stored as a Rust path."),
            )
            .children(self.methods.iter().enumerate().map(|(method_index, method)| {
                let remove = Button::new(format!("remove-method-{method_index}"))
                    .label("Remove")
                    .danger()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_method(method_index, cx)
                    }));
                let add_parameter = Button::new(format!("add-param-{method_index}"))
                    .label("Add parameter")
                    .ghost()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_parameter(method_index, window, cx)
                    }));

                v_flex()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.sidebar)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Method {}", method_index + 1)))
                            .child(remove),
                    )
                    .child(labeled_input("Name", TextInput::new(&method.name).w_full()))
                    .child(labeled_input(
                        "Return type",
                        TextInput::new(&method.return_type).w_full(),
                    ))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Parameters"))
                            .children(method.params.iter().enumerate().map(|(parameter_index, parameter)| {
                                let remove_parameter = Button::new(format!("remove-param-{method_index}-{parameter_index}"))
                                    .label("×")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_parameter(method_index, parameter_index, cx)
                                    }));
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(TextInput::new(&parameter.name).flex_1())
                                    .child(TextInput::new(&parameter.type_ref).flex_1())
                                    .child(remove_parameter)
                            }))
                            .child(add_parameter),
                    )
                    .child(labeled_input(
                        "Documentation",
                        TextInput::new(&method.doc).w_full(),
                    ))
                    .child(labeled_input(
                        "Default body (optional)",
                        TextInput::new(&method.default_body).w_full(),
                    ))
            }))
            .child(
                Button::new("add-method")
                    .label("Add required method")
                    .on_click(cx.listener(|this, _, window, cx| this.add_method(window, cx))),
            )
            .when_some(save_status, |element, result| {
                let (message, color) = match result {
                    Ok(()) => ("Trait saved successfully".to_owned(), theme.success),
                    Err(error) => (error, theme.danger),
                };
                element.child(div().text_sm().text_color(color).child(message))
            })
            .when(self.dirty, |element| {
                element.child(
                    div()
                        .text_sm()
                        .text_color(theme.warning)
                        .child("Unsaved changes"),
                )
            });

        div()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(editor)
    }
}

fn field_input(
    window: &mut Window,
    cx: &mut Context<TraitEditor>,
    value: &str,
    placeholder: &str,
) -> Entity<InputState> {
    create_input(window, cx, value, placeholder, false)
}

fn multiline_input(
    window: &mut Window,
    cx: &mut Context<TraitEditor>,
    value: &str,
    placeholder: &str,
) -> Entity<InputState> {
    create_input(window, cx, value, placeholder, true)
}

fn create_input(
    window: &mut Window,
    cx: &mut Context<TraitEditor>,
    value: &str,
    placeholder: &str,
    multiline: bool,
) -> Entity<InputState> {
    let input = cx.new(|cx| {
        let state = InputState::new(window, cx)
            .default_value(value.to_owned())
            .placeholder(placeholder);
        if multiline {
            state.multi_line().auto_grow(2, 6)
        } else {
            state
        }
    });
    cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
        if matches!(
            event,
            InputEvent::Change | InputEvent::Blur | InputEvent::PressEnter { .. }
        ) {
            this.mark_dirty(cx);
        }
    })
    .detach();
    input
}

fn input_value(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().to_string()
}

fn empty_asset(path: &std::path::Path) -> TraitAsset {
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("NewTrait")
        .strip_suffix(".trait")
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("NewTrait")
        });
    TraitAsset {
        schema_version: 1,
        type_kind: TypeKind::Trait,
        name: sanitize_type_name(name),
        display_name: name.to_owned(),
        description: None,
        methods: Vec::new(),
        meta: serde_json::Value::Object(Default::default()),
    }
}

fn sanitize_type_name(value: &str) -> String {
    let mut name = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if index == 0 && character.is_ascii_digit() {
                name.push('_');
            }
            name.push(character);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name = name.trim_matches('_');
    if name.is_empty() {
        "NewTrait".to_owned()
    } else {
        name.to_owned()
    }
}

fn parse_type_ref(value: &str) -> Result<TypeRef, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Type cannot be empty".to_owned());
    }
    if let Some(alias) = value.strip_prefix("alias:") {
        let alias = alias.trim();
        validate_name(alias).map_err(|error| error.to_string())?;
        return Ok(TypeRef::alias(alias));
    }
    let primitive = ui_types_common::PRIMITIVES.contains(&value);
    if primitive {
        Ok(TypeRef::primitive(value))
    } else {
        Ok(TypeRef::path(value))
    }
}

fn format_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => name.clone(),
        TypeRef::Path { path } => path.clone(),
        TypeRef::AliasRef { alias } => format!("alias:{alias}"),
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn load_type_index_for_asset(
    file_path: &std::path::Path,
) -> Result<ui_types_common::TypeIndex, String> {
    let project_root = file_path
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .ok_or_else(|| {
            "Trait asset must be inside the project's types/traits directory".to_owned()
        })?;
    let index_path = project_root.join("type-index").join("index.json");
    match engine_fs::virtual_fs::exists(&index_path) {
        Ok(false) => Ok(ui_types_common::TypeIndex::default()),
        Ok(true) => {
            let bytes = engine_fs::virtual_fs::read_file(&index_path)
                .map_err(|error| format!("Could not load type index: {error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("Type index is malformed: {error}"))
        }
        Err(error) => Err(format!("Could not inspect type index: {error}")),
    }
}

fn section(title: &'static str) -> impl IntoElement {
    div()
        .pt_2()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .child(title)
}

fn labeled_input(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
        .child(input)
}
