use agent_chat_tools::{ToolContext, ToolRegistry};
use gpui::{
    div, px, App, AppContext, Context, Corner, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, Styled, Subscription,
    Window,
};
 use gpui::StatefulInteractiveElement;
use gpui::prelude::FluentBuilder as _;
use serde_json::{json, Number, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    input::{InputState, TextInput},
    popover::Popover,
    v_flex, ActiveTheme as _, Disableable, IconName, Sizable,
};

#[derive(Clone)]
struct ToolOption {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Clone, Copy)]
enum FieldKind {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
    Unknown,
}

impl FieldKind {
    fn label(self) -> &'static str {
        match self {
            FieldKind::String => "string",
            FieldKind::Integer => "integer",
            FieldKind::Number => "number",
            FieldKind::Boolean => "boolean",
            FieldKind::Object => "object(json)",
            FieldKind::Array => "array(json)",
            FieldKind::Unknown => "json",
        }
    }
}

#[derive(Clone)]
struct DynamicField {
    name: String,
    kind: FieldKind,
    required: bool,
    description: String,
    input: Entity<InputState>,
}

pub struct ManualToolPanel {
    focus_handle: FocusHandle,
    tool_registry: ToolRegistry,
    tool_catalog: Vec<ToolOption>,
    tool_list: Entity<SearchableList<ToolOption>>,
    selected_tool_name: Option<String>,
    selected_tool_description: String,
    fields_by_tool: HashMap<String, Vec<DynamicField>>,
    dynamic_fields: Vec<DynamicField>,
    result_text: String,
    result_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ManualToolPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tool_registry = ToolRegistry::with_default_tools();
        let tool_catalog = Self::build_tool_catalog(&tool_registry);

        let tool_list = cx.new(|cx| {
            SearchableList::new(window, cx, tool_catalog.clone(), |t: &ToolOption| {
                format!("{}", t.name)
            })
            .with_empty_text("No tools found")
            .with_max_width(px(360.0))
            .with_max_height(px(420.0))
            .with_icon_getter(|_| IconName::Brain)
        });

        let mut fields_by_tool = HashMap::new();
        for tool in &tool_catalog {
            let fields = Self::build_fields_for_tool(window, cx, tool);
            fields_by_tool.insert(tool.name.clone(), fields);
        }

        let selected_tool_name = tool_catalog.first().map(|t| t.name.clone());
        let selected_tool_description = tool_catalog
            .first()
            .map(|t| t.description.clone())
            .unwrap_or_else(|| "Select a tool to configure inputs.".to_string());
        let dynamic_fields = selected_tool_name
            .as_ref()
            .and_then(|name| fields_by_tool.get(name).cloned())
            .unwrap_or_default();

        let subscriptions = vec![cx.subscribe(
            &tool_list,
            move |this, _, event: &SearchableListEvent<ToolOption>, cx| {
                if let SearchableListEvent::Select(selected_tool) = event {
                    this.select_tool(&selected_tool.name, cx);
                }
            },
        )];

        Self {
            focus_handle: cx.focus_handle(),
            tool_registry,
            tool_catalog,
            tool_list,
            selected_tool_name,
            selected_tool_description,
            fields_by_tool,
            dynamic_fields,
            result_text: "Ready. Select a tool and click Run Tool.".to_string(),
            result_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn build_tool_catalog(tool_registry: &ToolRegistry) -> Vec<ToolOption> {
        tool_registry
            .available_tools_schema()
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str())?.to_string();
                let description = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let parameters = tool.get("parameters").cloned().unwrap_or_else(|| json!({}));
                Some(ToolOption {
                    name,
                    description,
                    parameters,
                })
            })
            .collect()
    }

    fn select_tool(&mut self, tool_name: &str, cx: &mut Context<Self>) {
        self.selected_tool_name = Some(tool_name.to_string());
        self.dynamic_fields = self
            .fields_by_tool
            .get(tool_name)
            .cloned()
            .unwrap_or_default();

        self.selected_tool_description = self
            .tool_catalog
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| t.description.clone())
            .unwrap_or_default();

        cx.notify();
    }

    fn infer_field_kind(schema: Option<&Value>) -> FieldKind {
        let Some(schema) = schema else {
            return FieldKind::Unknown;
        };

        let type_value = schema.get("type");
        let resolved = match type_value {
            Some(Value::String(t)) => Some(t.clone()),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .find(|t| *t != "null")
                .map(|t| t.to_string()),
            _ => None,
        };

        match resolved.as_deref() {
            Some("string") => FieldKind::String,
            Some("integer") => FieldKind::Integer,
            Some("number") => FieldKind::Number,
            Some("boolean") => FieldKind::Boolean,
            Some("object") => FieldKind::Object,
            Some("array") => FieldKind::Array,
            _ => FieldKind::Unknown,
        }
    }

    fn build_fields_for_tool(
        window: &mut Window,
        cx: &mut Context<Self>,
        tool: &ToolOption,
    ) -> Vec<DynamicField> {
        let mut required = HashSet::new();
        if let Some(req) = tool.parameters.get("required").and_then(|v| v.as_array()) {
            for r in req {
                if let Some(name) = r.as_str() {
                    required.insert(name.to_string());
                }
            }
        }

        let mut fields = Vec::new();
        let props = tool
            .parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        for (name, schema) in props {
            let kind = Self::infer_field_kind(Some(&schema));
            let description = schema
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let placeholder = if description.is_empty() {
                format!("{} ({})", name, kind.label())
            } else {
                format!("{} ({}) - {}", name, kind.label(), description)
            };

            let input = cx.new(|cx| InputState::new(window, cx).placeholder(&placeholder));
            fields.push(DynamicField {
                name: name.clone(),
                kind,
                required: required.contains(&name),
                description,
                input,
            });
        }

        fields
    }

    fn parse_field_value(kind: FieldKind, raw: &str) -> Result<Value, String> {
        match kind {
            FieldKind::String => Ok(Value::String(raw.to_string())),
            FieldKind::Integer => raw
                .parse::<i64>()
                .map(Value::from)
                .map_err(|e| format!("expected integer: {e}")),
            FieldKind::Number => {
                let parsed = raw
                    .parse::<f64>()
                    .map_err(|e| format!("expected number: {e}"))?;
                let Some(num) = Number::from_f64(parsed) else {
                    return Err("number cannot be NaN or infinite".to_string());
                };
                Ok(Value::Number(num))
            }
            FieldKind::Boolean => {
                let lowered = raw.to_lowercase();
                match lowered.as_str() {
                    "true" | "1" | "yes" => Ok(Value::Bool(true)),
                    "false" | "0" | "no" => Ok(Value::Bool(false)),
                    _ => Err("expected boolean (true/false)".to_string()),
                }
            }
            FieldKind::Object | FieldKind::Array | FieldKind::Unknown => {
                serde_json::from_str::<Value>(raw).map_err(|e| format!("invalid JSON: {e}"))
            }
        }
    }

    fn run_tool(&mut self, cx: &mut Context<Self>) {
        let Some(tool_name) = self.selected_tool_name.clone() else {
            self.result_text = "Select a tool first.".to_string();
            cx.notify();
            return;
        };

        let mut args = serde_json::Map::new();
        let mut first_file_path: Option<String> = None;

        for field in &self.dynamic_fields {
            let raw = field.input.read(cx).text().to_string();
            let raw = raw.trim().to_string();

            if raw.is_empty() {
                if field.required {
                    self.result_text = format!("Missing required field: {}", field.name);
                    cx.notify();
                    return;
                }
                continue;
            }

            let parsed = match Self::parse_field_value(field.kind, &raw) {
                Ok(v) => v,
                Err(err) => {
                    self.result_text = format!("Invalid value for '{}': {}", field.name, err);
                    cx.notify();
                    return;
                }
            };

            if field.name == "file_path" {
                if let Some(s) = parsed.as_str() {
                    first_file_path = Some(s.to_string());
                }
            }

            args.insert(field.name.clone(), parsed);
        }

        if tool_name == "open_file_in_default_editor" {
            if let Some(path) = first_file_path {
                let path_buf = PathBuf::from(path);
                cx.dispatch_action(&crate::actions::OpenFile {
                    path: path_buf.clone(),
                });
                self.result_text = json!({
                    "ok": true,
                    "dispatched": "OpenFile",
                    "path": path_buf.display().to_string()
                })
                .to_string();
                cx.notify();
                return;
            }
        }

        if tool_name == "activate_open_editor" {
            if let Some(index) = args.get("index").and_then(|v| v.as_u64()).map(|v| v as usize) {
                cx.dispatch_action(&crate::actions::ActivateOpenEditor { index });
                self.result_text = json!({
                    "ok": true,
                    "dispatched": "ActivateOpenEditor",
                    "index": index
                })
                .to_string();
                cx.notify();
                return;
            }
        }

        let workspace_root = match engine_state::get_project_path() {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from("."),
        };

        let plugin_bridge = plugin_manager::global().and_then(|manager_lock| {
            manager_lock
                .read()
                .ok()
                .map(|manager| Arc::new(RwLock::new(manager.build_tool_bridge())))
        });

        let current_file = first_file_path.clone().map(PathBuf::from);

        let ctx = ToolContext {
            workspace_root,
            plugin_bridge,
            current_file,
            open_file_request: None,
            query_open_editors: Some(Arc::new(|| Ok(crate::app::open_editors::snapshot_json()))),
            activate_open_editor_request: None,
        };

        let result = self.tool_registry.execute(&tool_name, Value::Object(args), &ctx);
        self.result_text = match result {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
            Err(err) => format!("Tool error: {err}"),
        };

        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for ManualToolPanel {}

impl Focusable for ManualToolPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ManualToolPanel {
    fn panel_name(&self) -> &'static str {
        "manual_tool_runner"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        "Tools".into_any_element()
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState::new(self)
    }
}

impl Render for ManualToolPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let run_disabled = self.selected_tool_name.is_none();

        let tool_list = self.tool_list.clone();
        let selected_label = self
            .selected_tool_name
            .clone()
            .unwrap_or_else(|| "Select tool".to_string());

        let tool_popover = Popover::<SearchableList<ToolOption>>::new("manual-tool-popover")
            .anchor(Corner::TopLeft)
            .trigger(
                Button::new("manual-tool-select-trigger")
                    .xsmall()
                    .ghost()
                    .justify_start()
                    .tooltip("Select tool")
                    .label(format!("Tool: {}", selected_label))
                    .dropdown_caret(true),
            )
            .content(move |_window, _cx| tool_list.clone());

        let mut dynamic_section = v_flex().w_full().gap_2();
        for field in &self.dynamic_fields {
            let title = if field.required {
                format!("{} ({}) *", field.name, field.kind.label())
            } else {
                format!("{} ({})", field.name, field.kind.label())
            };
            dynamic_section = dynamic_section.child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(div().text_xs().child(title))
                    .when(!field.description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(field.description.clone()),
                        )
                    })
                    .child(TextInput::new(&field.input).w_full()),
            );
        }

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_2()
            .gap_2()
            .child(tool_popover)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.selected_tool_description.clone()),
            )
            .child(dynamic_section)
            .child(
                h_flex().child(
                    Button::new("manual-tool-run")
                        .icon(IconName::Play)
                        .label("Run Tool")
                        .disabled(run_disabled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_tool(cx);
                        })),
                ),
            )
            .child(
                div()
                    .id("manual-tool-result")
                    .w_full()
                    .h(gpui::px(200.0))
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .p_2()
                    .text_xs()
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .track_scroll(&self.result_scroll)
                    .child(self.result_text.clone()),
            )
    }
}
