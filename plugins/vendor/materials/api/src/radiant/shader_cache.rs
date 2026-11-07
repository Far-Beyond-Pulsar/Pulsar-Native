use std::collections::{HashMap, VecDeque};

const MAX_SHADER_MODULES: usize = 512;

/// Key for cached shader modules: (template_id, graph_hash, feature_flag_mask)
#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct RadiantShaderKey {
    pub template_id: u32,
    pub graph_hash: u64,
    pub feature_flags: u32,
}

/// Cache of compiled shader modules keyed by template variant.
///
/// The same (template, graph, flags) tuple always produces identical WGSL, so
/// we keep the `wgpu::ShaderModule` around to avoid recompilation.
pub struct RadiantShaderCache {
    modules: HashMap<RadiantShaderKey, wgpu::ShaderModule>,
    insertion_order: VecDeque<RadiantShaderKey>,
}

impl RadiantShaderCache {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    pub fn get(&self, key: &RadiantShaderKey) -> Option<&wgpu::ShaderModule> {
        self.modules.get(key)
    }

    pub fn insert(&mut self, key: RadiantShaderKey, module: wgpu::ShaderModule) {
        if self.modules.contains_key(&key) {
            self.modules.insert(key, module);
            return;
        }
        self.modules.insert(key, module);
        self.insertion_order.push_back(key);
        while self.modules.len() > MAX_SHADER_MODULES {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.modules.remove(&oldest);
            }
        }
    }

    /// Get or compile the shader module of `key`: `template` composed into
    /// the drawer's `program` with `graph_wgsl` (empty: the template as it
    /// is) and the program's own `snippets` (`RadiantTemplate::compose`),
    /// lowered to baseline-WebGPU bindings where the device has no binding
    /// arrays.
    #[allow(clippy::too_many_arguments)]
    pub fn get_or_compile(
        &mut self,
        device: &wgpu::Device,
        key: RadiantShaderKey,
        template: &super::template::RadiantTemplate,
        program: &str,
        graph_wgsl: &str,
        snippets: &[helio_core::shader::ShaderSnippet],
        material_binding: crate::material::MaterialBindingConfig,
        label: &str,
    ) -> &wgpu::ShaderModule {
        if !self.modules.contains_key(&key) {
            let source =
                template.compose(program, graph_wgsl, material_binding.max_textures, snippets);
            let source = if material_binding.uses_binding_arrays() {
                source
            } else {
                super::template::RadiantTemplate::apply_webgpu_fixups(
                    &source,
                    material_binding.max_textures,
                )
            };
            // Runtime-generated WGSL (template + material graph + webgpu
            // fixups): there is no source file to watch, so it cannot go
            // through `helio_core::shader::module`'s registered path.
            #[allow(clippy::disallowed_methods)]
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            self.insert(key, module);
        }
        self.modules.get(&key).unwrap()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }
}
