//! Template generation logic
//!
//! Contains all the template generation code for different asset types.

use super::AssetKind;
use serde_json::json;

/// Template generator for various asset types
pub struct TemplateGenerator;

impl TemplateGenerator {
    /// Generate a template for the given asset kind and name
    pub fn generate(kind: AssetKind, name: &str) -> String {
        match kind {
            AssetKind::TypeAlias => Self::type_alias(name),
            AssetKind::Struct => Self::struct_type(name),
            AssetKind::Enum => Self::enum_type(name),
            AssetKind::Trait => Self::trait_type(name),
            AssetKind::Blueprint => Self::blueprint(name),
            AssetKind::BlueprintClass => Self::blueprint_class(name),
            AssetKind::BlueprintFunction => Self::blueprint_function(name),
            AssetKind::RustScript => Self::rust_script(name),
            AssetKind::LuaScript => Self::lua_script(name),
            AssetKind::Scene => Self::scene(name),
            AssetKind::Prefab => Self::prefab(name),
            AssetKind::Material => Self::material(name),
            AssetKind::Shader => Self::shader(name),
            AssetKind::AudioSource => Self::audio_source(name),
            AssetKind::AudioMixer => Self::audio_mixer(name),
            AssetKind::UILayout => Self::ui_layout(name),
            AssetKind::UITheme => Self::ui_theme(name),
            AssetKind::DataTable => Self::data_table(name),
            AssetKind::JsonData => Self::json_data(name),
            AssetKind::ProjectConfig => Self::project_config(name),
            AssetKind::EditorConfig => Self::editor_config(name),
        }
    }

    fn type_alias(name: &str) -> String {
        json!({
            "name": name,
            "display_name": name,
            "description": "",
            "ast": {
                "nodeKind": "Primitive",
                "name": "i32"
            }
        })
        .to_string()
    }

    fn struct_type(name: &str) -> String {
        json!({
            "name": name,
            "display_name": name,
            "description": "",
            "visibility": "Public",
            "fields": []
        })
        .to_string()
    }

    fn enum_type(name: &str) -> String {
        json!({
            "name": name,
            "display_name": name,
            "description": "",
            "visibility": "Public",
            "variants": []
        })
        .to_string()
    }

    fn trait_type(name: &str) -> String {
        json!({
            "name": name,
            "display_name": name,
            "description": "",
            "visibility": "Public",
            "methods": []
        })
        .to_string()
    }

    fn blueprint(name: &str) -> String {
        json!({
            "name": name,
            "version": "1.0.0",
            "nodes": [],
            "connections": []
        })
        .to_string()
    }

    fn blueprint_class(name: &str) -> String {
        json!({
            "name": name,
            "base_class": null,
            "variables": [],
            "functions": []
        })
        .to_string()
    }

    fn blueprint_function(name: &str) -> String {
        json!({
            "name": name,
            "parameters": [],
            "return_type": "void",
            "nodes": []
        })
        .to_string()
    }

    fn rust_script(name: &str) -> String {
        format!(
            "// {}\n\
             // Auto-generated Rust script\n\n\
             fn main() {{\n    \
                 tracing::debug!(\"Hello from {}\");\n\
             }}\n",
            name, name
        )
    }

    fn lua_script(name: &str) -> String {
        format!(
            "-- {}\n\
             -- Auto-generated Lua script\n\n\
             function init()\n    \
                 print(\"Hello from {}\")\n\
             end\n",
            name, name
        )
    }

    fn scene(name: &str) -> String {
        json!({
            "name": name,
            "entities": [],
            "environment": {
                "ambient_light": [1.0, 1.0, 1.0],
                "skybox": null
            }
        })
        .to_string()
    }

    fn prefab(name: &str) -> String {
        json!({
            "name": name,
            "root": {
                "transform": {
                    "position": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                },
                "components": []
            }
        })
        .to_string()
    }

    fn material(name: &str) -> String {
        json!({
            "name": name,
            "shader": "default",
            "properties": {
                "albedo": [1.0, 1.0, 1.0, 1.0],
                "metallic": 0.0,
                "roughness": 0.5
            },
            "textures": {}
        })
        .to_string()
    }

    fn shader(name: &str) -> String {
        format!(
            "// {} Shader\n\
             // WGSL Shader\n\n\
             @vertex\n\
             fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> @builtin(position) vec4<f32> {{\n    \
                 return vec4<f32>(0.0, 0.0, 0.0, 1.0);\n\
             }}\n\n\
             @fragment\n\
             fn fs_main() -> @location(0) vec4<f32> {{\n    \
                 return vec4<f32>(1.0, 1.0, 1.0, 1.0);\n\
             }}\n",
            name
        )
    }

    fn audio_source(name: &str) -> String {
        json!({
            "name": name,
            "file_path": "",
            "volume": 1.0,
            "loop": false,
            "spatial": false
        })
        .to_string()
    }

    fn audio_mixer(name: &str) -> String {
        json!({
            "name": name,
            "channels": [],
            "master_volume": 1.0
        })
        .to_string()
    }

    fn ui_layout(name: &str) -> String {
        json!({
            "name": name,
            "root": {
                "type": "Container",
                "children": []
            }
        })
        .to_string()
    }

    fn ui_theme(name: &str) -> String {
        json!({
            "name": name,
            "colors": {
                "primary": "#0066FF",
                "secondary": "#6C757D",
                "success": "#28A745",
                "danger": "#DC3545"
            },
            "fonts": {
                "default": "sans-serif",
                "monospace": "monospace"
            }
        })
        .to_string()
    }

    fn data_table(_name: &str) -> String {
        // SQLite database creation handled separately
        String::new()
    }

    fn json_data(name: &str) -> String {
        json!({
            "name": name,
            "data": {}
        })
        .to_string()
    }

    fn project_config(name: &str) -> String {
        format!(
            "# {} Project Configuration\n\
             [project]\n\
             name = \"{}\"\n\
             version = \"0.1.0\"\n\
             \n\
             [build]\n\
             target = \"debug\"\n",
            name, name
        )
    }

    fn editor_config(name: &str) -> String {
        format!(
            "# {} Editor Configuration\n\
             [editor]\n\
             theme = \"dark\"\n\
             font_size = 14\n\
             \n\
             [keybindings]\n\
             # Add custom keybindings\n",
            name
        )
    }
}
