//! Variable management - create, delete, and manage class variables

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::{Pin, PinType, variables::ClassVariable};
use ui::graph::DataType;

impl BlueprintEditorPanel {
    /// Start creating a new variable
    pub fn start_creating_variable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_creating_variable = true;

        self.variable_name_input = cx.new(|cx| {
            ui::input::InputState::new(window, cx).placeholder("Variable name...")
        });

        let available_types = self.get_available_types();
        let type_items: Vec<super::super::variables::TypeItem> = available_types.into_iter()
            .map(|type_str| super::super::variables::TypeItem::new(type_str))
            .collect();

        self.variable_type_dropdown.update(cx, |dropdown, cx| {
            dropdown.set_items(type_items, window, cx);
            dropdown.set_selected_index(Some(ui::IndexPath::default()), window, cx);
        });

        cx.notify();
    }

    /// Cancel variable creation
    pub fn cancel_creating_variable(&mut self, cx: &mut Context<Self>) {
        self.is_creating_variable = false;
        cx.notify();
    }

    /// Complete variable creation
    pub fn complete_creating_variable(&mut self, cx: &mut Context<Self>) {
        let name = self.variable_name_input.read(cx).text().to_string().trim().to_string();
        let selected_type = self.variable_type_dropdown.read(cx)
            .selected_value()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "i32".to_string());

        if !name.is_empty() {
            let variable = ClassVariable {
                name,
                var_type: selected_type,
                default_value: None,
            };
            self.class_variables.push(variable);

            if let Err(e) = self.save_variables_to_class() {
                eprintln!("Failed to save variables: {}", e);
            }
        }
        self.is_creating_variable = false;
        cx.notify();
    }

    /// Remove a variable
    pub fn remove_variable(&mut self, name: &str, cx: &mut Context<Self>) {
        self.class_variables.retain(|v| v.name != name);

        if let Err(e) = self.save_variables_to_class() {
            eprintln!("Failed to save variables: {}", e);
        }

        cx.notify();
    }

    /// Get available types
    pub fn get_available_types(&self) -> Vec<String> {
        ui::compiler::type_extractor::extract_all_blueprint_types()
    }

    /// Add input pin to subgraph
    pub fn add_input_pin(&mut self, cx: &mut Context<Self>) {
        if let Some(input_node) = self.graph.nodes.iter_mut().find(|n| n.definition_id == "subgraph_input") {
            let pin_count = input_node.outputs.len();
            let new_pin = Pin {
                id: format!("input_{}", pin_count),
                name: format!("Input {}", pin_count + 1),
                pin_type: PinType::Output,
                data_type: DataType::Execution,
            };
            input_node.outputs.push(new_pin);
            cx.notify();
        }
    }

    /// Add output pin to subgraph
    pub fn add_output_pin(&mut self, cx: &mut Context<Self>) {
        if let Some(output_node) = self.graph.nodes.iter_mut().find(|n| n.definition_id == "subgraph_output") {
            let pin_count = output_node.inputs.len();
            let new_pin = Pin {
                id: format!("output_{}", pin_count),
                name: format!("Output {}", pin_count + 1),
                pin_type: PinType::Input,
                data_type: DataType::Execution,
            };
            output_node.inputs.push(new_pin);
            cx.notify();
        }
    }

    /// Remove input pin from subgraph
    pub fn remove_input_pin(&mut self, pin_id: &str, cx: &mut Context<Self>) {
        if let Some(input_node) = self.graph.nodes.iter_mut().find(|n| n.definition_id == "subgraph_input") {
            input_node.outputs.retain(|p| p.id != pin_id);
            cx.notify();
        }
    }

    /// Remove output pin from subgraph
    pub fn remove_output_pin(&mut self, pin_id: &str, cx: &mut Context<Self>) {
        if let Some(output_node) = self.graph.nodes.iter_mut().find(|n| n.definition_id == "subgraph_output") {
            output_node.inputs.retain(|p| p.id != pin_id);
            cx.notify();
        }
    }

    /// Load variables from vars_save.json
    pub(super) fn load_variables_from_class(&mut self, class_path: &std::path::Path) -> Result<(), String> {
        let vars_file = class_path.join("vars_save.json");

        if !vars_file.exists() {
            self.class_variables.clear();
            return Ok(());
        }

        let content = std::fs::read_to_string(&vars_file)
            .map_err(|e| format!("Failed to read vars_save.json: {}", e))?;
        let variables: Vec<ClassVariable> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse vars_save.json: {}", e))?;

        self.class_variables = variables;
        Ok(())
    }

    /// Save variables to vars_save.json
    pub(super) fn save_variables_to_class(&self) -> Result<(), String> {
        let class_path = self.current_class_path.as_ref()
            .ok_or_else(|| "No class currently loaded".to_string())?;

        let vars_file = class_path.join("vars_save.json");
        let json = serde_json::to_string_pretty(&self.class_variables)
            .map_err(|e| format!("Failed to serialize variables: {}", e))?;

        std::fs::write(&vars_file, json)
            .map_err(|e| format!("Failed to write vars_save.json: {}", e))?;

        Ok(())
    }

    /// Finish dragging variable and show Get/Set context menu
    pub fn finish_dragging_variable(&mut self, drop_position: Point<f32>, cx: &mut Context<Self>) {
        if self.dragging_variable.is_some() {
            self.variable_drop_menu_position = Some(drop_position);
            cx.notify();
        }
    }
    
    /// Cancel dragging variable
    pub fn cancel_dragging_variable(&mut self, cx: &mut Context<Self>) {
        self.dragging_variable = None;
        self.variable_drop_menu_position = None;
        cx.notify();
    }
    
    /// Create a getter node for a variable at the specified position
    pub fn create_getter_node(
        &mut self,
        var_name: String,
        var_type: String,
        position: Point<f32>,
        cx: &mut Context<Self>,
    ) {
        use super::super::BlueprintNode;
        use super::super::NodeType;
        
        let node_id = format!("get_{}_node_{}", var_name, uuid::Uuid::new_v4());

        let node = BlueprintNode {
            id: node_id,
            definition_id: format!("get_{}", var_name),
            title: format!("Get {}", var_name),
            icon: "📖".to_string(),
            node_type: NodeType::Logic,
            position,
            size: super::super::Size::new(180.0, 80.0),
            inputs: vec![],
            outputs: vec![Pin {
                id: "value".to_string(),
                name: var_name.clone(),
                pin_type: PinType::Output,
                data_type: DataType::from_type_str(&var_type),
            }],
            properties: std::collections::HashMap::new(),
            is_selected: false,
            description: format!("Gets the value of {}", var_name),
            color: None,
        };

        self.add_node(node, cx);
        self.cancel_dragging_variable(cx);
    }
    
    /// Create a setter node for a variable at the specified position
    pub fn create_setter_node(
        &mut self,
        var_name: String,
        var_type: String,
        position: Point<f32>,
        cx: &mut Context<Self>,
    ) {
        use super::super::BlueprintNode;
        use super::super::NodeType;
        
        let node_id = format!("set_{}_node_{}", var_name, uuid::Uuid::new_v4());

        let node = BlueprintNode {
            id: node_id,
            definition_id: format!("set_{}", var_name),
            title: format!("Set {}", var_name),
            icon: "📝".to_string(),
            node_type: NodeType::Logic,
            position,
            size: super::super::Size::new(180.0, 100.0),
            inputs: vec![
                Pin {
                    id: "exec".to_string(),
                    name: "".to_string(),
                    pin_type: PinType::Input,
                    data_type: DataType::from_type_str("execution"),
                },
                Pin {
                    id: "value".to_string(),
                    name: var_name.clone(),
                    pin_type: PinType::Input,
                    data_type: DataType::from_type_str(&var_type),
                },
            ],
            outputs: vec![Pin {
                id: "exec_out".to_string(),
                name: "".to_string(),
                pin_type: PinType::Output,
                data_type: DataType::from_type_str("execution"),
            }],
            properties: std::collections::HashMap::new(),
            is_selected: false,
            description: format!("Sets the value of {}", var_name),
            color: None,
        };

        self.add_node(node, cx);
        self.cancel_dragging_variable(cx);
    }
    
    /// Start dragging a variable
    pub fn start_dragging_variable(&mut self, var_name: String, var_type: String, cx: &mut Context<Self>) {
        self.dragging_variable = Some(super::super::variables::VariableDrag {
            var_name,
            var_type,
        });
        cx.notify();
    }
    
    /// Get search query (placeholder)
    pub fn get_search_query(&self) -> String {
        String::new()
    }
    
    /// Get search input state
    pub fn get_search_input_state(&self) -> &Entity<ui::input::InputState> {
        &self.variable_name_input
    }

    /// Generate vars/mod.rs from current variables
    pub(super) fn generate_vars_module(&self) -> Result<(), String> {
        let class_path = self.current_class_path.as_ref()
            .ok_or_else(|| "No class currently loaded".to_string())?;

        let vars_dir = class_path.join("vars");
        std::fs::create_dir_all(&vars_dir)
            .map_err(|e| format!("Failed to create vars directory: {}", e))?;

        let mut code = String::new();
        code.push_str("//! Auto-generated variables module\n");
        code.push_str("//! DO NOT EDIT MANUALLY - YOUR CHANGES WILL BE OVERWRITTEN\n\n");

        let needs_refcell = self.class_variables.iter().any(|v| {
            !matches!(
                v.var_type.as_str(),
                "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "bool" |
                "char" | "usize" | "isize" | "i8" | "i16" | "u8" | "u16"
            )
        });

        code.push_str("use std::cell::Cell;\n");
        if needs_refcell {
            code.push_str("use std::cell::RefCell;\n");
        }
        code.push_str("\n");

        for var in &self.class_variables {
            let default_value = if let Some(default) = &var.default_value {
                default.clone()
            } else {
                match var.var_type.as_str() {
                    "i32" | "i64" | "u32" | "u64" | "f32" | "f64" => "0".to_string(),
                    "bool" => "false".to_string(),
                    "&str" => "\"\"".to_string(),
                    "String" => "String::new()".to_string(),
                    _ => "Default::default()".to_string(),
                }
            };

            let use_cell = matches!(
                var.var_type.as_str(),
                "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "bool" |
                "char" | "usize" | "isize" | "i8" | "i16" | "u8" | "u16"
            );

            let cell_type = if use_cell { "Cell" } else { "RefCell" };
            code.push_str(&format!(
                "thread_local! {{\n    pub static {}: {}::<{}> = {}::new({});\n}}\n\n",
                var.name.to_uppercase(),
                cell_type,
                var.var_type,
                cell_type,
                default_value
            ));
        }

        let vars_mod_file = vars_dir.join("mod.rs");
        std::fs::write(&vars_mod_file, code)
            .map_err(|e| format!("Failed to write vars/mod.rs: {}", e))?;

        Ok(())
    }
}
