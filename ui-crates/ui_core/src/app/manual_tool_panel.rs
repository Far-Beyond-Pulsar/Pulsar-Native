use agent_chat_tools::{ToolContext, ToolRegistry};
use gpui::{
    div, px, App, AppContext, Context, Corner, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Subscription, Window,
};
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

/// A tool that can be called from the manual tool runner.
/// For plugin tools, `plugin_id` is set and execution goes directly to the plugin.
/// Core tools are called through the ToolRegistry.
#[derive(Clone)]
struct ToolOption {
    /// Display name shown in the dropdown
    display_name: String,
    /// Actual tool name used for execution
    tool_name: String,
    description: String,
    /// The user-visible parameter schema (already unwrapped for plugin tools)
    parameters: Value,
    /// Set for plugin tools — the plugin that owns this tool
    plugin_id: Option<String>,
    /// Category / plugin group label shown in the dropdown
    group: String,
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
    selected_tool_key: Option<String>, // "plugin_id::tool_name" or "core::tool_name"
    selected_tool_description: String,
    selected_plugin_id: Option<String>,
    fields_by_key: HashMap<String, Vec<DynamicField>>,
    dynamic_fields: Vec<DynamicField>,
    /// Shared context file path input shown when a plugin tool is selected
    file_path_input: Entity<InputState>,
    result_text: String,
    result_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ManualToolPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tool_registry = ToolRegistry::with_default_tools();
        let tool_catalog = Self::build_tool_catalog(&tool_registry);

        let file_path_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Context file path (e.g. scenes/default.level)")
        });

        let tool_list = cx.new(|cx| {
            SearchableList::new(window, cx, tool_catalog.clone(), |t: &ToolOption| {
                format!("[{}] {}", t.group, t.display_name)
            })
            .with_empty_text("No tools found")
            .with_max_width(px(400.0))
            .with_max_height(px(480.0))
            .with_icon_getter(|_| IconName::Brain)
        });

        let mut fields_by_key: HashMap<String, Vec<DynamicField>> = HashMap::new();
        for tool in &tool_catalog {
            let key = Self::tool_key(tool);
            let fields = Self::build_fields_for_tool(window, cx, tool);
            fields_by_key.insert(key, fields);
        }

        let first = tool_catalog.first();
        let selected_tool_key = first.map(|t| Self::tool_key(t));
        let selected_tool_description = first
            .map(|t| t.description.clone())
            .unwrap_or_else(|| "Select a tool to configure inputs.".to_string());
        let selected_plugin_id = first.and_then(|t| t.plugin_id.clone());
        let dynamic_fields = selected_tool_key
            .as_ref()
            .and_then(|k| fields_by_key.get(k).cloned())
            .unwrap_or_default();

        let subscriptions = vec![cx.subscribe(
            &tool_list,
            move |this, _, event: &SearchableListEvent<ToolOption>, cx| {
                if let SearchableListEvent::Select(opt) = event {
                    this.select_tool_option(opt, cx);
                }
            },
        )];

        Self {
            focus_handle: cx.focus_handle(),
            tool_registry,
            tool_catalog,
            tool_list,
            selected_tool_key,
            selected_tool_description,
            selected_plugin_id,
            fields_by_key,
            dynamic_fields,
            file_path_input,
            result_text: "Ready. Select a tool and click Run Tool.".to_string(),
            result_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn tool_key(opt: &ToolOption) -> String {
        match &opt.plugin_id {
            Some(pid) => format!("plugin::{}::{}", pid, opt.tool_name),
            None => format!("core::{}", opt.tool_name),
        }
    }

    /// Build a combined catalog: core tools first, then all plugin tools.
    fn build_tool_catalog(tool_registry: &ToolRegistry) -> Vec<ToolOption> {
        let mut catalog: Vec<ToolOption> = Vec::new();

        // ── Core tools from ToolRegistry ──────────────────────────────────
        // Exclude meta-tools that are only meaningful for agent-to-agent calls
        const HIDDEN_CORE: &[&str] = &["execute_plugin_tool", "query_plugin_tools"];

        for schema in tool_registry.available_tools_schema() {
            let name = match schema.get("name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if HIDDEN_CORE.contains(&name.as_str()) {
                continue;
            }
            let description = schema
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let parameters = schema.get("parameters").cloned().unwrap_or_else(|| json!({}));
            catalog.push(ToolOption {
                display_name: name.clone(),
                tool_name: name,
                description,
                parameters,
                plugin_id: None,
                group: "core".to_string(),
            });
        }

        // ── Plugin tools ──────────────────────────────────────────────────
        if let Some(manager_lock) = plugin_manager::global() {
            if let Ok(manager) = manager_lock.read() {
                let bridge = manager.build_tool_bridge();
                let mut plugin_tools = bridge.all_tools();
                // Sort for stable display order
                plugin_tools.sort_by(|a, b| {
                    a.plugin_id
                        .as_str()
                        .cmp(b.plugin_id.as_str())
                        .then(a.definition.name.cmp(&b.definition.name))
                });

                for available in plugin_tools {
                    let pid = available.plugin_id.to_string();
                    let def = &available.definition;
                    catalog.push(ToolOption {
                        display_name: def.name.clone(),
                        tool_name: def.name.clone(),
                        description: def.description.clone(),
                        parameters: def.parameters_json_schema.clone(),
                        plugin_id: Some(pid.clone()),
                        group: pid,
                    });
                }
            }
        }

        catalog
    }

    fn select_tool_option(&mut self, opt: &ToolOption, cx: &mut Context<Self>) {
        let key = Self::tool_key(opt);
        self.selected_tool_key = Some(key.clone());
        self.selected_plugin_id = opt.plugin_id.clone();
        self.selected_tool_description = opt.description.clone();
        self.dynamic_fields = self
            .fields_by_key
            .get(&key)
            .cloned()
            .unwrap_or_default();
        cx.notify();
    }

    fn infer_field_kind(schema: Option<&Value>) -> FieldKind {
        let Some(schema) = schema else {
            return FieldKind::Unknown;
        };
        let resolved = match schema.get("type") {
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
                if let Some(n) = r.as_str() {
                    required.insert(n.to_string());
                }
            }
        }

        let props = tool
            .parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        props
            .into_iter()
            .map(|(name, schema)| {
                let kind = Self::infer_field_kind(Some(&schema));
                let description = schema
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let placeholder = if description.is_empty() {
                    format!("{} ({})", name, kind.label())
                } else {
                    format!("{} — {}", name, description)
                };
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder(&placeholder));
                DynamicField {
                    required: required.contains(&name),
                    name,
                    kind,
                    description,
                    input,
                }
            })
            .collect()
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
                Number::from_f64(parsed)
                    .map(Value::Number)
                    .ok_or_else(|| "number cannot be NaN or infinite".to_string())
            }
            FieldKind::Boolean => match raw.to_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(Value::Bool(true)),
                "false" | "0" | "no" => Ok(Value::Bool(false)),
                _ => Err("expected boolean (true/false)".to_string()),
            },
            FieldKind::Object | FieldKind::Array | FieldKind::Unknown => {
                serde_json::from_str::<Value>(raw).map_err(|e| format!("invalid JSON: {e}"))
            }
        }
    }

    fn run_tool(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_tool_key.clone() else {
            self.result_text = "Select a tool first.".to_string();
            cx.notify();
            return;
        };

        // Collect field values
        let mut args = serde_json::Map::new();
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
            match Self::parse_field_value(field.kind, &raw) {
                Ok(v) => { args.insert(field.name.clone(), v); }
                Err(err) => {
                    self.result_text = format!("Invalid value for '{}': {}", field.name, err);
                    cx.notify();
                    return;
                }
            }
        }

        // ── Plugin tool path ──────────────────────────────────────────────
        if let Some(plugin_id) = self.selected_plugin_id.clone() {
            let tool_name = key
                .splitn(4, "::")
                .nth(2)
                .unwrap_or("")
                .to_string();

            let file_path_raw = self.file_path_input.read(cx).text().to_string();
            let file_path_raw = file_path_raw.trim().to_string();
            let file_path = if file_path_raw.is_empty() {
                // Fall back to project root
                engine_state::get_project_path()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            } else {
                let root = engine_state::get_project_path()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                let p = PathBuf::from(&file_path_raw);
                if p.is_absolute() { p } else { root.join(p) }
            };

            let result = plugin_manager::global()
                .and_then(|lock| lock.read().ok().map(|m| {
                    m.execute_plugin_ai_tool(
                        &plugin_manager::PluginId::new(&plugin_id),
                        &file_path,
                        &tool_name,
                        Value::Object(args),
                    )
                }));

            self.result_text = match result {
                Some(Ok(v)) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
                Some(Err(e)) => format!("Plugin error: {e}"),
                None => "Plugin manager not available".to_string(),
            };
            cx.notify();
            return;
        }

        // ── Core tool path ────────────────────────────────────────────────
        let tool_name = key.trim_start_matches("core::").to_string();
        let file_path_raw = self.file_path_input.read(cx).text().to_string();
        let first_file_path = {
            let raw = file_path_raw.trim().to_string();
            if raw.is_empty() { None } else { Some(raw) }
        };

        // Action-backed shortcut tools
        if tool_name == "open_file_in_default_editor" {
            if let Some(ref path) = first_file_path {
                let path_buf = PathBuf::from(path);
                cx.dispatch_action(&crate::actions::OpenFile { path: path_buf.clone() });
                self.result_text = json!({"ok": true, "dispatched": "OpenFile", "path": path_buf.display().to_string()}).to_string();
                cx.notify();
                return;
            }
        }
        if tool_name == "activate_open_editor" {
            if let Some(index) = args.get("index").and_then(|v| v.as_u64()).map(|v| v as usize) {
                cx.dispatch_action(&crate::actions::ActivateOpenEditor { index });
                self.result_text = json!({"ok": true, "dispatched": "ActivateOpenEditor", "index": index}).to_string();
                cx.notify();
                return;
            }
        }

        let workspace_root = engine_state::get_project_path()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        let plugin_bridge = plugin_manager::global().and_then(|manager_lock| {
            manager_lock
                .read()
                .ok()
                .map(|manager| Arc::new(RwLock::new(manager.build_tool_bridge())))
        });

        let ctx = ToolContext {
            workspace_root,
            plugin_bridge,
            current_file: first_file_path.clone().map(PathBuf::from),
            open_file_request: None,
            query_open_editors: Some(Arc::new(|| Ok(crate::app::open_editors::snapshot_json()))),
            activate_open_editor_request: None,
        };

        let result = self
            .tool_registry
            .execute(&tool_name, Value::Object(args), &ctx);
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
        let run_disabled = self.selected_tool_key.is_none();
        let is_plugin_tool = self.selected_plugin_id.is_some();

        let tool_list = self.tool_list.clone();
        let selected_label = self
            .selected_tool_key
            .as_ref()
            .and_then(|k| {
                self.tool_catalog
                    .iter()
                    .find(|t| &Self::tool_key(t) == k)
                    .map(|t| format!("[{}] {}", t.group, t.display_name))
            })
            .unwrap_or_else(|| "Select tool".to_string());

        let tool_popover = Popover::<SearchableList<ToolOption>>::new("manual-tool-popover")
            .anchor(Corner::TopLeft)
            .trigger(
                Button::new("manual-tool-select-trigger")
                    .xsmall()
                    .ghost()
                    .justify_start()
                    .tooltip("Select tool")
                    .label(selected_label)
                    .dropdown_caret(true),
            )
            .content(move |_window, _cx| tool_list.clone());

        // File path input shown for all tools (needed as context for plugin tools)
        let file_path_row = v_flex()
            .w_full()
            .gap_1()
            .child(
                div().text_xs().child(if is_plugin_tool {
                    "File path (context for this tool) *"
                } else {
                    "File path (optional context)"
                }),
            )
            .child(TextInput::new(&self.file_path_input).w_full());

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
                    .when(!field.description.is_empty(), |this: gpui::Div| {
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
            .child(file_path_row)
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
