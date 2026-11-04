use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::material::{
    MATERIAL_CLASS_ANISOTROPIC, MATERIAL_CLASS_CLEAR_COAT, MATERIAL_CLASS_FOLIAGE,
    MATERIAL_CLASS_SKIN, MATERIAL_CLASS_SUBSURFACE,
};

/// A material class's shading: its surface function (`radiant_eval_surface`
/// or, for the transparent drawers, `radiant_eval_transparent`) and the
/// helpers beside it, which a drawer composes into its own program
/// ([`Self::compose`]).
pub struct RadiantTemplate {
    pub name: Arc<str>,
    /// The surface function's WGSL, with `// RADIANT_OVERRIDE_SURFACE` /
    /// `// RADIANT_OVERRIDE_END` around what a material graph replaces.
    pub wgsl_source: Arc<str>,
}

impl Clone for RadiantTemplate {
    fn clone(&self) -> Self {
        Self {
            // Both fields are immutable for the lifetime of a template.  They
            // are already static (built-ins and registered strings), so a clone
            // can copy the references directly.  Allocating/leaking here used
            // to make every registry clone permanently grow the process heap.
            name: Arc::clone(&self.name),
            wgsl_source: Arc::clone(&self.wgsl_source),
        }
    }
}

impl RadiantTemplate {
    /// This template's surface in a drawer's `program`: the WGSL a drawer of
    /// any kind compiles to shade its surfaces with this material class.
    ///
    /// A template is only its surface function (`radiant_eval_surface`, or
    /// `radiant_eval_transparent` for the transparent drawers) and the
    /// helpers beside it, over [`crate::MATERIAL_SNIPPETS`]' declarations
    /// (`GpuMaterial`, `MaterialInput`, `SurfaceData`, `default_pbr_surface`).
    /// The program is the drawer's: its vertex layout, rows and bindings and
    /// the entry points that build a `MaterialInput` and write what the
    /// template returns. `snippets` are the program's other snippets (a
    /// kind's rows, the lights' functions). A non-empty `graph_wgsl` (a
    /// material graph, `/*RADIANT_GRAPH_DECLARATIONS*/` then
    /// `/*RADIANT_GRAPH_BODY*/`) replaces the template's override section.
    pub fn compose(
        &self,
        program: &str,
        graph_wgsl: &str,
        max_textures: usize,
        snippets: &[helio_core::shader::ShaderSnippet],
    ) -> String {
        let max_tex_str = max_textures.to_string();
        let joined = format!("{program}\n{}\n", self.wgsl_source);
        let mut all = crate::MATERIAL_SNIPPETS.to_vec();
        all.extend_from_slice(snippets);
        #[allow(unused_mut)]
        let mut src = helio_core::shader::resolve_with(&joined, &all)
            .replace(
                "binding_array<texture_2d<f32>, 256>",
                &format!("binding_array<texture_2d<f32>, {max_tex_str}>"),
            )
            .replace(
                "binding_array<sampler, 256>",
                &format!("binding_array<sampler, {max_tex_str}>"),
            );

        // wgpu's Naga requires `enable wgpu_binding_array;` for binding_array
        // support.  Browser WebGPU (Chrome's Tint) does not recognise this
        // extension — it has binding_array support in core WGSL.  Strip it for
        // wasm builds, keep it for native wgpu.
        #[cfg(target_arch = "wasm32")]
        {
            src = src.replace("enable wgpu_binding_array;\n", "");
            src = src.replace("enable wgpu_binding_array;\r\n", "");
        }

        if graph_wgsl.is_empty() {
            // No graph: remove the override markers, leaving the default code
            src.replace("// RADIANT_OVERRIDE_SURFACE\n", "")
                .replace("// RADIANT_OVERRIDE_END\n", "")
        } else {
            // Graph materials carry module-scope declarations separately from
            // the surface assignments injected into radiant_eval_surface.
            // PSGC emits a complete fragment module; the runtime adapter turns
            // that into this small two-part representation.
            let (graph_declarations, graph_body) = graph_wgsl
                .split_once("\n/*RADIANT_GRAPH_BODY*/\n")
                .map(|(declarations, body)| {
                    (
                        declarations
                            .strip_prefix("/*RADIANT_GRAPH_DECLARATIONS*/\n")
                            .unwrap_or(declarations),
                        body,
                    )
                })
                .unwrap_or(("", graph_wgsl));
            // Graph `time` nodes read `radiant_graph_time()`. A template whose
            // `Globals` carries the shared clock (tagged `RADIANT_GLOBALS_TIME`)
            // reads it from there; any other template is frozen at 0, which is
            // always valid WGSL, rather than failing to compile.
            let time_fn = if src.contains("RADIANT_GLOBALS_TIME") {
                "fn radiant_graph_time() -> f32 { return globals.time; }\n"
            } else {
                "fn radiant_graph_time() -> f32 { return 0.0; }\n"
            };
            let graph_declarations = format!("{time_fn}{graph_declarations}");
            let src = if let Some(at) = src.find("fn radiant_eval_surface") {
                format!("{}{}\n{}", &src[..at], graph_declarations, &src[at..])
            } else {
                src
            };
            // Graph present: replace everything from OVERRIDE_SURFACE to OVERRIDE_END
            // with the graph's override code
            let override_start = "// RADIANT_OVERRIDE_SURFACE";
            let override_end = "// RADIANT_OVERRIDE_END";
            if let Some(start) = src.find(override_start) {
                if let Some(end) = src.find(override_end) {
                    let before = &src[..start];
                    let after = &src[end + override_end.len()..];
                    format!("{}{}\n{}", before, graph_body, after)
                } else {
                    src
                }
            } else {
                src
            }
        }
    }

    /// Replace native binding arrays with baseline-WebGPU individual bindings.
    ///
    /// WebGPU does not expose wgpu's `binding_array` WGSL extension. Individual
    /// texture/sampler bindings plus an explicit switch retain all material slots
    /// without requiring a native-only feature.
    pub fn apply_webgpu_fixups(src: &str, max_textures: usize) -> String {
        crate::shader::apply_webgpu_material_bindings(src, max_textures)
    }
}

/// Built-in templates shipped with the engine.
pub struct RadiantTemplateRegistry {
    templates: HashMap<u32, RadiantTemplate>,
    next_id: u32,
    dynamic_ids: VecDeque<u32>,
}

const MAX_DYNAMIC_TEMPLATES: usize = 1024;

impl Clone for RadiantTemplateRegistry {
    fn clone(&self) -> Self {
        Self {
            templates: self.templates.clone(),
            next_id: self.next_id,
            dynamic_ids: self.dynamic_ids.clone(),
        }
    }
}

/// A cheaply-clonable, thread-safe handle to a registry that the renderer
/// and its passes share instead of each keeping their own copy.
///
/// A shared registry avoids copying the template map when it is handed to
/// multiple renderer passes. Template clones themselves only copy static
/// references and do not allocate.
pub type SharedTemplateRegistry = std::sync::Arc<std::sync::RwLock<RadiantTemplateRegistry>>;

impl RadiantTemplateRegistry {
    /// The transparent drawers' registry: class 0 is the default
    /// transparent surface ([`crate::templates::TRANSPARENT_DEFAULT`]).
    pub fn new_transparent() -> Self {
        let mut reg = Self::new_empty();
        reg.templates.insert(
            0,
            RadiantTemplate {
                name: Arc::from("transparent_default"),
                wgsl_source: Arc::from(crate::templates::TRANSPARENT_DEFAULT),
            },
        );
        reg
    }

    /// Create an empty registry (no built-in templates).
    pub fn new_empty() -> Self {
        Self {
            templates: HashMap::new(),
            next_id: crate::FIRST_CUSTOM_MATERIAL_CLASS,
            dynamic_ids: VecDeque::new(),
        }
    }

    pub fn new() -> Self {
        let mut reg = Self {
            templates: HashMap::new(),
            // Custom templates start past the built-ins:
            //   0 = default_pbr, 1 = clear_coat, 2 = subsurface,
            //   3 = anisotropic, 4 = skin, 5 = foliage
            next_id: crate::FIRST_CUSTOM_MATERIAL_CLASS,
            dynamic_ids: VecDeque::new(),
        };
        reg.templates.insert(
            0,
            RadiantTemplate {
                name: Arc::from("default_pbr"),
                wgsl_source: Arc::from(crate::templates::DEFAULT_SURFACE),
            },
        );
        reg.register_default_templates();
        reg
    }

    /// Register the built-in tier-2 surface templates shipped with the engine.
    /// Each template is registered with its predefined `MATERIAL_CLASS_*` ID
    /// so users can reference them by the constants from `helio_mats`.
    fn register_default_templates(&mut self) {
        for (class, name, source) in [
            (
                MATERIAL_CLASS_CLEAR_COAT,
                "clear_coat",
                crate::templates::CLEAR_COAT,
            ),
            (
                MATERIAL_CLASS_SUBSURFACE,
                "subsurface",
                crate::templates::SUBSURFACE,
            ),
            (
                MATERIAL_CLASS_ANISOTROPIC,
                "anisotropic",
                crate::templates::ANISOTROPIC,
            ),
            (MATERIAL_CLASS_SKIN, "skin", crate::templates::SKIN),
            (MATERIAL_CLASS_FOLIAGE, "foliage", crate::templates::FOLIAGE),
        ] {
            self.register_str_at(class, name, source.to_string());
        }
    }

    pub fn get(&self, class: u32) -> Option<&RadiantTemplate> {
        self.templates.get(&class)
    }

    pub fn keys(&self) -> Vec<u32> {
        self.templates.keys().copied().collect()
    }

    /// Iterate over all (class_id, template) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &RadiantTemplate)> {
        self.templates.iter()
    }

    pub fn register(&mut self, class: u32, template: RadiantTemplate) {
        self.templates.insert(class, template);
    }

    /// Override an existing class with a new surface source.
    pub fn override_class(&mut self, class: u32, name: &str, wgsl_source: &str) {
        self.templates.insert(
            class,
            RadiantTemplate {
                name: Arc::from(name),
                wgsl_source: Arc::from(wgsl_source),
            },
        );
    }

    /// Load a template (a surface function, see [`RadiantTemplate::compose`])
    /// from a WGSL file on disk. Returns the assigned template_id.
    pub fn load_from_file(&mut self, path: &std::path::Path) -> std::io::Result<u32> {
        let source = std::fs::read_to_string(path)?;
        Ok(self.register_str(
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            source,
        ))
    }

    /// Register a template (a surface function and its helpers, see
    /// [`RadiantTemplate::compose`]) from a string.
    pub fn register_str(&mut self, name: &str, wgsl_source: String) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        log::info!("[Radiant] register_str '{}' → class {}", name, id);
        self.templates.insert(
            id,
            RadiantTemplate {
                name: Arc::from(format!("Radiant:{}", name)),
                wgsl_source: Arc::from(wgsl_source),
            },
        );
        self.dynamic_ids.push_back(id);
        while self.dynamic_ids.len() > MAX_DYNAMIC_TEMPLATES {
            if let Some(oldest) = self.dynamic_ids.pop_front() {
                self.templates.remove(&oldest);
            }
        }
        id
    }

    /// Register a template at a specific class ID (instead of auto-assigning).
    pub fn register_str_at(&mut self, class: u32, name: &str, wgsl_source: String) {
        self.templates.insert(
            class,
            RadiantTemplate {
                name: Arc::from(format!("Radiant:{}", name)),
                wgsl_source: Arc::from(wgsl_source),
            },
        );
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }
}
