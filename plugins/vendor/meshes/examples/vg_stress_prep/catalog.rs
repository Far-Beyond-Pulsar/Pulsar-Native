//! What the museum downloads from Poly Haven (all CC0) and how the user picks
//! a subset. Pure data plus env/CLI parsing; shared by `build.rs`, the
//! provisioning code, the scene layout and the loader.

/// One tree package: a glTF with several tree variants (separate glTF meshes)
/// and a shared texture set.
#[derive(Debug)]
pub struct TreeDef {
    /// Poly Haven asset id (also the folder under `assets/vg_stress/trees/`).
    pub id: &'static str,
    pub label: &'static str,
    /// Skipped by the `small` preset (the multi-hundred-megabyte packages).
    pub large: bool,
    /// Preferred texture resolutions, best first.
    pub resolutions: &'static [&'static str],
    /// Variants (glTF source meshes) measured from the 2k glTF accessors.
    pub variants: &'static [TreeVariant],
    /// Every glTF material slot of the package. `masked` mirrors the glTF's
    /// `alphaMode: BLEND` (twig / leaf cards); the loader reads the real flag
    /// from the glTF and the two must agree (see the tests).
    pub slots: &'static [TreeSlot],
    /// Uniform scale range used when scattering this tree as forest.
    pub forest_scale: (f32, f32),
}

#[derive(Debug)]
pub struct TreeSlot {
    pub name: &'static str,
    pub masked: bool,
    /// Flat colour used until (or without) the textures.
    pub fallback: [f32; 3],
}

const fn slot(name: &'static str, masked: bool, fallback: [f32; 3]) -> TreeSlot {
    TreeSlot {
        name,
        masked,
        fallback,
    }
}
const BARK: [f32; 3] = [0.17, 0.11, 0.07];
const LEAF: [f32; 3] = [0.07, 0.17, 0.06];

#[derive(Debug)]
pub struct TreeVariant {
    /// Index of the glTF mesh (`ConvertedMesh::source_mesh`).
    pub mesh: usize,
    pub label: &'static str,
    /// Source triangles of the variant, all materials.
    pub triangles: u64,
    /// Triangles in alpha-masked slots (VG skips these until masked support).
    pub masked_triangles: u64,
    /// Height in metres at scale 1.
    pub height: f32,
}

impl TreeDef {
    pub fn triangles(&self) -> u64 {
        self.variants.iter().map(|v| v.triangles).sum()
    }

    /// Path of the glTF relative to `assets/vg_stress/` for the first
    /// (preferred) resolution. The provisioned resolution is recorded in the
    /// unit manifest; see `polyhaven::tree_gltf`.
    pub fn gltf_rel(&self) -> String {
        format!("trees/{0}/{0}_{1}.gltf", self.id, self.resolutions[0])
    }
}

pub const TREES: &[TreeDef] = &[
    TreeDef {
        id: "pine_tree_01",
        label: "Pine Tree 01",
        large: true,
        resolutions: &["2k", "1k"],
        variants: &[
            TreeVariant {
                mesh: 0,
                label: "a",
                triangles: 6_951_524,
                masked_triangles: 0,
                height: 20.4,
            },
            TreeVariant {
                mesh: 1,
                label: "b",
                triangles: 4_234_432,
                masked_triangles: 0,
                height: 14.9,
            },
            TreeVariant {
                mesh: 2,
                label: "c",
                triangles: 5_996_296,
                masked_triangles: 0,
                height: 17.5,
            },
        ],
        slots: &[
            slot("pine_tree_01_bark", false, BARK),
            slot("pine_tree_01_trunk_a", false, BARK),
            slot("pine_tree_01_twig", false, LEAF),
            slot("pine_tree_01_dead_branches", false, BARK),
            slot("pine_tree_01_trunk_b", false, BARK),
            slot("pine_tree_01_trunk_c", false, BARK),
        ],
        forest_scale: (0.8, 1.25),
    },
    TreeDef {
        id: "fir_tree_01",
        label: "Fir Tree 01",
        large: true,
        resolutions: &["2k", "1k"],
        variants: &[
            TreeVariant {
                mesh: 0,
                label: "a",
                triangles: 4_176_819,
                masked_triangles: 4_066_119,
                height: 18.9,
            },
            TreeVariant {
                mesh: 1,
                label: "b",
                triangles: 2_300_624,
                masked_triangles: 2_207_296,
                height: 14.1,
            },
            TreeVariant {
                mesh: 2,
                label: "c",
                triangles: 505_494,
                masked_triangles: 446_074,
                height: 14.5,
            },
        ],
        slots: &[
            slot("fir_tree_01_bark", false, BARK),
            slot("fir_tree_01_trunk_a", false, BARK),
            slot("fir_tree_01_twig", true, LEAF),
            slot("fir_tree_01_dead_branches", false, BARK),
            slot("fir_tree_01_trunk_b", false, BARK),
            slot("fir_tree_01_trunk_c", false, BARK),
        ],
        forest_scale: (0.8, 1.25),
    },
    TreeDef {
        id: "island_tree_01",
        label: "Island Tree 01",
        large: false,
        resolutions: &["2k", "1k"],
        variants: &[TreeVariant {
            mesh: 0,
            label: "a",
            triangles: 1_599_403,
            masked_triangles: 1_060_032,
            height: 5.0,
        }],
        slots: &[
            slot("island_tree_01", false, BARK),
            slot("island_tree_01_leaves", true, LEAF),
            slot("island_tree_01_branches", false, BARK),
        ],
        forest_scale: (1.0, 1.7),
    },
    TreeDef {
        id: "island_tree_02",
        label: "Island Tree 02",
        large: false,
        resolutions: &["2k", "1k"],
        variants: &[TreeVariant {
            mesh: 0,
            label: "a",
            triangles: 1_072_213,
            masked_triangles: 714_744,
            height: 3.4,
        }],
        slots: &[
            slot("island_tree_02", false, BARK),
            slot("island_tree_02_leaves", true, LEAF),
            slot("island_tree_02_branches", false, BARK),
        ],
        forest_scale: (1.1, 1.9),
    },
    TreeDef {
        id: "island_tree_03",
        label: "Island Tree 03",
        large: false,
        resolutions: &["2k", "1k"],
        variants: &[TreeVariant {
            mesh: 0,
            label: "a",
            triangles: 2_085_320,
            masked_triangles: 1_408_704,
            height: 2.6,
        }],
        slots: &[
            slot("island_tree_03", false, BARK),
            slot("island_tree_03_leaves", true, LEAF),
            slot("island_tree_03_branches", false, BARK),
        ],
        forest_scale: (1.3, 2.2),
    },
    TreeDef {
        id: "tree_small_02",
        label: "Small Tree 02",
        large: false,
        resolutions: &["2k", "1k"],
        variants: &[TreeVariant {
            mesh: 0,
            label: "a",
            triangles: 2_062_487,
            masked_triangles: 1_939_380,
            height: 4.6,
        }],
        slots: &[
            slot("tree_small_02_branches", false, BARK),
            slot("tree_small_02_leaves", true, LEAF),
            slot("tree_small_02_trunk", false, BARK),
        ],
        forest_scale: (1.0, 1.8),
    },
];

/// A marble PBR set. Which museum surfaces use it is decided in
/// `vg_stress_data::materials`.
#[derive(Debug)]
pub struct MarbleDef {
    pub id: &'static str,
    pub label: &'static str,
    pub use_for: &'static str,
    /// Preferred resolutions, best first (8k, then 4k when 8k is not offered).
    pub resolutions: &'static [&'static str],
}

pub const MARBLES: &[MarbleDef] = &[
    MarbleDef {
        id: "grey_cartago_01",
        label: "Grey Cartago 01",
        use_for: "main floor, pedestals, impluvium",
        resolutions: &["8k", "4k", "2k"],
    },
    MarbleDef {
        id: "grey_cartago_03",
        label: "Grey Cartago 03",
        use_for: "Ionic columns, secondary slabs, benches, steles, step accents",
        resolutions: &["8k", "4k", "2k"],
    },
    MarbleDef {
        id: "marble_01",
        label: "Marble 01",
        use_for: "Doric columns, entablature, walls, pediments, steps",
        resolutions: &["8k", "4k", "2k"],
    },
    MarbleDef {
        id: "marble_mosaic_tiles",
        label: "Marble Mosaic Tiles",
        use_for: "floor inlay border, plaza",
        resolutions: &["8k", "4k", "2k"],
    },
];

pub fn tree(id: &str) -> Option<&'static TreeDef> {
    TREES.iter().find(|t| t.id == id)
}

/// How hard the GPU texture budget is pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureQuality {
    /// Marble diffuse 4k, normal/ARM 2k; trees 1k.
    Low,
    /// Marble diffuse 8k, normal/ARM 4k; trees 2k diffuse, 1k normal/ARM.
    Balanced,
    /// Everything at the resolution that was downloaded.
    Max,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Trees to provision and scatter (default: all six).
    pub trees: Vec<&'static TreeDef>,
    /// Scene density multiplier (1 = moderate).
    pub density: f32,
    pub quality: TextureQuality,
    /// Also download the displacement maps (not used by the renderer yet).
    pub fetch_displacement: bool,
}

pub const DEFAULT_DENSITY: f32 = 1.0;
/// Highest supported museum scene-density multiplier. The layout creates
/// individual tree instances, so keep the input bounded while allowing scenes
/// well beyond the original 32x stress limit.
pub const MAX_DENSITY: f32 = 128.0;

impl Default for Config {
    fn default() -> Self {
        Config::resolve(None, None, None, None)
    }
}

/// Parse a density: a number or `low` / `normal` / `moderate` / `high` / `insane`.
pub fn parse_density(text: &str) -> Option<f32> {
    let t = text.trim().to_ascii_lowercase();
    let named = match t.as_str() {
        "low" => Some(0.4),
        "normal" | "moderate" | "default" => Some(1.0),
        "high" => Some(3.0),
        "insane" => Some(8.0),
        _ => None,
    };
    named
        .or_else(|| t.trim_end_matches('x').parse::<f32>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| d.clamp(0.05, MAX_DENSITY))
}

pub fn parse_trees(text: &str) -> Result<Vec<&'static TreeDef>, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "" | "full" | "all" => Ok(TREES.iter().collect()),
        "small" | "lite" => Ok(TREES.iter().filter(|t| !t.large).collect()),
        "none" | "off" => Ok(Vec::new()),
        list => list
            .split(',')
            .map(|id| tree(id.trim()).ok_or_else(|| format!("unknown tree `{}`", id.trim())))
            .collect(),
    }
}

impl Config {
    /// Invalid values fall back to the default with no panic: the build script
    /// must never fail on a typo.
    pub fn resolve(
        trees: Option<&str>,
        density: Option<&str>,
        quality: Option<&str>,
        displacement: Option<&str>,
    ) -> Self {
        let trees = trees
            .and_then(|t| parse_trees(t).ok())
            .unwrap_or_else(|| TREES.iter().collect());
        let density = density.and_then(parse_density).unwrap_or(DEFAULT_DENSITY);
        let quality = match quality.map(|q| q.trim().to_ascii_lowercase()).as_deref() {
            Some("low") => TextureQuality::Low,
            Some("max") | Some("high") => TextureQuality::Max,
            _ => TextureQuality::Balanced,
        };
        let fetch_displacement =
            displacement.is_some_and(|d| !matches!(d.trim(), "" | "0" | "false"));
        Config {
            trees,
            density,
            quality,
            fetch_displacement,
        }
    }

    /// CLI (`--trees <v>`, `--density <v>`, `--textures <v>`, `--small-trees`)
    /// over environment (`VG_STRESS_TREES`, `VG_STRESS_DENSITY`,
    /// `VG_STRESS_TEXTURES`, `VG_STRESS_DISPLACEMENT`).
    pub fn from_env_and_args() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::from_sources(&args, &|k| std::env::var(k).ok())
    }

    pub fn from_sources(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Self {
        let flag = |name: &str| -> Option<String> {
            let prefix = format!("{name}=");
            let mut it = args.iter();
            while let Some(a) = it.next() {
                if a == name {
                    return it.next().cloned();
                }
                if let Some(v) = a.strip_prefix(&prefix) {
                    return Some(v.to_string());
                }
            }
            None
        };
        let small = args.iter().any(|a| a == "--small-trees");
        let trees = flag("--trees")
            .or(small.then(|| "small".to_string()))
            .or_else(|| env("VG_STRESS_TREES"));
        Config::resolve(
            trees.as_deref(),
            flag("--density")
                .or_else(|| env("VG_STRESS_DENSITY"))
                .as_deref(),
            flag("--textures")
                .or_else(|| env("VG_STRESS_TEXTURES"))
                .as_deref(),
            env("VG_STRESS_DISPLACEMENT").as_deref(),
        )
    }

    pub fn has_tree(&self, id: &str) -> bool {
        self.trees.iter().any(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(args: &[&str], env: &[(&str, &str)]) -> Config {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_sources(&args, &|k| {
            env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn default_is_full_moderate_balanced() {
        let c = cfg(&[], &[]);
        assert_eq!(c.trees.len(), 6);
        assert_eq!(c.density, 1.0);
        assert_eq!(c.quality, TextureQuality::Balanced);
        assert!(!c.fetch_displacement);
    }

    #[test]
    fn small_preset_drops_the_large_packages() {
        for c in [
            cfg(&["--trees", "small"], &[]),
            cfg(&["--small-trees"], &[]),
            cfg(&[], &[("VG_STRESS_TREES", "small")]),
        ] {
            let ids: Vec<_> = c.trees.iter().map(|t| t.id).collect();
            assert_eq!(
                ids,
                [
                    "island_tree_01",
                    "island_tree_02",
                    "island_tree_03",
                    "tree_small_02"
                ]
            );
        }
        // CLI beats environment.
        assert_eq!(
            cfg(&["--trees=none"], &[("VG_STRESS_TREES", "small")])
                .trees
                .len(),
            0
        );
        assert_eq!(
            cfg(&[], &[("VG_STRESS_TREES", "pine_tree_01, island_tree_02")])
                .trees
                .len(),
            2
        );
        // A typo never breaks a build: falls back to the default.
        assert_eq!(cfg(&[], &[("VG_STRESS_TREES", "oak")]).trees.len(), 6);
    }

    #[test]
    fn density_names_and_numbers() {
        assert_eq!(parse_density("high"), Some(3.0));
        assert_eq!(parse_density("insane"), Some(8.0));
        assert_eq!(parse_density("2.5"), Some(2.5));
        assert_eq!(parse_density("2x"), Some(2.0));
        assert_eq!(parse_density("0"), None);
        assert_eq!(parse_density("nan"), None);
        assert_eq!(parse_density("1000"), Some(MAX_DENSITY));
        assert_eq!(
            cfg(&["--density", "high"], &[("VG_STRESS_DENSITY", "low")]).density,
            3.0
        );
    }

    #[test]
    fn variant_triangles_are_consistent() {
        for t in TREES {
            for v in t.variants {
                assert!(v.masked_triangles <= v.triangles, "{}", t.id);
            }
            assert_eq!(
                t.slots.iter().all(|s| !s.masked),
                t.variants.iter().all(|v| v.masked_triangles == 0),
                "{}",
                t.id
            );
        }
        assert_eq!(tree("pine_tree_01").unwrap().triangles(), 17_182_252);
        assert_eq!(tree("fir_tree_01").unwrap().triangles(), 6_982_937);
    }
}
