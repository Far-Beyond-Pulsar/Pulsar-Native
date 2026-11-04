//! Poly Haven (CC0) client for the museum's trees and marble: file lists from
//! `https://api.polyhaven.com`, multi-file glTF packages with their directory
//! structure preserved, PBR texture sets, per-asset manifests and the credits
//! file written at provisioning time.
//!
//! Network use follows the Poly Haven API terms: a descriptive `User-Agent`
//! (see [`download::USER_AGENT`]) and no scraping beyond the three documented
//! endpoints used here (`/files/<id>`, `/info/<id>`, and the download URLs the
//! files endpoint hands out). Everything is plain Rust; no scripts.

use super::catalog::{self, MarbleDef, TreeDef};
use super::download::{self, Checksum};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};

pub const API_BASE: &str = "https://api.polyhaven.com";
pub const LICENCE: &str = "CC0 1.0 Universal (public domain dedication)";
pub const LICENCE_URL: &str = "https://polyhaven.com/license";

/// Bump when the on-disk layout or the selection of files changes, so existing
/// users re-provision (also recorded as `ph:<id>` entries in `prepared.json`).
pub const PH_RECIPE_VERSION: u32 = 2;

// ── API client ──────────────────────────────────────────────────────────────

pub struct Api {
    base: String,
    backoff: Duration,
}

impl Api {
    pub fn new(base: impl Into<String>, backoff: Duration) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            backoff,
        }
    }

    pub fn production() -> Self {
        Self::new(API_BASE, Duration::from_secs(2))
    }

    /// GET `<base>/<path>` and parse the JSON body. Server errors and dropped
    /// connections are retried with back-off; other statuses fail at once.
    pub fn get_json(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let agent = ureq::AgentBuilder::new()
            .user_agent(download::USER_AGENT)
            .timeout_connect(Duration::from_secs(20))
            .timeout_read(Duration::from_secs(60))
            .build();
        let mut last = String::new();
        for attempt in 1..=4u32 {
            match agent.get(&url).call() {
                Ok(resp) => {
                    let text = resp.into_string().map_err(|e| format!("{url}: {e}"))?;
                    return serde_json::from_str(&text)
                        .map_err(|e| format!("{url}: bad JSON: {e}"));
                }
                Err(ureq::Error::Status(code, _)) if code >= 500 || code == 429 => {
                    last = format!("HTTP {code}");
                }
                Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code} for {url}")),
                Err(e) => last = e.to_string(),
            }
            std::thread::sleep(self.backoff * attempt);
        }
        Err(format!("{url}: {last}"))
    }
}

// ── plans ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFile {
    /// Path relative to the unit directory, `/` separated (glTF packages keep
    /// the `textures/` folder their `.gltf` refers to).
    pub rel: String,
    pub url: String,
    pub md5: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedFile {
    pub remote: RemoteFile,
    /// `gltf`, `bin`, `texture` for trees; `diffuse`, `normal`, `arm`,
    /// `displacement` for marble.
    pub role: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub resolution: String,
    pub files: Vec<PlannedFile>,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.remote.size).sum()
    }
}

/// Reject anything that could escape the unit directory.
pub fn safe_rel(rel: &str) -> Result<(), String> {
    let bad = rel.is_empty()
        || rel.starts_with('/')
        || rel.contains('\\')
        || rel.contains(':')
        || rel
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        Err(format!("unsafe file path `{rel}` in the file list"))
    } else {
        Ok(())
    }
}

fn file_name(url: &str) -> &str {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url)
}

fn remote(rel: &str, node: &Value) -> Result<RemoteFile, String> {
    safe_rel(rel)?;
    let field = |k: &str| {
        node.get(k)
            .ok_or_else(|| format!("{rel}: file entry has no `{k}`"))
    };
    let url = field("url")?
        .as_str()
        .ok_or_else(|| format!("{rel}: bad url"))?;
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(format!("{rel}: unsupported url scheme"));
    }
    let md5 = field("md5")?
        .as_str()
        .ok_or_else(|| format!("{rel}: bad md5"))?
        .to_ascii_lowercase();
    if md5.len() != 32 || !md5.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{rel}: malformed md5"));
    }
    let size = field("size")?
        .as_u64()
        .ok_or_else(|| format!("{rel}: bad size"))?;
    Ok(RemoteFile {
        rel: rel.to_string(),
        url: url.to_string(),
        md5,
        size,
    })
}

/// The glTF package at the best available of `resolutions`: the `.gltf` plus
/// everything in its `include` map (the `.bin` and textures), each at the
/// relative path the glTF refers to.
pub fn plan_gltf_package(files: &Value, resolutions: &[&str]) -> Result<Plan, String> {
    let gltf = files.get("gltf").ok_or("file list has no `gltf` section")?;
    for res in resolutions {
        let Some(entry) = gltf.get(*res).and_then(|r| r.get("gltf")) else {
            continue;
        };
        let main = remote(
            file_name(entry.get("url").and_then(Value::as_str).unwrap_or("")),
            entry,
        )?;
        let mut out = vec![PlannedFile {
            remote: main,
            role: "gltf".into(),
        }];
        if let Some(include) = entry.get("include").and_then(Value::as_object) {
            let mut keys: Vec<_> = include.keys().collect();
            keys.sort();
            for rel in keys {
                let role = if rel.ends_with(".bin") {
                    "bin"
                } else {
                    "texture"
                };
                out.push(PlannedFile {
                    remote: remote(rel, &include[rel])?,
                    role: role.into(),
                });
            }
        }
        // The JPEG glTF package omits the separate opacity maps shipped with
        // the Blender package. JPEG cannot carry the leaf cutout alpha.
        let mut opacity = std::collections::BTreeMap::new();
        if let Some(include) = files["blend"][*res]["blend"]["include"].as_object() {
            for (rel, node) in include {
                if rel.contains("_alpha_") && rel.ends_with(".png") {
                    opacity.insert(rel.clone(), node.clone());
                }
            }
        }
        if let Some(maps) = files.as_object() {
            for (name, map) in maps {
                if name == "alpha" || name.ends_with("_alpha") {
                    if let Some(node) = map[*res].get("png") {
                        let url = node["url"].as_str().ok_or("opacity map has no URL")?;
                        opacity.insert(format!("textures/{}", file_name(url)), node.clone());
                    }
                }
            }
        }
        for (rel, node) in opacity {
            if !out.iter().any(|f| f.remote.rel == rel) {
                out.push(PlannedFile {
                    remote: remote(&rel, &node)?,
                    role: "opacity".into(),
                });
            }
        }
        return Ok(Plan {
            resolution: (*res).to_string(),
            files: out,
        });
    }
    Err(format!("no glTF at any of {resolutions:?}"))
}

/// `(role, Poly Haven map key, required)`.
const MARBLE_MAPS: &[(&str, &str, bool)] = &[
    ("diffuse", "Diffuse", true),
    ("normal", "nor_gl", true),
    ("arm", "arm", true),
    ("displacement", "Displacement", false),
];

/// A texture set: the first resolution in `resolutions` that offers every
/// required map as a JPEG (`.jpg` is Poly Haven's high-quality lossy format;
/// the PNG of an 8k map is 100 to 350 MB). `displacement` adds the optional
/// height map where available.
pub fn plan_texture_set(
    files: &Value,
    resolutions: &[&str],
    displacement: bool,
) -> Result<Plan, String> {
    'res: for res in resolutions {
        let mut out = Vec::new();
        for (role, key, required) in MARBLE_MAPS {
            let entry = files
                .get(*key)
                .and_then(|m| m.get(*res))
                .and_then(|r| r.get("jpg"));
            match entry {
                Some(node) => {
                    if *role == "displacement" && !displacement {
                        continue;
                    }
                    let url = node.get("url").and_then(Value::as_str).unwrap_or("");
                    out.push(PlannedFile {
                        remote: remote(file_name(url), node)?,
                        role: (*role).into(),
                    });
                }
                None if *required => continue 'res,
                None => {}
            }
        }
        return Ok(Plan {
            resolution: (*res).to_string(),
            files: out,
        });
    }
    Err(format!(
        "no jpg texture set with diffuse, normal and ARM maps at any of {resolutions:?}"
    ))
}

/// `[(name, role)]` from an `/info/<id>` response, sorted by name.
pub fn authors(info: &Value) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = info
        .get("authors")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

// ── pins ────────────────────────────────────────────────────────────────────

struct Pins {
    assets: HashMap<String, (String, HashMap<String, (u64, String)>)>,
}

fn pins() -> &'static Pins {
    static PINS: OnceLock<Pins> = OnceLock::new();
    PINS.get_or_init(|| {
        let v: Value =
            serde_json::from_str(include_str!("polyhaven_pins.json")).expect("pins JSON");
        let mut assets = HashMap::new();
        for (id, a) in v["assets"].as_object().expect("pins assets") {
            let res = a["res"].as_str().unwrap_or("").to_string();
            let files = a["files"]
                .as_object()
                .expect("pin files")
                .iter()
                .map(|(rel, f)| {
                    (
                        rel.clone(),
                        (
                            f["size"].as_u64().unwrap_or(0),
                            f["md5"].as_str().unwrap_or("").to_string(),
                        ),
                    )
                })
                .collect();
            assets.insert(id.clone(), (res, files));
        }
        Pins { assets }
    })
}

/// Download size in bytes of what a normal run fetches for `id` (the pinned
/// resolution, no displacement map). 0 for an unknown asset.
pub fn pinned_bytes(id: &str) -> u64 {
    pins()
        .assets
        .get(id)
        .map(|(_, files)| {
            files
                .iter()
                .filter(|(rel, _)| !rel.contains("_disp_"))
                .map(|(_, (s, _))| *s)
                .sum()
        })
        .unwrap_or(0)
}

/// Differences between the pinned (captured 2026-10-22) and the API-reported
/// files. The API stays authoritative; a mismatch only warns that upstream
/// replaced something.
pub fn pin_warnings(id: &str, plan: &Plan) -> Vec<String> {
    let Some((res, pinned)) = pins().assets.get(id) else {
        return Vec::new();
    };
    if *res != plan.resolution {
        return Vec::new();
    }
    plan.files
        .iter()
        .filter_map(|f| match pinned.get(&f.remote.rel) {
            Some((size, md5)) if *size != f.remote.size || *md5 != f.remote.md5 => Some(format!(
                "{id}/{}: upstream file changed since the pins were captured",
                f.remote.rel
            )),
            _ => None,
        })
        .collect()
}

// ── units and manifests ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Tree,
    Marble,
}

#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub kind: UnitKind,
    pub id: &'static str,
    pub label: &'static str,
    resolutions: &'static [&'static str],
}

impl From<&'static TreeDef> for Unit {
    fn from(t: &'static TreeDef) -> Self {
        Unit {
            kind: UnitKind::Tree,
            id: t.id,
            label: t.label,
            resolutions: t.resolutions,
        }
    }
}

impl From<&'static MarbleDef> for Unit {
    fn from(m: &'static MarbleDef) -> Self {
        Unit {
            kind: UnitKind::Marble,
            id: m.id,
            label: m.label,
            resolutions: m.resolutions,
        }
    }
}

impl Unit {
    pub fn dir(&self, assets: &Path) -> PathBuf {
        assets
            .join(match self.kind {
                UnitKind::Tree => "trees",
                UnitKind::Marble => "marble",
            })
            .join(self.id)
    }

    /// Key in `prepared.json`.
    pub fn key(&self) -> String {
        format!("ph:{}", self.id)
    }

    pub fn recipe(&self) -> String {
        format!(
            "ph{PH_RECIPE_VERSION}:{}:{}",
            self.id,
            self.resolutions.join("/")
        )
    }
}

/// Every unit a configuration needs: all marble sets and the selected trees.
pub fn units(config: &catalog::Config) -> Vec<Unit> {
    catalog::MARBLES
        .iter()
        .map(Unit::from)
        .chain(config.trees.iter().map(|t| Unit::from(*t)))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub recipe: String,
    pub id: String,
    pub name: String,
    pub resolution: String,
    pub authors: Vec<(String, String)>,
    /// `(rel, role, size, md5)`.
    pub files: Vec<(String, String, u64, String)>,
}

const MANIFEST_NAME: &str = "polyhaven.json";

impl Manifest {
    fn to_json(&self) -> Value {
        json!({
            "recipe": self.recipe,
            "id": self.id,
            "name": self.name,
            "resolution": self.resolution,
            "licence": LICENCE,
            "source": format!("https://polyhaven.com/a/{}", self.id),
            "authors": self.authors.iter().map(|(n, r)| json!({"name": n, "role": r})).collect::<Vec<_>>(),
            "files": self.files.iter().map(|(rel, role, size, md5)| json!({"rel": rel, "role": role, "size": size, "md5": md5})).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Manifest {
            recipe: v["recipe"].as_str()?.to_string(),
            id: v["id"].as_str()?.to_string(),
            name: v["name"].as_str()?.to_string(),
            resolution: v["resolution"].as_str()?.to_string(),
            authors: v["authors"]
                .as_array()?
                .iter()
                .map(|a| {
                    Some((
                        a["name"].as_str()?.to_string(),
                        a["role"].as_str()?.to_string(),
                    ))
                })
                .collect::<Option<_>>()?,
            files: v["files"]
                .as_array()?
                .iter()
                .map(|f| {
                    Some((
                        f["rel"].as_str()?.to_string(),
                        f["role"].as_str()?.to_string(),
                        f["size"].as_u64()?,
                        f["md5"].as_str()?.to_string(),
                    ))
                })
                .collect::<Option<_>>()?,
        })
    }

    pub fn file(&self, role: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.1 == role)
            .map(|f| f.0.as_str())
    }
}

pub fn read_manifest(dir: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(dir.join(MANIFEST_NAME)).ok()?;
    Manifest::from_json(&serde_json::from_str(&text).ok()?)
}

/// True when `unit` was provisioned by the current recipe, every listed file
/// is on disk with its recorded size, and (when asked) a displacement map is
/// present. No network and no hashing: the hash was checked when downloaded.
pub fn unit_complete(assets: &Path, unit: &Unit, want_displacement: bool) -> bool {
    let dir = unit.dir(assets);
    let Some(m) = read_manifest(&dir) else {
        return false;
    };
    m.recipe == unit.recipe()
        && m.files.iter().all(|(rel, _, size, _)| {
            std::fs::metadata(dir.join(rel)).is_ok_and(|x| x.len() == *size)
        })
        && (!want_displacement || unit.kind != UnitKind::Marble || m.file("displacement").is_some())
}

/// Absolute path of a marble map (`diffuse`, `normal`, `arm`, `displacement`).
pub fn marble_map(assets: &Path, id: &str, role: &str) -> Option<PathBuf> {
    let m = marble_unit(id)?;
    let dir = m.dir(assets);
    let manifest = read_manifest(&dir)?;
    let path = dir.join(manifest.file(role)?);
    path.is_file().then_some(path)
}

/// Absolute path of a tree's `.gltf`.
pub fn tree_gltf(assets: &Path, id: &str) -> Option<PathBuf> {
    let t = catalog::tree(id)?;
    let dir = Unit::from(t).dir(assets);
    let manifest = read_manifest(&dir)?;
    let path = dir.join(manifest.file("gltf")?);
    path.is_file().then_some(path)
}

fn marble_unit(id: &str) -> Option<Unit> {
    catalog::MARBLES.iter().find(|m| m.id == id).map(Unit::from)
}

// ── provisioning ────────────────────────────────────────────────────────────

/// Resolve `unit` through the API, download every file (resumable, retried,
/// MD5- and size-checked) and write its manifest. Returns the manifest.
pub fn provision_unit(
    api: &Api,
    assets: &Path,
    unit: &Unit,
    displacement: bool,
    backoff: Duration,
    say: &(dyn Fn(&str, &str) + Sync),
) -> Result<Manifest, String> {
    say(unit.id, &format!("{}: resolving files", unit.label));
    let files = api.get_json(&format!("files/{}", unit.id))?;
    let info = api.get_json(&format!("info/{}", unit.id)).ok();
    let plan = match unit.kind {
        UnitKind::Tree => plan_gltf_package(&files, unit.resolutions),
        UnitKind::Marble => plan_texture_set(&files, unit.resolutions, displacement),
    }
    .map_err(|e| format!("{}: {e}", unit.id))?;
    for warning in pin_warnings(unit.id, &plan) {
        say(unit.id, &warning);
    }
    say(
        unit.id,
        &format!(
            "{}: {} files, {} MB at {}",
            unit.label,
            plan.files.len(),
            plan.total_bytes() >> 20,
            plan.resolution
        ),
    );

    let dir = unit.dir(assets);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut queue: Vec<&PlannedFile> = plan.files.iter().collect();
    queue.sort_by_key(|f| std::cmp::Reverse(f.remote.size));
    let next = AtomicUsize::new(0);
    let errors = Mutex::new(Vec::<String>::new());
    std::thread::scope(|scope| {
        for _ in 0..queue.len().min(4) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(f) = queue.get(i) else { break };
                let dest = dir.join(&f.remote.rel);
                let label = format!("{}/{}", unit.id, f.remote.rel);
                if let Err(e) = download::fetch_checked(
                    &[&f.remote.url],
                    &dest,
                    f.remote.size,
                    Checksum::Md5(&f.remote.md5),
                    backoff,
                    &|m| say(&label, m),
                ) {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    let manifest = Manifest {
        recipe: unit.recipe(),
        id: unit.id.to_string(),
        name: info
            .as_ref()
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(unit.label)
            .to_string(),
        resolution: plan.resolution.clone(),
        authors: info.as_ref().map(authors).unwrap_or_default(),
        files: plan
            .files
            .iter()
            .map(|f| {
                (
                    f.remote.rel.clone(),
                    f.role.clone(),
                    f.remote.size,
                    f.remote.md5.clone(),
                )
            })
            .collect(),
    };
    let tmp = dir.join(format!("{MANIFEST_NAME}.tmp"));
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&manifest.to_json()).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(MANIFEST_NAME)).map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// The credits text for every provisioned Poly Haven unit.
pub fn credits_markdown(manifests: &[Manifest]) -> String {
    let mut s = String::new();
    s.push_str("# Poly Haven assets used by the VG museum\n\n");
    s.push_str(&format!(
        "Generated by `vg_stress_prep` when the assets were downloaded. All assets below are {LICENCE}; see {LICENCE_URL}. \
Attribution is not required by the licence and is given here with thanks to the artists and to Poly Haven (https://polyhaven.com), \
whose API and CDN deliver the files.\n\n"
    ));
    for m in manifests {
        let who = if m.authors.is_empty() {
            "Poly Haven".to_string()
        } else {
            m.authors
                .iter()
                .map(|(n, r)| {
                    if r.is_empty() {
                        n.clone()
                    } else {
                        format!("{n} ({r})")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        s.push_str(&format!(
            "- **{}** (`{}`, {}): {} - https://polyhaven.com/a/{}\n",
            m.name, m.id, m.resolution, who, m.id
        ));
    }
    s.push_str("\nThe Stanford scans are covered by their own notice in `CREDITS.md`.\n");
    s
}

/// Rewrite `assets/vg_stress/CREDITS_POLYHAVEN.md` from the manifests on disk.
pub fn write_credits(assets: &Path, config: &catalog::Config) -> Result<(), String> {
    let manifests: Vec<Manifest> = units(config)
        .iter()
        .filter_map(|u| read_manifest(&u.dir(assets)))
        .collect();
    if manifests.is_empty() {
        return Ok(());
    }
    std::fs::write(
        assets.join("CREDITS_POLYHAVEN.md"),
        credits_markdown(&manifests),
    )
    .map_err(|e| e.to_string())
}

/// `{unit key: recipe}` entries for `prepared.json`.
pub fn recipes(config: &catalog::Config) -> BTreeMap<String, String> {
    units(config)
        .iter()
        .map(|u| (u.key(), u.recipe()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    fn fixture(name: &str) -> Value {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("vg_stress_prep/fixtures")
                .join(name),
        )
        .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vgprep-ph-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ── plans from real captured API responses ──────────────────────────────

    #[test]
    fn gltf_package_keeps_the_directory_structure_and_roles() {
        let plan = plan_gltf_package(&fixture("files_island_tree_01.json"), &["2k", "1k"]).unwrap();
        assert_eq!(plan.resolution, "2k");
        assert_eq!(plan.files.len(), 12);
        assert_eq!(plan.files[0].remote.rel, "island_tree_01_2k.gltf");
        assert_eq!(plan.files[0].role, "gltf");
        assert!(plan.files.iter().any(|f| f.role == "bin"
            && f.remote.rel == "island_tree_01.bin"
            && f.remote.size == 60_709_812));
        let textures: Vec<_> = plan.files.iter().filter(|f| f.role == "texture").collect();
        assert_eq!(textures.len(), 9);
        assert!(textures
            .iter()
            .all(|f| f.remote.rel.starts_with("textures/") && f.remote.rel.ends_with("_2k.jpg")));
        assert!(plan
            .files
            .iter()
            .all(|f| f.remote.url.starts_with("https://dl.polyhaven.org/")));
        assert_eq!(plan.total_bytes(), 79_783_969 + 139_077);
        assert!(plan.files.iter().any(|f| f.role == "opacity"
            && f.remote.rel == "textures/island_tree_01_leaves_alpha_2k.png"));
        // Same capture the pins came from: nothing differs.
        assert!(pin_warnings("island_tree_01", &plan).is_empty());
    }

    #[test]
    fn gltf_package_falls_back_to_a_lower_resolution_and_errors_when_none() {
        let mut files = fixture("files_island_tree_01.json");
        files["gltf"].as_object_mut().unwrap().remove("2k");
        let plan = plan_gltf_package(&files, &["2k", "1k"]).unwrap();
        assert_eq!(plan.resolution, "1k");
        assert!(plan.files[0].remote.rel.ends_with("_1k.gltf"));
        files["gltf"].as_object_mut().unwrap().remove("1k");
        assert!(plan_gltf_package(&files, &["2k", "1k"]).is_err());
        assert!(plan_gltf_package(&json!({}), &["2k"]).is_err());
    }

    #[test]
    fn texture_set_picks_8k_jpg_diffuse_normal_arm_without_displacement() {
        let plan =
            plan_texture_set(&fixture("files_marble_01.json"), &["8k", "4k", "2k"], false).unwrap();
        assert_eq!(plan.resolution, "8k");
        let roles: Vec<_> = plan.files.iter().map(|f| f.role.as_str()).collect();
        assert_eq!(roles, ["diffuse", "normal", "arm"]);
        let names: Vec<_> = plan.files.iter().map(|f| f.remote.rel.as_str()).collect();
        assert_eq!(
            names,
            [
                "marble_01_diff_8k.jpg",
                "marble_01_nor_gl_8k.jpg",
                "marble_01_arm_8k.jpg"
            ]
        );
        assert_eq!(plan.files[0].remote.size, 17_078_773);
        assert_eq!(plan.files[0].remote.md5, "f140d2527f16f2c9854a6b171ea9bf54");
        assert!(pin_warnings("marble_01", &plan).is_empty());

        let with_disp = plan_texture_set(&fixture("files_marble_01.json"), &["8k"], true).unwrap();
        assert_eq!(with_disp.files.last().unwrap().role, "displacement");
        assert_eq!(
            with_disp.files.last().unwrap().remote.rel,
            "marble_01_disp_8k.jpg"
        );
    }

    #[test]
    fn texture_set_skips_16k_and_falls_back_to_4k_when_8k_is_not_offered() {
        let files = fixture("files_grey_cartago_01.json");
        assert_eq!(
            plan_texture_set(&files, &["8k", "4k", "2k"], false)
                .unwrap()
                .resolution,
            "8k"
        );
        let mut no8k = files.clone();
        for map in no8k.as_object_mut().unwrap().values_mut() {
            map.as_object_mut().unwrap().remove("8k");
        }
        let plan = plan_texture_set(&no8k, &["8k", "4k", "2k"], false).unwrap();
        assert_eq!(plan.resolution, "4k");
        assert!(plan.files.iter().all(|f| f.remote.rel.ends_with("_4k.jpg")));
        // One missing required map at 8k drops the whole set to 4k (consistent sets).
        let mut partial = files.clone();
        partial["arm"].as_object_mut().unwrap().remove("8k");
        assert_eq!(
            plan_texture_set(&partial, &["8k", "4k"], false)
                .unwrap()
                .resolution,
            "4k"
        );
        // Nothing usable at all is an error.
        partial["arm"].as_object_mut().unwrap().clear();
        assert!(plan_texture_set(&partial, &["8k", "4k"], false).is_err());
    }

    #[test]
    fn unsafe_paths_and_malformed_entries_are_rejected() {
        for bad in [
            "../evil.jpg",
            "/abs.jpg",
            "a//b",
            "a\\b",
            "C:/x",
            "textures/../../x",
            "",
        ] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert!(safe_rel("textures/a_b.jpg").is_ok());
        let mut files = fixture("files_island_tree_01.json");
        let include = files["gltf"]["2k"]["gltf"]["include"]
            .as_object_mut()
            .unwrap();
        let first = include.values().next().unwrap().clone();
        include.insert("../escape.bin".into(), first);
        assert!(plan_gltf_package(&files, &["2k"]).is_err());
        let mut files = fixture("files_island_tree_01.json");
        files["gltf"]["2k"]["gltf"]["md5"] = json!("zz");
        assert!(plan_gltf_package(&files, &["2k"]).is_err());
    }

    #[test]
    fn authors_and_credits_come_from_the_info_endpoint() {
        let info = fixture("info_island_tree_01.json");
        let a = authors(&info);
        assert_eq!(
            a,
            [
                (
                    "Rico Cilliers".to_string(),
                    "cleanup, processing".to_string()
                ),
                ("Rob Tuytel".to_string(), "scanning, processing".to_string())
            ]
        );
        let manifest = Manifest {
            recipe: "r".into(),
            id: "island_tree_01".into(),
            name: info["name"].as_str().unwrap().into(),
            resolution: "2k".into(),
            authors: a,
            files: vec![],
        };
        let md = credits_markdown(&[manifest]);
        assert!(
            md.contains("Island Tree 01") && md.contains("Rob Tuytel (scanning, processing)"),
            "{md}"
        );
        assert!(md.contains("CC0") && md.contains("https://polyhaven.com/a/island_tree_01"));
        assert!(md.contains("Stanford"));
    }

    #[test]
    fn pins_cover_every_catalogue_entry_and_match_the_expected_sizes() {
        for t in catalog::TREES {
            let (res, files) = &pins().assets[t.id];
            assert_eq!(res, "2k", "{}", t.id);
            assert!(
                files.keys().any(|r| r.ends_with(".gltf"))
                    && files.keys().any(|r| r.ends_with(".bin")),
                "{}",
                t.id
            );
            assert!(
                files.keys().filter(|r| r.starts_with("textures/")).count() >= 9,
                "{}",
                t.id
            );
        }
        for m in catalog::MARBLES {
            let (res, files) = &pins().assets[m.id];
            assert_eq!(res, "8k");
            assert_eq!(files.len(), 4, "{}", m.id);
        }
        // pine: 948,849,556 byte .bin + textures; HEAD on the CDN reported the same .bin length.
        assert!(pins().assets["pine_tree_01"].1["pine_tree_01.bin"].0 == 948_849_556);
        assert!((970_000_000..990_000_000).contains(&pinned_bytes("pine_tree_01")));
        assert!((470_000_000..520_000_000).contains(&pinned_bytes("fir_tree_01")));
        assert!(
            (76_000_000..82_000_000).contains(&pinned_bytes("grey_cartago_01")),
            "{}",
            pinned_bytes("grey_cartago_01")
        );
    }

    // ── end to end against a local fixture server ───────────────────────────

    fn spawn_server(
        listener: TcpListener,
        routes: HashMap<String, Vec<u8>>,
        require_user_agent: bool,
    ) {
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                let mut user_agent = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("user-agent:") {
                        user_agent = v.trim().to_string();
                    }
                }
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                // The Poly Haven API terms ask clients to identify themselves.
                if require_user_agent && !user_agent.contains("pulsarengine") {
                    let _ = s.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                match routes.get(&path) {
                    Some(body) => {
                        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                        let _ = s.write_all(body);
                    }
                    None => {
                        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
    }

    /// Serve `routes` (path -> body); anything else is a 404.
    fn serve_routes(routes: HashMap<String, Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        spawn_server(listener, routes, false);
        base
    }

    /// Point every file entry of a real captured file list at a local body of
    /// a few hundred bytes whose size and MD5 are recomputed, registering the
    /// bodies in `routes`.
    fn localize(node: &mut Value, base: &str, routes: &mut HashMap<String, Vec<u8>>) {
        match node {
            Value::Object(map) => {
                if map.contains_key("url") && map.contains_key("md5") && map.contains_key("size") {
                    let index = routes.len();
                    let name = file_name(map["url"].as_str().unwrap()).to_string();
                    let body: Vec<u8> = (0..300 + index * 17)
                        .map(|i| ((i * 7 + index) % 251) as u8)
                        .collect();
                    let path = format!("/dl/{index}/{name}");
                    map.insert("url".into(), json!(format!("{base}{path}")));
                    map.insert("md5".into(), json!(super::super::md5::hex(&body)));
                    map.insert("size".into(), json!(body.len()));
                    routes.insert(path, body);
                }
                for v in map.values_mut() {
                    localize(v, base, routes);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|v| localize(v, base, routes)),
            _ => {}
        }
    }

    /// A local stand-in for the Poly Haven API serving the captured responses
    /// for `id`, with every file entry rewritten to a small local body.
    /// `tamper` can corrupt the (already localised) file list.
    fn api_for(
        id: &str,
        files_fixture: &str,
        info_fixture: &str,
        tamper: &dyn Fn(&mut Value),
    ) -> String {
        api_for_many(&[(id, files_fixture, info_fixture)], tamper)
    }

    /// Several ids, each answered with the given captured responses.
    fn api_for_many(entries: &[(&str, &str, &str)], tamper: &dyn Fn(&mut Value)) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut routes = HashMap::new();
        for (id, files_fixture, info_fixture) in entries {
            let mut files = fixture(files_fixture);
            localize(&mut files, &base, &mut routes);
            tamper(&mut files);
            routes.insert(format!("/files/{id}"), serde_json::to_vec(&files).unwrap());
            routes.insert(
                format!("/info/{id}"),
                serde_json::to_vec(&fixture(info_fixture)).unwrap(),
            );
        }
        spawn_server(listener, routes, true);
        base
    }

    fn island_unit() -> Unit {
        Unit::from(catalog::tree("island_tree_01").unwrap())
    }

    #[test]
    fn provisions_a_gltf_package_with_directory_structure_manifest_and_credits() {
        let base = api_for(
            "island_tree_01",
            "files_island_tree_01.json",
            "info_island_tree_01.json",
            &|_| {},
        );
        let assets = tmp("tree");
        let api = Api::new(base, Duration::from_millis(1));
        let unit = island_unit();
        assert!(!unit_complete(&assets, &unit, false));
        let manifest = provision_unit(
            &api,
            &assets,
            &unit,
            false,
            Duration::from_millis(1),
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(manifest.files.len(), 12);
        assert_eq!(manifest.resolution, "2k");
        assert_eq!(manifest.authors.len(), 2);

        let dir = unit.dir(&assets);
        assert!(dir.join("island_tree_01_2k.gltf").is_file());
        assert!(dir.join("island_tree_01.bin").is_file());
        assert!(dir.join("textures/island_tree_01_diff_2k.jpg").is_file());
        for (rel, _, size, md5) in &manifest.files {
            let bytes = std::fs::read(dir.join(rel)).unwrap();
            assert_eq!(bytes.len() as u64, *size);
            assert_eq!(&super::super::md5::hex(&bytes), md5);
        }
        assert!(unit_complete(&assets, &unit, false));
        assert_eq!(
            tree_gltf(&assets, "island_tree_01").unwrap(),
            dir.join("island_tree_01_2k.gltf")
        );
        assert_eq!(read_manifest(&dir).unwrap(), manifest);

        // A damaged file (wrong size) makes the unit incomplete again.
        std::fs::write(dir.join("island_tree_01.bin"), b"x").unwrap();
        assert!(!unit_complete(&assets, &unit, false));

        write_credits(
            &assets,
            &catalog::Config {
                trees: vec![catalog::tree("island_tree_01").unwrap()],
                ..Default::default()
            },
        )
        .unwrap();
        let credits = std::fs::read_to_string(assets.join("CREDITS_POLYHAVEN.md")).unwrap();
        assert!(
            credits.contains("Rob Tuytel") && credits.contains("CC0"),
            "{credits}"
        );
    }

    #[test]
    fn provisions_a_marble_set_and_resolves_maps_by_role() {
        let base = api_for(
            "marble_01",
            "files_marble_01.json",
            "info_marble_01.json",
            &|_| {},
        );
        let assets = tmp("marble");
        let api = Api::new(base, Duration::from_millis(1));
        let unit = Unit::from(
            catalog::MARBLES
                .iter()
                .find(|m| m.id == "marble_01")
                .unwrap(),
        );
        let manifest = provision_unit(
            &api,
            &assets,
            &unit,
            false,
            Duration::from_millis(1),
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(manifest.resolution, "8k");
        assert_eq!(manifest.files.len(), 3);
        assert!(marble_map(&assets, "marble_01", "diffuse")
            .unwrap()
            .ends_with("marble_01_diff_8k.jpg"));
        assert!(marble_map(&assets, "marble_01", "arm").is_some());
        assert!(marble_map(&assets, "marble_01", "displacement").is_none());
        assert!(unit_complete(&assets, &unit, false));
        // Asking for displacement later means the set is not complete yet.
        assert!(!unit_complete(&assets, &unit, true));
        provision_unit(
            &api,
            &assets,
            &unit,
            true,
            Duration::from_millis(1),
            &|_, _| {},
        )
        .unwrap();
        assert!(unit_complete(&assets, &unit, true));
        assert!(marble_map(&assets, "marble_01", "displacement")
            .unwrap()
            .ends_with("marble_01_disp_8k.jpg"));
    }

    #[test]
    fn a_hash_mismatch_fails_the_unit_and_writes_no_manifest() {
        let base = api_for(
            "island_tree_01",
            "files_island_tree_01.json",
            "info_island_tree_01.json",
            &|files| {
                files["gltf"]["2k"]["gltf"]["include"]["island_tree_01.bin"]["md5"] =
                    json!("00000000000000000000000000000000");
            },
        );
        let assets = tmp("mismatch");
        let api = Api::new(base, Duration::from_millis(1));
        let unit = island_unit();
        let err = provision_unit(
            &api,
            &assets,
            &unit,
            false,
            Duration::from_millis(1),
            &|_, _| {},
        )
        .unwrap_err();
        assert!(
            err.contains("MD5 mismatch") && err.contains("island_tree_01.bin"),
            "{err}"
        );
        assert!(read_manifest(&unit.dir(&assets)).is_none());
        assert!(!unit.dir(&assets).join("island_tree_01.bin").exists());
        assert!(!unit_complete(&assets, &unit, false));
    }

    #[test]
    fn unknown_assets_are_an_error_and_clients_must_identify_themselves() {
        let base = api_for(
            "island_tree_01",
            "files_island_tree_01.json",
            "info_island_tree_01.json",
            &|_| {},
        );
        let api = Api::new(base.clone(), Duration::from_millis(1));
        assert!(api
            .get_json("files/not_an_asset")
            .unwrap_err()
            .contains("404"));
        assert!(api.get_json("files/island_tree_01").is_ok());
        // A bare request without a User-Agent is refused by the fixture, as the real API asks.
        let bare = ureq::AgentBuilder::new().user_agent("").build();
        assert!(bare
            .get(&format!("{base}/files/island_tree_01"))
            .call()
            .is_err());
    }

    #[test]
    fn md5_checked_download_rejects_a_wrong_hash() {
        let body: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let base = serve_routes(HashMap::from([("/f.bin".to_string(), body.clone())]));
        let dir = tmp("md5");
        let url = format!("{base}/f.bin");
        let err = download::fetch_checked(
            &[&url],
            &dir.join("f.bin"),
            body.len() as u64,
            Checksum::Md5("00000000000000000000000000000000"),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap_err();
        assert!(err.contains("MD5 mismatch"), "{err}");
        assert!(!dir.join("f.bin").exists());
        download::fetch_checked(
            &[&url],
            &dir.join("f.bin"),
            body.len() as u64,
            Checksum::Md5(&super::super::md5::hex(&body)),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(dir.join("f.bin")).unwrap(), body);
        assert!(dir.join("f.bin.md5").is_file());
    }

    #[test]
    fn provisioning_all_units_records_recipes_and_writes_credits_and_is_idempotent() {
        use super::super::{
            missing_units, provision_polyhaven, read_manifest as read_prepared, Lock,
        };
        let config = catalog::Config {
            trees: vec![catalog::tree("island_tree_01").unwrap()],
            ..Default::default()
        };
        let assets = tmp("all");
        assert_eq!(missing_units(&assets, &config).len(), 5);
        let mut entries = vec![(
            "island_tree_01",
            "files_island_tree_01.json",
            "info_island_tree_01.json",
        )];
        // The captured marble_01 answer stands in for every marble set.
        for m in catalog::MARBLES {
            entries.push((m.id, "files_marble_01.json", "info_marble_01.json"));
        }
        let api = Api::new(api_for_many(&entries, &|_| {}), Duration::from_millis(1));
        let lock = Lock::acquire(assets.join("source").join(".provision.lock"), &|_| {}).unwrap();
        let todo = missing_units(&assets, &config);
        let errors = provision_polyhaven(&assets, &config, &api, &todo, &|_| {}, &lock);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(missing_units(&assets, &config).is_empty());
        let prepared = read_prepared(&assets);
        assert_eq!(
            prepared["ph:marble_01"],
            Unit::from(&catalog::MARBLES[2]).recipe()
        );
        assert!(prepared["ph:island_tree_01"].starts_with(&format!("ph{PH_RECIPE_VERSION}:")));
        let credits = std::fs::read_to_string(assets.join("CREDITS_POLYHAVEN.md")).unwrap();
        assert!(
            credits.contains("Island Tree 01") && credits.contains("Marble 01"),
            "{credits}"
        );
        // A recipe bump makes everything stale again (existing users re-provision).
        let mut stale = prepared.clone();
        stale.insert("ph:marble_01".into(), "ph0:old".into());
        std::fs::write(
            assets.join("meshes/prepared.json"),
            super::super::serde_json_lite::write_map(&stale),
        )
        .unwrap();
        let again = missing_units(&assets, &config);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, "marble_01");
    }
}
