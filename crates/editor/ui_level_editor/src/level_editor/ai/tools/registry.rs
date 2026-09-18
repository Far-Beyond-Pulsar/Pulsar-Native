//! Registry wiring: exposes the editor's AI tools to `plugin_editor_api`.

use super::definitions::ai_tool_definitions;
use super::execute::execute_ai_tool_impl;
use super::*;

struct LevelEditorRegistryTool {
    name: &'static str,
    description: &'static str,
    category: Option<&'static str>,
    parameters_schema: Value,
}

impl ChatTool for LevelEditorRegistryTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn category(&self) -> Option<&'static str> {
        self.category
    }

    fn parameters_schema(&self) -> Value {
        self.parameters_schema.clone()
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = ctx
            .current_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Current file path missing from ToolContext"))?;
        execute_ai_tool_impl(file_path, self.name, args)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }
}

pub(super) fn tool_registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ToolRegistry::new();
        for definition in ai_tool_definitions() {
            let name: &'static str = Box::leak(definition.name.clone().into_boxed_str());
            let description: &'static str =
                Box::leak(definition.description.clone().into_boxed_str());
            let category: Option<&'static str> = definition
                .category
                .as_ref()
                .map(|c| Box::leak(c.clone().into_boxed_str()) as &'static str);

            registry.register(Arc::new(LevelEditorRegistryTool {
                name,
                description,
                category,
                parameters_schema: definition.parameters_json_schema.clone(),
            }));
        }
        registry
    })
}

pub fn ai_tools() -> Vec<AiToolDefinition> {
    tool_registry()
        .definitions()
        .into_iter()
        .map(|def| {
            let mut ai_def =
                AiToolDefinition::new(def.name, def.description, def.parameters_schema);
            if let Some(category) = def.category {
                ai_def = ai_def.with_category(category);
            }
            ai_def
        })
        .collect()
}

pub fn capabilities_for_file(file_path: &Path) -> Vec<String> {
    if !is_level_file(file_path) {
        return Vec::new();
    }

    tool_registry()
        .names()
        .into_iter()
        .map(|name| name.to_string())
        .collect()
}