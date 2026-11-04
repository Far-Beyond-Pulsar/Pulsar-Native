use std::collections::{HashMap, VecDeque};
use std::sync::{OnceLock, RwLock};

const MAX_GRAPH_SNIPPETS: usize = 4096;

/// Registry of graph-generated WGSL snippets, keyed by hash.
///
/// External tools (graph compilers, editors) call `register()` to make a
/// compiled material graph available to the engine. The GBuffer pass looks
/// up the snippet by hash when it encounters a material with `graph_hash != 0`.
pub struct RadiantGraphRegistry {
    /// graph_hash → WGSL source snippet injected at RADIANT_OVERRIDE_SURFACE
    snippets: HashMap<u64, String>,
    insertion_order: VecDeque<u64>,
}

impl RadiantGraphRegistry {
    pub fn new() -> Self {
        Self {
            snippets: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    /// Register a compiled graph snippet. `graph_hash` should be a content-hash
    /// of the graph's serialized form (provided by the external compiler).
    pub fn register(&mut self, graph_hash: u64, wgsl_snippet: String) {
        if self.snippets.contains_key(&graph_hash) {
            self.snippets.insert(graph_hash, wgsl_snippet);
            return;
        }
        self.snippets.insert(graph_hash, wgsl_snippet);
        self.insertion_order.push_back(graph_hash);
        while self.snippets.len() > MAX_GRAPH_SNIPPETS {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.snippets.remove(&oldest);
            }
        }
    }

    /// Look up a graph snippet by hash. Returns None if not found.
    pub fn get(&self, graph_hash: u64) -> Option<&str> {
        self.snippets.get(&graph_hash).map(|s| s.as_str())
    }

    /// Remove a graph snippet (e.g. when a material is deleted).
    pub fn unregister(&mut self, graph_hash: u64) {
        self.snippets.remove(&graph_hash);
        self.insertion_order.retain(|hash| *hash != graph_hash);
    }

    /// Number of registered graph snippets.
    pub fn len(&self) -> usize {
        self.snippets.len()
    }
}

/// Process-wide graph source registry shared by SceneDB's material projection
/// and the render passes. The source is immutable for a given content hash.
static ACTIVE_GRAPHS: OnceLock<RwLock<RadiantGraphRegistry>> = OnceLock::new();

fn active_graphs() -> &'static RwLock<RadiantGraphRegistry> {
    ACTIVE_GRAPHS.get_or_init(|| RwLock::new(RadiantGraphRegistry::new()))
}

/// Register graph WGSL for render rows that reference `graph_hash`.
pub fn register_graph_source(graph_hash: u64, wgsl_snippet: String) {
    if graph_hash == 0 {
        return;
    }
    if let Ok(mut registry) = active_graphs().write() {
        registry.register(graph_hash, wgsl_snippet);
    }
}

/// Clone a graph snippet so a render pass can compile it without holding the
/// registry lock during pipeline creation.
pub fn graph_source(graph_hash: u64) -> Option<String> {
    if graph_hash == 0 {
        return None;
    }
    active_graphs()
        .read()
        .ok()?
        .get(graph_hash)
        .map(str::to_owned)
}
