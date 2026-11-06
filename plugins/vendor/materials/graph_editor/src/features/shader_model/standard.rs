//! Standard Unlit and PBR shader models
//!
//! Defines the built-in shader models that ship with the editor.

use super::{ShaderModel, ShaderModelInput, register};

pub fn register_standard_models() {
    register(ShaderModel {
        id: "Standard_unlit".to_string(),
        name: "Standard Unlit".to_string(),
        description: "Simple unlit shader with emissive color output".to_string(),
        inputs: vec![ShaderModelInput {
            id: "emissive_color".to_string(),
            name: "Emissive Color".to_string(),
            data_type: "vec4<f32>".to_string(),
            default_value: "vec4<f32>(1.0, 1.0, 1.0, 1.0)".to_string(),
        }],
        fragment_template: r#"
    let emissive = {emissive_color};
    return emissive;
"#
        .to_string(),
    });

    register(ShaderModel {
        id: "Standard_pbr".to_string(),
        name: "Standard PBR".to_string(),
        description:
            "Physically-based shader with base color, metallic, roughness, and normal".to_string(),
        inputs: vec![
            ShaderModelInput {
                id: "base_color".to_string(),
                name: "Base Color".to_string(),
                data_type: "vec4<f32>".to_string(),
                default_value: "vec4<f32>(1.0, 1.0, 1.0, 1.0)".to_string(),
            },
            ShaderModelInput {
                id: "metallic".to_string(),
                name: "Metallic".to_string(),
                data_type: "f32".to_string(),
                default_value: "0.0".to_string(),
            },
            ShaderModelInput {
                id: "roughness".to_string(),
                name: "Roughness".to_string(),
                data_type: "f32".to_string(),
                default_value: "0.5".to_string(),
            },
            ShaderModelInput {
                id: "normal".to_string(),
                name: "Normal".to_string(),
                data_type: "vec3<f32>".to_string(),
                default_value: "vec3<f32>(0.0, 0.0, 1.0)".to_string(),
            },
            ShaderModelInput {
                id: "emissive".to_string(),
                name: "Emissive".to_string(),
                data_type: "vec3<f32>".to_string(),
                default_value: "vec3<f32>(0.0, 0.0, 0.0)".to_string(),
            },
        ],
        fragment_template: r#"
    let base = {base_color};
    let metal = {metallic};
    let rough = {roughness};
    let norm = {normal};
    let emiss = {emissive};

    let n = normalize(norm);
    let light_dir = normalize(vec3<f32>(1.0, 1.0, 1.0));
    let diffuse = max(dot(n, light_dir), 0.0);
    let ambient = 0.3;

    return vec4<f32>(emiss + base.rgb * (ambient + diffuse * (1.0 - metal)), base.a);
"#
        .to_string(),
    });
}
