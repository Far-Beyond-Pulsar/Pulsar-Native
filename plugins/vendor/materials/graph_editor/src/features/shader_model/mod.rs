//! Shader model system - registerable shader output models
//!
//! A shader model defines what input pins a material graph exposes
//! (e.g., Base Color, Metallic, Roughness, Normal) and how they
//! are assembled into the final shader output.

pub mod standard;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

/// A registered shader model definition
#[derive(Clone, Debug)]
pub struct ShaderModel {
    /// Unique identifier (e.g. "Standard_unlit", "Standard_pbr")
    pub id: String,
    /// Display name for the UI
    pub name: String,
    /// Description of what this model does
    pub description: String,
    /// Input pin definitions the material graph exposes
    pub inputs: Vec<ShaderModelInput>,
    /// WGSL code template for the fragment shader body.
    /// Use {pin_name} placeholders that get replaced with the resolved variable names.
    pub fragment_template: String,
}

/// A single input pin on a shader model
#[derive(Clone, Debug)]
pub struct ShaderModelInput {
    pub id: String,
    pub name: String,
    pub data_type: String,
    pub default_value: String,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, ShaderModel>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, ShaderModel>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(model: ShaderModel) {
    registry().lock().unwrap().insert(model.id.clone(), model);
}

pub fn get_model(id: &str) -> Option<ShaderModel> {
    registry().lock().unwrap().get(id).cloned()
}

pub fn get_all_models() -> Vec<ShaderModel> {
    registry().lock().unwrap().values().cloned().collect()
}

pub fn get_default_inputs(model_id: &str) -> Vec<ShaderModelInput> {
    get_model(model_id).map(|m| m.inputs).unwrap_or_default()
}
