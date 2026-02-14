//! Asset Templates
//!
//! Provides templates for creating new assets of any type

use serde_json::json;

/// Metadata for an asset kind
struct AssetMetadata {
    extension: &'static str,
    directory: &'static str,
    display_name: &'static str,
    icon: &'static str,
    description: &'static str,
}

macro_rules! define_asset_kinds {
    (
        $(
            $variant:ident {
                ext: $ext:expr,
                dir: $dir:expr,
                name: $name:expr,
                icon: $icon:expr,
                desc: $desc:expr
                $(,)?
            }
        ),* $(,)?
    ) => {
        /// All possible asset types that can be created
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum AssetKind {
            $($variant,)*
        }

        impl AssetKind {
            fn metadata(&self) -> &'static AssetMetadata {
                match self {
                    $(
                        AssetKind::$variant => &AssetMetadata {
                            extension: $ext,
                            directory: $dir,
                            display_name: $name,
                            icon: $icon,
                            description: $desc,
                        },
                    )*
                }
            }
        }
    };
}

define_asset_kinds! {
    // Type System
    TypeAlias { ext: "alias.json", dir: "types/aliases", name: "Type Alias", icon: "🔗", desc: "Create a reusable type definition" },
    Struct { ext: "struct.json", dir: "types/structs", name: "Struct", icon: "📦", desc: "Create a data structure" },
    Enum { ext: "enum.json", dir: "types/enums", name: "Enum", icon: "🎯", desc: "Create an enumeration type" },
    Trait { ext: "trait.json", dir: "types/traits", name: "Trait", icon: "🔧", desc: "Create a trait interface" },
    
    // Blueprint System
    Blueprint { ext: "blueprint.json", dir: "blueprints", name: "Blueprint", icon: "🔷", desc: "Create a visual script" },
    BlueprintClass { ext: "bpclass.json", dir: "blueprints/classes", name: "Blueprint Class", icon: "📘", desc: "Create a blueprint class" },
    BlueprintFunction { ext: "bpfunc.json", dir: "blueprints/functions", name: "Blueprint Function", icon: "⚡", desc: "Create a blueprint function" },
    
    // Scripts
    RustScript { ext: "rs", dir: "scripts/rust", name: "Rust Script", icon: "🦀", desc: "Create a Rust code file" },
    LuaScript { ext: "lua", dir: "scripts/lua", name: "Lua Script", icon: "🌙", desc: "Create a Lua script" },
    
    // Scenes
    Scene { ext: "scene.json", dir: "scenes", name: "Scene", icon: "🎬", desc: "Create a scene" },
    Prefab { ext: "prefab.json", dir: "prefabs", name: "Prefab", icon: "🎁", desc: "Create a reusable prefab" },
    
    // Materials & Shaders
    Material { ext: "mat.json", dir: "materials", name: "Material", icon: "🎨", desc: "Create a material definition" },
    Shader { ext: "shader.wgsl", dir: "shaders", name: "Shader", icon: "✨", desc: "Create a WGSL shader" },
    
    // Audio
    AudioSource { ext: "audio.json", dir: "audio/sources", name: "Audio Source", icon: "🔊", desc: "Create an audio source" },
    AudioMixer { ext: "mixer.json", dir: "audio/mixers", name: "Audio Mixer", icon: "🎚️", desc: "Create an audio mixer" },
    
    // UI
    UILayout { ext: "ui.json", dir: "ui/layouts", name: "UI Layout", icon: "📐", desc: "Create a UI layout" },
    UITheme { ext: "theme.json", dir: "ui/themes", name: "UI Theme", icon: "🎭", desc: "Create a UI theme" },
    
    // Data
    DataTable { ext: "table.db", dir: "data/tables", name: "Data Table", icon: "📊", desc: "Create a data table" },
    JsonData { ext: "json", dir: "data", name: "JSON Data", icon: "📄", desc: "Create a JSON data file" },
    
    // Config
    ProjectConfig { ext: "project.toml", dir: "config", name: "Project Config", icon: "⚙️", desc: "Create project configuration" },
    EditorConfig { ext: "editor.toml", dir: "config", name: "Editor Config", icon: "🛠️", desc: "Create editor configuration" },
}

impl AssetKind {
    /// Get the file extension for this asset type
    pub fn extension(&self) -> &'static str {
        self.metadata().extension
    }
    
    /// Get the default subdirectory for this asset type
    pub fn default_directory(&self) -> &'static str {
        self.metadata().directory
    }
    
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        self.metadata().display_name
    }
    
    /// Get icon for UI
    pub fn icon(&self) -> &'static str {
        self.metadata().icon
    }
    
    /// Get description for UI
    pub fn description(&self) -> &'static str {
        self.metadata().description
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
