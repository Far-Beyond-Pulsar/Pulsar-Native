//! Asset Templates
//!
//! Provides templates for creating new assets of any type

use serde_json::json;

/// All possible asset types that can be created
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    // Type System
    TypeAlias,
    Struct,
    Enum,
    Trait,
    
    // Blueprint System
    Blueprint,
    BlueprintClass,
    BlueprintFunction,
    
    // Scripts
    RustScript,
    LuaScript,
    
    // Scenes
    Scene,
    Prefab,
    
    // Materials & Shaders
    Material,
    Shader,
    
    // Audio
    AudioSource,
    AudioMixer,
    
    // UI
    UILayout,
    UITheme,
    
    // Data
    DataTable,
    JsonData,
    
    // Config
    ProjectConfig,
    EditorConfig,
}

impl AssetKind {
    /// Get the file extension for this asset type
    pub fn extension(&self) -> &'static str {
        match self {
            AssetKind::TypeAlias => "alias.json",
            AssetKind::Struct => "struct.json",
            AssetKind::Enum => "enum.json",
            AssetKind::Trait => "trait.json",
            AssetKind::Blueprint => "blueprint.json",
            AssetKind::BlueprintClass => "bpclass.json",
            AssetKind::BlueprintFunction => "bpfunc.json",
            AssetKind::RustScript => "rs",
            AssetKind::LuaScript => "lua",
            AssetKind::Scene => "scene.json",
            AssetKind::Prefab => "prefab.json",
            AssetKind::Material => "mat.json",
            AssetKind::Shader => "shader.wgsl",
            AssetKind::AudioSource => "audio.json",
            AssetKind::AudioMixer => "mixer.json",
            AssetKind::UILayout => "ui.json",
            AssetKind::UITheme => "theme.json",
            AssetKind::DataTable => "table.db",
            AssetKind::JsonData => "json",
            AssetKind::ProjectConfig => "project.toml",
            AssetKind::EditorConfig => "editor.toml",
        }
    }
    
    /// Get the default subdirectory for this asset type
    pub fn default_directory(&self) -> &'static str {
        match self {
            AssetKind::TypeAlias => "types/aliases",
            AssetKind::Struct => "types/structs",
            AssetKind::Enum => "types/enums",
            AssetKind::Trait => "types/traits",
            AssetKind::Blueprint => "blueprints",
            AssetKind::BlueprintClass => "blueprints/classes",
            AssetKind::BlueprintFunction => "blueprints/functions",
            AssetKind::RustScript => "scripts/rust",
            AssetKind::LuaScript => "scripts/lua",
            AssetKind::Scene => "scenes",
            AssetKind::Prefab => "prefabs",
            AssetKind::Material => "materials",
            AssetKind::Shader => "shaders",
            AssetKind::AudioSource => "audio/sources",
            AssetKind::AudioMixer => "audio/mixers",
            AssetKind::UILayout => "ui/layouts",
            AssetKind::UITheme => "ui/themes",
            AssetKind::DataTable => "data/tables",
            AssetKind::JsonData => "data",
            AssetKind::ProjectConfig => "config",
            AssetKind::EditorConfig => "config",
        }
    }
    
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            AssetKind::TypeAlias => "Type Alias",
            AssetKind::Struct => "Struct",
            AssetKind::Enum => "Enum",
            AssetKind::Trait => "Trait",
            AssetKind::Blueprint => "Blueprint",
            AssetKind::BlueprintClass => "Blueprint Class",
            AssetKind::BlueprintFunction => "Blueprint Function",
            AssetKind::RustScript => "Rust Script",
            AssetKind::LuaScript => "Lua Script",
            AssetKind::Scene => "Scene",
            AssetKind::Prefab => "Prefab",
            AssetKind::Material => "Material",
            AssetKind::Shader => "Shader",
            AssetKind::AudioSource => "Audio Source",
            AssetKind::AudioMixer => "Audio Mixer",
            AssetKind::UILayout => "UI Layout",
            AssetKind::UITheme => "UI Theme",
            AssetKind::DataTable => "Data Table",
            AssetKind::JsonData => "JSON Data",
            AssetKind::ProjectConfig => "Project Config",
            AssetKind::EditorConfig => "Editor Config",
        }
    }
    
    /// Get icon for UI
    pub fn icon(&self) -> &'static str {
        match self {
            AssetKind::TypeAlias => "🔗",
            AssetKind::Struct => "📦",
            AssetKind::Enum => "🎯",
            AssetKind::Trait => "🔧",
            AssetKind::Blueprint => "🔷",
            AssetKind::BlueprintClass => "📘",
            AssetKind::BlueprintFunction => "⚡",
            AssetKind::RustScript => "🦀",
            AssetKind::LuaScript => "🌙",
            AssetKind::Scene => "🎬",
            AssetKind::Prefab => "🎁",
            AssetKind::Material => "🎨",
            AssetKind::Shader => "✨",
            AssetKind::AudioSource => "🔊",
            AssetKind::AudioMixer => "🎚️",
            AssetKind::UILayout => "📐",
            AssetKind::UITheme => "🎭",
            AssetKind::DataTable => "📊",
            AssetKind::JsonData => "📄",
            AssetKind::ProjectConfig => "⚙️",
            AssetKind::EditorConfig => "🛠️",
        }
    }
    
    /// Get description for UI
    pub fn description(&self) -> &'static str {
        match self {
            AssetKind::TypeAlias => "Create a reusable type definition",
            AssetKind::Struct => "Create a data structure",
            AssetKind::Enum => "Create an enumeration type",
            AssetKind::Trait => "Create a trait interface",
            AssetKind::Blueprint => "Create a visual script",
            AssetKind::BlueprintClass => "Create a blueprint class",
            AssetKind::BlueprintFunction => "Create a blueprint function",
            AssetKind::RustScript => "Create a Rust code file",
            AssetKind::LuaScript => "Create a Lua script",
            AssetKind::Scene => "Create a scene",
            AssetKind::Prefab => "Create a reusable prefab",
            AssetKind::Material => "Create a material definition",
            AssetKind::Shader => "Create a WGSL shader",
            AssetKind::AudioSource => "Create an audio source",
            AssetKind::AudioMixer => "Create an audio mixer",
            AssetKind::UILayout => "Create a UI layout",
            AssetKind::UITheme => "Create a UI theme",
            AssetKind::DataTable => "Create a data table",
            AssetKind::JsonData => "Create a JSON data file",
            AssetKind::ProjectConfig => "Create project configuration",
            AssetKind::EditorConfig => "Create editor configuration",
        }
    }
    
    /// Generate a blank template for this asset type
    pub fn generate_template(&self, name: &str) -> String {
        match self {
            AssetKind::TypeAlias => {
                json!({
                    "name": name,
                    "display_name": name,
                    "description": "",
                    "ast": {
                        "nodeKind": "Primitive",
                        "name": "i32"
                    }
                }).to_string()
            }
            AssetKind::Struct => {
                json!({
                    "name": name,
                    "display_name": name,
                    "description": "",
                    "visibility": "Public",
                    "fields": []
                }).to_string()
            }
            AssetKind::Enum => {
                json!({
                    "name": name,
                    "display_name": name,
                    "description": "",
                    "visibility": "Public",
                    "variants": []
                }).to_string()
            }
            AssetKind::Trait => {
                json!({
                    "name": name,
                    "display_name": name,
                    "description": "",
                    "visibility": "Public",
                    "methods": []
                }).to_string()
            }
            AssetKind::Blueprint => {
                json!({
                    "name": name,
                    "version": "1.0.0",
                    "nodes": [],
                    "connections": []
                }).to_string()
            }
            AssetKind::BlueprintClass => {
                json!({
                    "name": name,
                    "base_class": null,
                    "variables": [],
                    "functions": []
                }).to_string()
            }
            AssetKind::BlueprintFunction => {
                json!({
                    "name": name,
                    "parameters": [],
                    "return_type": "void",
                    "nodes": []
                }).to_string()
            }
            AssetKind::RustScript => {
                format!(
                    "// {}\n\
                     // Auto-generated Rust script\n\n\
                     fn main() {{\n    \
                         tracing::debug!(\"Hello from {}\");\n\
                     }}\n",
                    name, name
                )
            }
            AssetKind::LuaScript => {
                format!(
                    "-- {}\n\
                     -- Auto-generated Lua script\n\n\
                     function init()\n    \
                         print(\"Hello from {}\")\n\
                     end\n",
                    name, name
                )
            }
            AssetKind::Scene => {
                json!({
                    "name": name,
                    "entities": [],
                    "environment": {
                        "ambient_light": [1.0, 1.0, 1.0],
                        "skybox": null
                    }
                }).to_string()
            }
            AssetKind::Prefab => {
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
                }).to_string()
            }
            AssetKind::Material => {
                json!({
                    "name": name,
                    "shader": "default",
                    "properties": {
                        "albedo": [1.0, 1.0, 1.0, 1.0],
                        "metallic": 0.0,
                        "roughness": 0.5
                    },
                    "textures": {}
                }).to_string()
            }
            AssetKind::Shader => {
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
            AssetKind::AudioSource => {
                json!({
                    "name": name,
                    "file_path": "",
                    "volume": 1.0,
                    "loop": false,
                    "spatial": false
                }).to_string()
            }
            AssetKind::AudioMixer => {
                json!({
                    "name": name,
                    "channels": [],
                    "master_volume": 1.0
                }).to_string()
            }
            AssetKind::UILayout => {
                json!({
                    "name": name,
                    "root": {
                        "type": "Container",
                        "children": []
                    }
                }).to_string()
            }
            AssetKind::UITheme => {
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
                }).to_string()
            }
            AssetKind::DataTable => {
                // SQLite database creation handled separately
                String::new()
            }
            AssetKind::JsonData => {
                json!({
                    "name": name,
                    "data": {}
                }).to_string()
            }
            AssetKind::ProjectConfig => {
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
            AssetKind::EditorConfig => {
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
    }
    
    /// Get all available asset kinds
    pub fn all() -> Vec<AssetKind> {
        vec![
            AssetKind::TypeAlias,
            AssetKind::Struct,
            AssetKind::Enum,
            AssetKind::Trait,
            AssetKind::Blueprint,
            AssetKind::BlueprintClass,
            AssetKind::BlueprintFunction,
            AssetKind::RustScript,
            AssetKind::LuaScript,
            AssetKind::Scene,
            AssetKind::Prefab,
            AssetKind::Material,
            AssetKind::Shader,
            AssetKind::AudioSource,
            AssetKind::AudioMixer,
            AssetKind::UILayout,
            AssetKind::UITheme,
            AssetKind::DataTable,
            AssetKind::JsonData,
            AssetKind::ProjectConfig,
            AssetKind::EditorConfig,
        ]
    }
    
    /// Get asset kinds by category
    pub fn by_category(category: AssetCategory) -> Vec<AssetKind> {
        match category {
            AssetCategory::TypeSystem => vec![
                AssetKind::TypeAlias,
                AssetKind::Struct,
                AssetKind::Enum,
                AssetKind::Trait,
            ],
            AssetCategory::Blueprints => vec![
                AssetKind::Blueprint,
                AssetKind::BlueprintClass,
                AssetKind::BlueprintFunction,
            ],
            AssetCategory::Scripts => vec![
                AssetKind::RustScript,
                AssetKind::LuaScript,
            ],
            AssetCategory::Scenes => vec![
                AssetKind::Scene,
                AssetKind::Prefab,
            ],
            AssetCategory::Rendering => vec![
                AssetKind::Material,
                AssetKind::Shader,
            ],
            AssetCategory::Audio => vec![
                AssetKind::AudioSource,
                AssetKind::AudioMixer,
            ],
            AssetCategory::UI => vec![
                AssetKind::UILayout,
                AssetKind::UITheme,
            ],
            AssetCategory::Data => vec![
                AssetKind::DataTable,
                AssetKind::JsonData,
            ],
            AssetCategory::Config => vec![
                AssetKind::ProjectConfig,
                AssetKind::EditorConfig,
            ],
        }
    }
}

/// Categories for organizing assets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetCategory {
    TypeSystem,
    Blueprints,
    Scripts,
    Scenes,
    Rendering,
    Audio,
    UI,
    Data,
    Config,
}

impl AssetCategory {
    pub fn display_name(&self) -> &'static str {
        match self {
            AssetCategory::TypeSystem => "Type System",
            AssetCategory::Blueprints => "Blueprints",
            AssetCategory::Scripts => "Scripts",
            AssetCategory::Scenes => "Scenes",
            AssetCategory::Rendering => "Rendering",
            AssetCategory::Audio => "Audio",
            AssetCategory::UI => "User Interface",
            AssetCategory::Data => "Data",
            AssetCategory::Config => "Configuration",
        }
    }
    
    pub fn icon(&self) -> &'static str {
        match self {
            AssetCategory::TypeSystem => "📝",
            AssetCategory::Blueprints => "🔷",
            AssetCategory::Scripts => "📜",
            AssetCategory::Scenes => "🎬",
            AssetCategory::Rendering => "🎨",
            AssetCategory::Audio => "🔊",
            AssetCategory::UI => "🖥️",
            AssetCategory::Data => "📊",
            AssetCategory::Config => "⚙️",
        }
    }
    
    pub fn all() -> Vec<AssetCategory> {
        vec![
            AssetCategory::TypeSystem,
            AssetCategory::Blueprints,
            AssetCategory::Scripts,
            AssetCategory::Scenes,
            AssetCategory::Rendering,
            AssetCategory::Audio,
            AssetCategory::UI,
            AssetCategory::Data,
            AssetCategory::Config,
        ]
    }
}
