//! Material blueprints from the images that come with an imported model.
//!
//! The editor's import pipeline knows nothing about materials. This module is
//! how the shader plugin tells it "I can help with this": an
//! [`ImportCompanion`] that, when the user accepts the offer, writes one
//! `.material` shader graph per model surface that has images (a base-colour
//! sample wired to the output, and so on), and offers each as the default
//! material of its mesh slot.

use std::path::{Path, PathBuf};

use plugin_editor_api::{
    CompanionOutput, ImportCompanion, ImportCompanionContext, ImportOffer, ImportProbe,
    SourceImageRole, SourceSurface,
};
use psgc::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};

use crate::definitions::NodeDefinitions;
use crate::io::formats::{serialize_shader_with_header, ShaderAsset};

/// The id stored with an import, so a re-import runs this companion again.
pub const COMPANION_ID: &str = "pulsar.shader.material_from_images";

const GRAPH_FILE: &str = "shader_graph_save.json";

pub struct MaterialFromImages;

/// The companion to register with the editor's import pipeline.
pub fn material_import_companion() -> std::sync::Arc<dyn ImportCompanion> {
    std::sync::Arc::new(MaterialFromImages)
}

impl ImportCompanion for MaterialFromImages {
    fn id(&self) -> &'static str {
        COMPANION_ID
    }

    fn offer(&self, probe: &ImportProbe) -> Option<ImportOffer> {
        if probe.importer != "mesh" || probe.nearby_images.is_empty() {
            return None;
        }
        let count = probe.nearby_images.len();
        Some(ImportOffer {
            label: "Generate materials from matching textures".into(),
            description: format!(
                "{count} image file{} found next to the model. A material is created for each \
                 surface that has textures, and assigned to the mesh where it has none.",
                if count == 1 { "" } else { "s" }
            ),
            default_enabled: true,
        })
    }

    fn run(&self, context: &ImportCompanionContext) -> Result<Vec<CompanionOutput>, String> {
        let stem = context
            .native_asset
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("model");
        // Beside the mesh, wherever the user keeps it: nesting is theirs to do.
        let folder = context.native_asset.parent().unwrap_or(context.project_root);
        // The name must not depend on how many surfaces have images, or a
        // re-import after adding a texture would make a second set.
        let single = context.surfaces.len() == 1;

        let mut outputs = Vec::new();
        let mut used_names = Vec::<String>::new();
        for surface in context.surfaces.iter().filter(|s| !s.images.is_empty()) {
            let name = if single {
                stem.to_owned()
            } else {
                format!("{stem}_{}", file_safe_name(surface))
            };
            let name = unique_name(name, &used_names);
            used_names.push(name.clone());
            let material = folder.join(format!("{name}.material"));
            // A material that exists may have been edited: keep it, and still
            // report it so the import keeps owning (and assigning) it.
            if !material.join(GRAPH_FILE).exists() {
                write_material(&material, folder, surface, context.project_root)?;
            }
            outputs.push(CompanionOutput {
                path: material,
                kind: "material".into(),
                source_name: Some(surface.name.clone()),
                assign_to_slot: Some(surface.slot),
            });
        }
        Ok(outputs)
    }
}

fn file_safe_name(surface: &SourceSurface) -> String {
    let cleaned: String = surface
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    if cleaned.is_empty() {
        format!("Material{}", surface.slot + 1)
    } else {
        cleaned.to_owned()
    }
}

fn unique_name(name: String, used: &[String]) -> String {
    let mut candidate = name.clone();
    let mut n = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&candidate)) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    candidate
}

fn write_material(
    material: &Path,
    folder: &Path,
    surface: &SourceSurface,
    project_root: &Path,
) -> Result<(), String> {
    let graph = build_graph(surface, |image| texture_reference(image, folder, project_root))?;
    let asset = ShaderAsset::with_graph(graph);
    let text = serialize_shader_with_header(&asset)?;
    std::fs::create_dir_all(material)
        .and_then(|()| std::fs::write(material.join(GRAPH_FILE), text))
        .map_err(|error| format!("write {}: {error}", material.display()))
}

/// How the graph names `image`: project-relative when it is in the project,
/// otherwise a copy in `folder`, beside the mesh (a material can only sample
/// project files). An existing file of that name is left alone.
fn texture_reference(image: &Path, folder: &Path, project_root: &Path) -> Result<String, String> {
    let inside = if image.starts_with(project_root) {
        image.to_path_buf()
    } else {
        let copy = folder.join(image.file_name().ok_or("image has no file name")?);
        if !copy.exists() {
            std::fs::copy(image, &copy)
                .map_err(|error| format!("copy {}: {error}", image.display()))?;
        }
        copy
    };
    Ok(inside
        .strip_prefix(project_root)
        .unwrap_or(&inside)
        .to_string_lossy()
        .replace('\\', "/"))
}

fn node(id: &str, kind: &str, x: f64, y: f64) -> Result<NodeInstance, String> {
    let definition = NodeDefinitions::load()
        .get_node_definition(kind)
        .ok_or_else(|| format!("shader node '{kind}' is not available"))?;
    let pin_type = |pin: &crate::definitions::PinDefinition| pin.data_type.clone();
    let mut instance = NodeInstance::new(id, kind, Position { x, y });
    for input in &definition.inputs {
        instance.inputs.push(PinInstance::new(
            input.id.clone(),
            Pin::new(input.id.clone(), input.name.clone(), pin_type(input), PinType::Input),
        ));
    }
    for output in &definition.outputs {
        instance.outputs.push(PinInstance::new(
            output.id.clone(),
            Pin::new(output.id.clone(), output.name.clone(), pin_type(output), PinType::Output),
        ));
    }
    Ok(instance)
}

fn connect(graph: &mut GraphDescription, from: &str, to: &str, pin: &str) {
    graph.add_connection(Connection::new(from, "result", to, pin, ConnectionType::Data));
}

/// One image sample per image, wired to the output pin its role feeds. A
/// normal map goes through a `normal_map` node first: the output's normal pin
/// takes a world-space `vec3`, and the sample is a tangent-space `vec4`.
fn build_graph(
    surface: &SourceSurface,
    mut reference: impl FnMut(&Path) -> Result<String, String>,
) -> Result<GraphDescription, String> {
    let mut graph = GraphDescription::new(&surface.name);
    graph.add_node(node("output", "fragment_output", 900.0, 200.0)?);
    graph.add_node(node("uv", "frag_uv", 0.0, 200.0)?);

    for (row, image) in surface.images.iter().enumerate() {
        let y = row as f64 * 220.0;
        let sample = format!("sample_{row}");
        let mut sampler = node(&sample, "sample_texture", 300.0, y)?;
        sampler
            .properties
            .insert("texture".into(), serde_json::json!(reference(&image.path)?));
        graph.add_node(sampler);
        connect(&mut graph, "uv", &sample, "uv");

        // (channel node, output pin) pairs fed by this image.
        let feeds: &[(Option<&str>, &str)] = match image.role {
            SourceImageRole::BaseColor => &[(None, "base_color")],
            SourceImageRole::Emissive => &[(None, "emissive_color")],
            SourceImageRole::MetallicRoughness => {
                &[(Some("component_g"), "roughness"), (Some("component_b"), "metallic")]
            }
            SourceImageRole::Roughness => &[(Some("component_r"), "roughness")],
            SourceImageRole::Metallic => &[(Some("component_r"), "metallic")],
            SourceImageRole::Occlusion => &[(Some("component_r"), "ambient_occlusion")],
            SourceImageRole::Normal => &[(None, "normal")],
        };
        for (channel, pin) in feeds {
            feed_pin(&mut graph, &sample, *channel, pin, row, y)?;
        }
    }
    Ok(graph)
}

/// The WGSL type of the output pin `pin` of the shader node `kind`.
fn param_type(kind: &str, pin: &str) -> Option<String> {
    psgc::get_shader_nodes()
        .into_iter()
        .find(|meta| meta.name == kind)?
        .params
        .into_iter()
        .find(|param| param.name == pin)
        .map(|param| param.param_type)
}

/// The WGSL type the shader node `kind` returns.
fn return_type(kind: &str) -> Option<String> {
    psgc::get_shader_nodes()
        .into_iter()
        .find(|meta| meta.name == kind)?
        .return_type
        .map(|ty| ty.type_string)
}

/// The conversion node that turns a `from` value into a `to` value, with the
/// name of its input pin.
fn conversion_node(from: &str, to: &str) -> Option<(String, String)> {
    psgc::get_shader_nodes().into_iter().find_map(|meta| {
        let conversion = meta.conversion.as_ref()?;
        (conversion.from_type.type_string == from && conversion.to_type.type_string == to)
            .then(|| Some((meta.name.clone(), meta.params.first()?.name.clone())))
            .flatten()
    })
}

/// Feed an image sample to the output pin `pin`, adding whatever nodes the
/// pin's type needs. The sample is a `vec4`; the importer reads the type the
/// pin declares and builds the path to it:
///
/// - a channel pick when the role names a channel (`vec4` to `f32`);
/// - a `normal_map` decode for the `normal` pin, which takes a world-space
///   `vec3` and not a tangent-space texel;
/// - the registered conversion node for any other mismatch.
///
/// A pin with no way to get its type from the sample is an error, so the
/// importer never writes a graph whose connection the compiler would reject.
fn feed_pin(
    graph: &mut GraphDescription,
    sample: &str,
    channel: Option<&str>,
    pin: &str,
    row: usize,
    y: f64,
) -> Result<(), String> {
    let target = param_type("fragment_output", pin)
        .ok_or_else(|| format!("the material output has no '{pin}' pin"))?;
    let mut from = sample.to_string();
    let mut current = return_type("sample_texture").ok_or("sample_texture has no type")?;

    if let Some(kind) = channel {
        let id = format!("{kind}_{row}_{pin}");
        graph.add_node(node(&id, kind, 600.0, y)?);
        connect(graph, &from, &id, "vec");
        current = return_type(kind).ok_or_else(|| format!("{kind} has no type"))?;
        from = id;
    }

    if pin == "normal" && current == "vec4<f32>" {
        // The decode builds a tangent frame from the surface normal and world
        // position; every normal map shares those two inputs.
        for (id, kind) in [("normal_in", "frag_normal"), ("position_in", "world_position")] {
            if !graph.nodes.contains_key(id) {
                graph.add_node(node(id, kind, 300.0, y + 110.0)?);
            }
        }
        let decode = format!("normal_map_{row}");
        graph.add_node(node(&decode, "normal_map", 600.0, y)?);
        connect(graph, &from, &decode, "texel");
        connect(graph, "uv", &decode, "uv");
        connect(graph, "normal_in", &decode, "normal");
        connect(graph, "position_in", &decode, "position");
        current = return_type("normal_map").ok_or("normal_map has no type")?;
        from = decode;
    }

    if current != target {
        let (kind, input) = conversion_node(&current, &target)
            .ok_or_else(|| format!("no conversion from {current} to {target} for '{pin}'"))?;
        let id = format!("{kind}_{row}_{pin}");
        graph.add_node(node(&id, &kind, 750.0, y)?);
        connect(graph, &from, &id, &input);
        from = id;
    }
    connect(graph, &from, "output", pin);
    Ok(())
}


#[cfg(test)]
mod tests {
    /// A scratch directory removed on drop (the crate has no tempfile dependency,
    /// so its tests build from the root workspace too).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("shader-import-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    use super::*;
    use crate::io::formats::deserialize_shader;
    use plugin_editor_api::SourceImage;

    fn surface(roles: &[(SourceImageRole, &str)]) -> SourceSurface {
        SourceSurface {
            slot: 1,
            name: "Rock Wall".into(),
            images: roles
                .iter()
                .map(|(role, file)| SourceImage {
                    role: *role,
                    path: PathBuf::from(file),
                })
                .collect(),
        }
    }

    #[test]
    fn roles_feed_their_output_pins() {
        let graph = build_graph(
            &surface(&[
                (SourceImageRole::BaseColor, "a.png"),
                (SourceImageRole::MetallicRoughness, "mr.png"),
                (SourceImageRole::Normal, "n.png"),
            ]),
            |path| Ok(path.to_string_lossy().into_owned()),
        )
        .unwrap();
        let into_output = |pin: &str| {
            graph
                .connections
                .iter()
                .any(|c| c.target_node == "output" && c.target_pin == pin)
        };
        assert!(into_output("base_color") && into_output("roughness") && into_output("metallic"));
        // The normal map is decoded to a world-space vec3 and feeds `normal`.
        assert!(into_output("normal"));
        assert_eq!(graph.nodes["normal_map_2"].node_type, "normal_map");
        let feeds = |from: &str, pin: &str| {
            graph.connections.iter().any(|c| {
                c.source_node == from && c.target_node == "normal_map_2" && c.target_pin == pin
            })
        };
        assert!(feeds("sample_2", "texel") && feeds("uv", "uv"));
        assert!(feeds("normal_in", "normal") && feeds("position_in", "position"));
        assert!(graph
            .connections
            .iter()
            .all(|c| !(c.source_node == "sample_2" && c.target_node == "output")));
        assert_eq!(
            graph.nodes["sample_1"].properties["texture"],
            serde_json::json!("mr.png")
        );
    }


    #[test]
    fn every_connection_matches_the_type_its_pin_declares() {
        let graph = build_graph(
            &surface(&[
                (SourceImageRole::BaseColor, "a.png"),
                (SourceImageRole::MetallicRoughness, "mr.png"),
                (SourceImageRole::Occlusion, "o.png"),
                (SourceImageRole::Emissive, "e.png"),
                (SourceImageRole::Normal, "n.png"),
            ]),
            |path| Ok(path.to_string_lossy().into_owned()),
        )
        .unwrap();
        for connection in graph.connections.iter().filter(|c| c.target_node == "output") {
            let source = &graph.nodes[&connection.source_node].node_type;
            assert_eq!(
                return_type(source),
                param_type("fragment_output", &connection.target_pin),
                "{} ({source}) into {}",
                connection.source_node,
                connection.target_pin
            );
        }
    }

    #[test]
    fn a_mismatched_pin_gets_the_registered_conversion_and_an_impossible_one_is_an_error() {
        assert_eq!(
            conversion_node("vec4<f32>", "vec3<f32>"),
            Some(("vec4_to_vec3".to_string(), "input".to_string()))
        );
        assert!(conversion_node("vec4<f32>", "TextureSrc").is_none());

        // `normal_in` is a vec3 into a vec4 pin: one conversion node between.
        let mut graph = GraphDescription::new("g");
        graph.add_node(node("output", "fragment_output", 0.0, 0.0).unwrap());
        graph.add_node(node("normal_in", "frag_normal", 0.0, 0.0).unwrap());
        let (kind, input) = conversion_node("vec3<f32>", "vec4<f32>").unwrap();
        assert_eq!(kind, "vec3_to_vec4");
        assert_eq!(input, "input");
        assert!(feed_pin(&mut graph, "sample_0", None, "no_such_pin", 0, 0.0).is_err());
    }

    #[test]
    fn run_writes_a_loadable_material_once_and_keeps_edits() {
        let dir = TempDir::new();
        let root = dir.path();
        std::fs::write(root.join("rock.png"), b"png").unwrap();
        let native = root.join("rock.mesh");
        let surfaces = vec![
            surface(&[(SourceImageRole::BaseColor, root.join("rock.png").to_str().unwrap())]),
            SourceSurface { slot: 0, name: "Bare".into(), images: vec![] },
        ];
        let context = ImportCompanionContext {
            project_root: root,
            source: &root.join("rock.fbx"),
            native_asset: &native,
            surfaces: &surfaces,
        };
        let outputs = MaterialFromImages.run(&context).unwrap();
        assert_eq!(outputs.len(), 1, "only surfaces with images get a material");
        assert_eq!(outputs[0].assign_to_slot, Some(1));
        // Flat: beside the mesh, named for the model and surface.
        assert_eq!(outputs[0].path, root.join("rock_Rock_Wall.material"));
        let graph_file = outputs[0].path.join(GRAPH_FILE);
        let written = std::fs::read_to_string(&graph_file).unwrap();
        let asset = deserialize_shader(&written).unwrap();
        assert_eq!(
            asset.main_graph.nodes["sample_0"].properties["texture"],
            serde_json::json!("rock.png")
        );

        std::fs::write(&graph_file, "edited").unwrap();
        MaterialFromImages.run(&context).unwrap();
        assert_eq!(std::fs::read_to_string(&graph_file).unwrap(), "edited");
    }

    #[test]
    fn a_single_surface_model_gets_one_material_named_for_the_model_and_images_outside_the_project_are_copied_flat(
    ) {
        let outside = TempDir::new();
        let project = TempDir::new();
        let root = project.path();
        let sub = root.join("props");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(outside.path().join("a_BaseColor.jpg"), b"jpg").unwrap();
        let surfaces = vec![SourceSurface {
            slot: 0,
            name: "Material.001".into(),
            images: vec![plugin_editor_api::SourceImage {
                role: SourceImageRole::BaseColor,
                path: outside.path().join("a_BaseColor.jpg"),
            }],
        }];
        let context = ImportCompanionContext {
            project_root: root,
            source: &sub.join("crate.fbx"),
            native_asset: &sub.join("crate.mesh"),
            surfaces: &surfaces,
        };
        let outputs = MaterialFromImages.run(&context).unwrap();
        assert_eq!(outputs[0].path, sub.join("crate.material"));
        assert!(sub.join("a_BaseColor.jpg").exists());
        let written = std::fs::read_to_string(sub.join("crate.material").join(GRAPH_FILE)).unwrap();
        assert!(written.contains("props/a_BaseColor.jpg"));
    }

    #[test]
    fn offer_needs_the_mesh_importer_and_nearby_images() {
        let sources = [PathBuf::from("a.fbx")];
        let probe = |importer, nearby: &[PathBuf]| {
            MaterialFromImages
                .offer(&ImportProbe { importer, sources: &sources, nearby_images: nearby })
                .is_some()
        };
        assert!(probe("mesh", &[PathBuf::from("x.png")]));
        assert!(!probe("mesh", &[]));
        assert!(!probe("other", &[PathBuf::from("x.png")]));
    }
}
