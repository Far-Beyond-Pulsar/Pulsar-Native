//! Tier-3 tuning knobs, the generated WGSL constant prelude, and shader assembly.

/// Compile-time (pipeline-creation-time) configuration of the dense kernels.
/// Every field becomes a WGSL `const`, so changing it means rebuilding the
/// pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DenseConfig {
    /// Tile edge in pixels (8 or 16). One workgroup of `tile * tile` threads
    /// per dense tile.
    pub tile: u32,
    /// Depth bucket resolution: `1 << bucket_bits` buckets over reversed-Z
    /// [0, 1]. The plan specifies 12.
    pub bucket_bits: u32,
    /// Peel-layer slab size: a layer consumes the nearest buckets until at
    /// least this many triangles are covered.
    pub slab_tris: u32,
    /// Nominal peel depth K (plan: K <= 4; 2 on low shared memory).
    pub nominal_layers: u32,
    /// Hard layer cap. Layers in `nominal_layers..max_layers` are the "extra
    /// peel iterations"; reaching the cap clamps the tile.
    pub max_layers: u32,
    /// Clusters one peel chunk holds. A tile whose list is longer is peeled in
    /// several chunks of at most this many clusters and the nearest fragment is
    /// kept across chunks (exact; the cap only bounds the u16 histogram and the
    /// cost of one chunk).
    pub cluster_cap: u32,
    /// Entries of the shared cluster-list pool all dense tiles draw from (each
    /// tile gets exactly as many as it needs). Entries past the pool are dropped
    /// and counted in `DenseStats::bin_overflow`: the only way geometry is lost.
    pub list_pool: u32,
    /// Triangles set up per shared-memory batch.
    pub tri_batch: u32,
    /// Upper bound on dense tiles processed per frame (further ones are
    /// dropped and counted in `DenseStats::slot_overflow`).
    pub max_dense_slots: u32,
}

impl Default for DenseConfig {
    fn default() -> Self {
        Self {
            tile: 16,
            bucket_bits: 12,
            slab_tris: 1024,
            nominal_layers: 4,
            max_layers: 16,
            cluster_cap: 511,
            list_pool: 1 << 23,
            tri_batch: 64,
            max_dense_slots: 8192,
        }
    }
}

impl DenseConfig {
    /// Plan §6 "Mobile / low shared memory (< 16 KiB)": 8x8 tiles, K = 2.
    pub fn low_shared_memory() -> Self {
        Self {
            tile: 8,
            bucket_bits: 10,
            slab_tris: 512,
            nominal_layers: 2,
            max_layers: 8,
            cluster_cap: 128,
            list_pool: 1 << 21,
            tri_batch: 32,
            max_dense_slots: 4096,
        }
    }

    /// Pick the default config when the device offers enough workgroup
    /// storage, else the low-memory one.
    pub fn for_workgroup_storage(max_workgroup_storage_bytes: u32) -> Self {
        let d = Self::default();
        if d.shared_memory_bytes() <= max_workgroup_storage_bytes {
            d
        } else {
            Self::low_shared_memory()
        }
    }

    /// Words of the `slot_lists` buffer: five header words per slot, then the pool.
    pub fn slot_list_words(&self) -> u64 {
        self.max_dense_slots as u64 * 5 + self.list_pool as u64
    }

    pub fn threads(&self) -> u32 {
        self.tile * self.tile
    }

    pub fn buckets(&self) -> u32 {
        1 << self.bucket_bits
    }

    /// Workgroup storage the peel kernel declares: histogram, staged cluster
    /// vertices, triangle setup batch + boxes, and ~16 scalars.
    pub fn shared_memory_bytes(&self) -> u32 {
        self.buckets() / 2 * 4 + 128 * 16 + self.tri_batch * 48 + self.tri_batch * 4 + 16 * 4
    }

    /// Workgroup storage of the alpha-masked peel: the opaque kernel plus staged
    /// uv and 1/w per vertex and the per-triangle uv planes.
    pub fn shared_memory_bytes_masked(&self) -> u32 {
        self.shared_memory_bytes() + 128 * 8 + 128 * 4 + self.tri_batch * 48 + 4
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.tile != 8 && self.tile != 16 {
            return Err(format!("tile must be 8 or 16, got {}", self.tile));
        }
        if !(2..=12).contains(&self.bucket_bits) {
            return Err("bucket_bits must be in 2..=12".into());
        }
        if self.tri_batch == 0 || self.tri_batch > self.threads() {
            return Err("tri_batch must be in 1..=tile*tile".into());
        }
        if self.nominal_layers == 0 || self.max_layers < self.nominal_layers {
            return Err("need 1 <= nominal_layers <= max_layers".into());
        }
        if self.slab_tris == 0
            || self.cluster_cap == 0
            || self.max_dense_slots == 0
            || self.list_pool == 0
        {
            return Err("slab_tris, cluster_cap and max_dense_slots must be nonzero".into());
        }
        // Histogram counts are u16 halves of u32 words.
        if self.cluster_cap * helio_vg_format::CLUSTER_MAX_TRIS >= 65536 {
            return Err("cluster_cap * 128 must stay below 65536 (u16 bucket counts)".into());
        }
        Ok(())
    }

    /// WGSL constants every Tier-3 kernel expects.
    pub fn prelude(&self) -> String {
        format!(
            "const TILE: u32 = {tile}u;\n\
             const WG_THREADS: u32 = {threads}u;\n\
             const BUCKET_BITS: u32 = {bits}u;\n\
             const NB: u32 = {nb}u;\n\
             const HIST_WORDS: u32 = {hw}u;\n\
             const SLAB_TRIS: u32 = {slab}u;\n\
             const NOMINAL_LAYERS: u32 = {nom}u;\n\
             const MAX_LAYERS: u32 = {max}u;\n\
             const CLUSTER_CAP: u32 = {cap}u;\n\
             const LIST_POOL_BASE: u32 = {pool_base}u;\n\
             const LIST_POOL_CAP: u32 = {pool_cap}u;\n\
             const TRI_BATCH: u32 = {batch}u;\n\
             const TRI_BATCH_X3: u32 = {batch3}u;\n\
             const CULL_RANGES: u32 = {cr}u;\n\
             const CULL_RANGE_COUNT: u32 = {rc}u;\n\
             const ARGS_PREFIX: u32 = {ap}u;\n\
             const CULL_FIRST_MASKED: u32 = {fm}u;\n",
            tile = self.tile,
            threads = self.threads(),
            bits = self.bucket_bits,
            nb = self.buckets(),
            hw = self.buckets() / 2,
            slab = self.slab_tris,
            nom = self.nominal_layers,
            max = self.max_layers,
            cap = self.cluster_cap,
            pool_base = self.max_dense_slots * 5,
            pool_cap = self.list_pool,
            batch = self.tri_batch,
            batch3 = self.tri_batch * 3,
            cr = helio_vg_format::CULL_STATE_RANGES,
            rc = 2 * helio_vg_format::CULL_BIN_COUNT,
            ap = 8 + 2 * helio_vg_format::CULL_BIN_COUNT,
            fm = helio_vg_format::CULL_FIRST_MASKED_BIN,
        )
    }
}

const COMMON: &str = include_str!("../shaders/common.wgsl");
const INPUTS: &str = include_str!("../shaders/inputs.wgsl");

/// Group 0 bindings 3 (page pool) and 4 (asset quant) read by the shared decode.
fn decode_bindings() -> String {
    helio_vg_format::decode_bindings_wgsl(0, 3, 4)
}
const TILES: &str = include_str!("../shaders/tiles.wgsl");
const ARGS: &str = include_str!("../shaders/args.wgsl");
const BOUNDS: &str = include_str!("../shaders/bounds.wgsl");
const FOOTPRINT: &str = include_str!("../shaders/footprint.wgsl");
const BIN: &str = include_str!("../shaders/bin.wgsl");
const COLLECT: &str = include_str!("../shaders/collect.wgsl");
const PEEL: &str = include_str!("../shaders/peel.wgsl");
const RT: &str = include_str!("../shaders/rt.wgsl");

fn assemble(parts: &[&str]) -> String {
    parts.join("\n")
}

/// Dense-tile compaction (`compact`, `compact_tiles_only`).
pub fn tiles_source(cfg: &DenseConfig) -> String {
    assemble(&[&cfg.prelude(), helio_vg_format::WGSL, COMMON, TILES])
}

/// Indirect-args kernels (`make_args`, `make_args_rt`).
pub fn args_source(cfg: &DenseConfig) -> String {
    assemble(&[&cfg.prelude(), helio_vg_format::WGSL, COMMON, ARGS])
}

/// Cluster-to-tile binning (`bin`).
pub fn bin_source(cfg: &DenseConfig) -> String {
    let bin = format!(
        "const BIN_DENSE_BIN: u32 = {}u;
",
        helio_pass_vg_cull::BIN_DENSE
    );
    assemble(&[
        &cfg.prelude(),
        &bin,
        helio_vg_format::WGSL,
        COMMON,
        &decode_bindings(),
        helio_vg_format::DECODE_WGSL,
        INPUTS,
        BOUNDS,
        BIN,
    ])
}

/// Tile footprint of the dense-bin clusters (`footprint_args`, `footprint`).
pub fn footprint_source(cfg: &DenseConfig) -> String {
    let bin = format!(
        "const FP_DENSE_BIN: u32 = {}u;
",
        helio_pass_vg_cull::BIN_DENSE
    );
    assemble(&[
        &cfg.prelude(),
        &bin,
        helio_vg_format::WGSL,
        COMMON,
        &decode_bindings(),
        helio_vg_format::DECODE_WGSL,
        INPUTS,
        BOUNDS,
        FOOTPRINT,
    ])
}

/// Dense-instance candidate collection for the ray-traced backend (`collect`).
pub fn collect_source(cfg: &DenseConfig) -> String {
    let bin = format!(
        "const BIN_DENSE_BIN: u32 = {}u;
const BIN_SW_BIN: u32 = {}u;
",
        helio_pass_vg_cull::BIN_DENSE,
        helio_pass_vg_cull::BIN_SW
    );
    assemble(&[
        &cfg.prelude(),
        &bin,
        helio_vg_format::WGSL,
        COMMON,
        &decode_bindings(),
        helio_vg_format::DECODE_WGSL,
        INPUTS,
        BOUNDS,
        COLLECT,
    ])
}

/// Depth peel (`peel`): the opaque variant of `peel.wgsl`.
pub fn peel_source(cfg: &DenseConfig) -> String {
    assemble(&[
        &cfg.prelude(),
        helio_vg_format::WGSL,
        COMMON,
        &decode_bindings(),
        helio_vg_format::DECODE_WGSL,
        INPUTS,
        &peel_variant(false),
    ])
}

/// Keep the lines of one variant of a marked shader: `//@M ` lines only in the
/// alpha-masked variant, `//@O ` lines only in the opaque one.
fn peel_variant(masked: bool) -> String {
    let mut out = String::new();
    for line in PEEL.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("//@M ") {
            if masked {
                out.push_str(rest);
                out.push('\n');
            }
        } else if let Some(rest) = t.strip_prefix("//@O ") {
            if !masked {
                out.push_str(rest);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Depth peel of alpha-tested clusters (`peel`, group 1 = the alpha bindings).
pub fn peel_masked_source(cfg: &DenseConfig, binding: helio_mats::MaterialBindingConfig) -> String {
    let alpha = helio_mats::vg_alpha::bindings_wgsl(binding, 1, 0);
    format!(
        "{}{}",
        helio_mats::vg_alpha::module_header(binding),
        assemble(&[
            &cfg.prelude(),
            helio_vg_format::WGSL,
            COMMON,
            &decode_bindings(),
            helio_vg_format::DECODE_WGSL,
            INPUTS,
            &alpha,
            helio_mats::vg_alpha::WGSL,
            helio_mats::vg_alpha::UV_WGSL,
            &peel_variant(true),
        ])
    )
}

/// Ray-query primary visibility (`trace`). Requires `EXPERIMENTAL_RAY_QUERY`.
pub fn rt_source(cfg: &DenseConfig) -> String {
    // `enable` directives must precede every declaration.
    let mut s = String::from("enable wgpu_ray_query;\n");
    s.push_str(&assemble(&[
        &cfg.prelude(),
        helio_vg_format::WGSL,
        COMMON,
        RT,
    ]));
    s
}

/// Every WGSL source this crate compiles, named (for validation tests).
pub fn validation_sources(cfg: &DenseConfig) -> Vec<(&'static str, String)> {
    vec![
        ("tiles", tiles_source(cfg)),
        ("args", args_source(cfg)),
        ("bin", bin_source(cfg)),
        ("collect", collect_source(cfg)),
        ("footprint", footprint_source(cfg)),
        ("peel", peel_source(cfg)),
        (
            "peel_masked (bindless)",
            peel_masked_source(
                cfg,
                helio_mats::MaterialBindingConfig {
                    mode: helio_mats::MaterialBindingMode::BindingArray,
                    max_textures: 256,
                },
            ),
        ),
        (
            "peel_masked (expanded)",
            peel_masked_source(
                cfg,
                helio_mats::MaterialBindingConfig {
                    mode: helio_mats::MaterialBindingMode::Expanded,
                    max_textures: 11,
                },
            ),
        ),
        ("rt", rt_source(cfg)),
    ]
}
