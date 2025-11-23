//! Registry integration for Script Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Script Editor
#[derive(Clone)]
pub struct ScriptEditorType;

impl EditorType for ScriptEditorType {
    fn editor_id(&self) -> &'static str {
        "script_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Script Editor"
    }
    
    fn icon(&self) -> &'static str {
        "📄"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type for Rust Script files
#[derive(Clone)]
pub struct RustScriptAssetType;

impl AssetType for RustScriptAssetType {
    fn type_id(&self) -> &'static str {
        "rust_script"
    }
    
    fn display_name(&self) -> &'static str {
        "Rust Script"
    }
    
    fn icon(&self) -> &'static str {
        "🦀"
    }
    
    fn description(&self) -> &'static str {
        "Rust source code file"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &[".rs"]
    }
    
    fn default_directory(&self) -> &'static str {
        "scripts/rust"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Scripts
    }
    
    fn generate_template(&self, name: &str) -> String {
        format!(r#"// {}

pub fn main() {{
    println!("Hello from {}!");
}}
"#, name, name)
    }
    
    fn editor_id(&self) -> &'static str {
        "script_editor"
    }
}

/// Asset type for Lua Script files
#[derive(Clone)]
pub struct LuaScriptAssetType;

impl AssetType for LuaScriptAssetType {
    fn type_id(&self) -> &'static str {
        "lua_script"
    }
    
    fn display_name(&self) -> &'static str {
        "Lua Script"
    }
    
    fn icon(&self) -> &'static str {
        "🌙"
    }
    
    fn description(&self) -> &'static str {
        "Lua script file"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &[".lua"]
    }
    
    fn default_directory(&self) -> &'static str {
        "scripts/lua"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Scripts
    }
    
    fn generate_template(&self, name: &str) -> String {
        format!(r#"-- {}

function main()
    print("Hello from {}!")
end
"#, name, name)
    }
    
    fn editor_id(&self) -> &'static str {
        "script_editor"
    }
}

/// Asset type for Shader files
#[derive(Clone)]
pub struct ShaderAssetType;

impl AssetType for ShaderAssetType {
    fn type_id(&self) -> &'static str {
        "shader"
    }
    
    fn display_name(&self) -> &'static str {
        "Shader"
    }
    
    fn icon(&self) -> &'static str {
        "✨"
    }
    
    fn description(&self) -> &'static str {
        "GLSL/WGSL shader file"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &[".wgsl", ".glsl", ".vert", ".frag"]
    }
    
    fn default_directory(&self) -> &'static str {
        "shaders"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Scripts
    }
    
    fn generate_template(&self, name: &str) -> String {
        format!(r#"// {}

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}}

@fragment
fn fs_main() -> @location(0) vec4<f32> {{
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}}
"#, name)
    }
    
    fn editor_id(&self) -> &'static str {
        "script_editor"
    }
}
