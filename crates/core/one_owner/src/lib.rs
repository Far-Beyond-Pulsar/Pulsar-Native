//! The one-owner rule (user, 2026-11-03; `docs/architecture/plugin-migration.md`
//! in the Helio repo, Phase C step 1).
//!
//! Every cross-kind concept has exactly one owner: a plugin
//! (`plugins/vendor/<kind>`) or the frame (the Helio repo: `helio-core`,
//! `helio-frame-api`, `helio-spine`, the scene join, ...). Everything else
//! reaches it through the owner's api crate (Rust) or the owner's WGSL
//! snippet (`//!use ...`), never through a private copy. This crate reads the
//! source tree and reports the copies:
//!
//! - **WGSL**: a struct, const or function another owner's snippet declares
//!   (same name), or a struct with the layout of another owner's snippet
//!   struct that holds a matrix (a camera, a light matrix under another
//!   name). The snippets are the files Rust declares with `wgsl_snippet!`,
//!   plus the prelude and the scene join's shared rows.
//! - **Rust**: a string literal naming a resource another owner defines as a
//!   key (`const X: &str = "..."`, `BufferContract::new("...")`, ...), where
//!   it is used as a resource name: in `ResourceKey::new`, `BufferKey::of`,
//!   graph builder reads and writes, pool lookups, or a pass's `reads` /
//!   `writes` list. Attribute literals (`#[gpu(buffer = "...")]`) and
//!   `same_layout!` take no const and are skipped (the batch 5 lesson).
//!
//! An owner's own files may repeat its own concepts (they adopt its snippets
//! as they change); test code, benches and example hosts are not checked.
//! What remains is listed in `exceptions.txt` with what each waits on; the
//! list may only shrink.

use std::collections::{BTreeMap, BTreeSet};

/// A source file, by its path relative to the Pulsar checkout (`/`-separated).
#[derive(Clone, Debug)]
pub struct Source {
    pub path: String,
    pub text: String,
}

/// One private copy of another owner's concept.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Copy {
    /// The file holding the copy.
    pub path: String,
    /// What is copied: `struct Camera`, `const MEDIA_NEAR`, `"pre_aa"`, ...
    pub symbol: String,
    /// The owner's definition.
    pub owner: String,
}

impl std::fmt::Display for Copy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} | {} | owned by {}",
            self.path, self.symbol, self.owner
        )
    }
}

/// Names any shader may define for itself: generic math, not a concept.
const GENERIC: &[&str] = &[
    "PI",
    "TAU",
    "INV_PI",
    "EPSILON",
    "pow5",
    "luminance",
    "saturate",
    "hash",
    "hash12",
    "hash21",
    "hash22",
    "hash3",
    "hash33",
    "noise3",
    "value_noise",
    "main",
    "vs_main",
    "fs_main",
    "cs_main",
];

/// The owner a file belongs to: `frame` (the Helio repo), `plugin:<kind>`
/// or `engine` (Pulsar's own crates); `None` for what is not checked.
pub fn owner_of(path: &str) -> Option<String> {
    if path.starts_with("crates/renderer/helio/") {
        return Some("frame".into());
    }
    if let Some(rest) = path.strip_prefix("plugins/vendor/") {
        let kind = rest.split('/').next()?;
        return Some(format!("plugin:{kind}"));
    }
    if path.starts_with("crates/core/") || path.starts_with("crates/subsystems/") {
        return Some("engine".into());
    }
    None
}

/// Whether `path` is code the rule covers (not a test, bench or example
/// host, not a third-party or UI framework file).
pub fn is_checked(path: &str) -> bool {
    let skip = [
        "/target/",
        "/tests/",
        "/benches/",
        "/examples/",
        "/web-demos/",
        "/android-demos/",
        "/third-party/",
        "/wgpui",
        "crates/renderer/helio/examples/",
        "crates/core/one_owner/",
    ];
    owner_of(path).is_some()
        && !skip.iter().any(|s| path.contains(s))
        && !path.ends_with("tests.rs")
        && !path.ends_with("_tests.rs")
}

// ── WGSL ────────────────────────────────────────────────────────────────────

fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ident(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

fn canon_type(t: &str) -> String {
    let mut t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    for (short, long) in [
        ("mat4x4f", "mat4x4<f32>"),
        ("mat3x3f", "mat3x3<f32>"),
        ("vec4f", "vec4<f32>"),
        ("vec3f", "vec3<f32>"),
        ("vec2f", "vec2<f32>"),
        ("vec4u", "vec4<u32>"),
        ("vec3u", "vec3<u32>"),
        ("vec2u", "vec2<u32>"),
        ("vec4i", "vec4<i32>"),
        ("vec2i", "vec2<i32>"),
    ] {
        if t == short {
            t = long.to_string();
        }
    }
    t
}

/// A WGSL module's top-level declarations: `(kind, name) -> field types`
/// (struct fields; empty for consts and functions).
pub fn wgsl_declarations(text: &str) -> BTreeMap<(&'static str, String), Vec<String>> {
    let text = strip_comments(text);
    let mut out = BTreeMap::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let line_end = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let line = &rest[..line_end];
        let trimmed = line.trim_start();
        let indented = trimmed.len() != line.len();
        if let Some(after) = trimmed.strip_prefix("struct ") {
            let name = ident(after.trim_start()).to_string();
            if let Some(open) = rest.find('{') {
                let mut depth = 0;
                let mut close = open;
                for (i, c) in rest[open..].char_indices() {
                    if c == '{' {
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            close = open + i;
                            break;
                        }
                    }
                }
                let body = &rest[open + 1..close];
                let fields = body
                    .split([',', '\n'])
                    .filter_map(|field| {
                        let field = field.trim();
                        // Attributes (`@location(0)`, `@builtin(...)`) before the name.
                        let field = field
                            .rsplit_once(')')
                            .filter(|(head, _)| head.trim_start().starts_with('@'))
                            .map_or(field, |(_, tail)| tail.trim());
                        let (_, ty) = field.split_once(':')?;
                        Some(canon_type(ty))
                    })
                    .collect();
                out.insert(("struct", name), fields);
                rest = &rest[close.min(rest.len() - 1) + 1..];
                continue;
            }
        } else if !indented {
            for (prefix, kind) in [("const ", "const"), ("override ", "const"), ("fn ", "fn")] {
                if let Some(after) = trimmed.strip_prefix(prefix) {
                    out.insert((kind, ident(after.trim_start()).to_string()), Vec::new());
                }
            }
        }
        rest = &rest[line_end..];
    }
    out
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// The snippet files: every `wgsl_snippet!(marker, "file")` the Rust
/// sources declare, plus the files the frame shares by other means.
pub fn snippet_files(sources: &[Source]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = [
        "crates/renderer/helio/crates/helio-core/src/shader/prelude.wgsl",
        "crates/renderer/helio/crates/helio-scene-join/shaders/scene_join_common.wgsl",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for source in sources.iter().filter(|s| s.path.ends_with(".rs")) {
        let dir = source.path.rsplit_once('/').map_or("", |(d, _)| d);
        let mut rest = source.text.as_str();
        while let Some(at) = rest.find("wgsl_snippet!(") {
            rest = &rest[at + "wgsl_snippet!(".len()..];
            let call = &rest[..rest.find(')').unwrap_or(rest.len())];
            if let Some(q) = call.find('"') {
                let file = &call[q + 1..];
                if let Some(end) = file.find('"') {
                    out.insert(normalize(&format!("{dir}/{}", &file[..end])));
                }
            }
        }
    }
    out
}

/// The WGSL copies among `sources`.
pub fn wgsl_copies(sources: &[Source]) -> Vec<Copy> {
    let snippets = snippet_files(sources);
    let mut owned: Vec<(String, String, (&'static str, String), Vec<String>)> = Vec::new();
    for source in sources.iter().filter(|s| snippets.contains(&s.path)) {
        let owner = owner_of(&source.path).unwrap_or_default();
        for (key, fields) in wgsl_declarations(&source.text) {
            owned.push((owner.clone(), source.path.clone(), key, fields));
        }
    }
    let mut copies = Vec::new();
    for source in sources
        .iter()
        .filter(|s| s.path.ends_with(".wgsl") && is_checked(&s.path) && !snippets.contains(&s.path))
    {
        let owner = owner_of(&source.path).unwrap_or_default();
        for ((kind, name), fields) in wgsl_declarations(&source.text) {
            if GENERIC.contains(&name.as_str()) {
                continue;
            }
            let mut found = None;
            for (snippet_owner, snippet_path, (snippet_kind, snippet_name), snippet_fields) in
                &owned
            {
                if *snippet_owner == owner {
                    continue;
                }
                let same_name = *snippet_kind == kind && *snippet_name == name;
                let same_layout = kind == "struct"
                    && *snippet_kind == "struct"
                    && fields.len() >= 3
                    && snippet_fields.len() >= fields.len()
                    && snippet_fields[..fields.len()] == fields[..]
                    && fields.iter().any(|f| f.starts_with("mat4x4"));
                if same_name || same_layout {
                    found = Some(format!("{snippet_path} ({snippet_kind} {snippet_name})"));
                    break;
                }
            }
            if let Some(owner) = found {
                copies.push(Copy {
                    path: source.path.clone(),
                    symbol: format!("{kind} {name}"),
                    owner,
                });
            }
        }
    }
    copies
}

// ── Rust ────────────────────────────────────────────────────────────────────

/// Rust code without its inline `#[cfg(test)] mod ... { }` (which runs to
/// the end of the file, the convention here); a `#[cfg(test)] mod tests;`
/// declaration or a test-only item keeps the rest.
fn without_tests(text: &str) -> &str {
    let mut from = 0;
    while let Some(at) = text[from..].find("#[cfg(test)]") {
        let at = from + at;
        let after = text[at + "#[cfg(test)]".len()..].trim_start();
        if let Some(module) = after.strip_prefix("mod ") {
            let brace = module.find('{');
            let semi = module.find(';');
            if brace.is_some() && (semi.is_none() || brace < semi) {
                return &text[..at];
            }
        }
        from = at + 1;
    }
    text
}

fn code_of(line: &str) -> &str {
    // `//` outside a string literal starts a comment.
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if line[i..].starts_with("//") {
            return &line[..i];
        }
    }
    line
}

fn literals(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(&after[..close]);
        rest = &after[close + 1..];
    }
    out
}

fn is_key(lit: &str) -> bool {
    lit.len() >= 3
        && lit
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Where a key is defined as one: `const X: &str = "key"` or a contract
/// constructor.
fn key_definitions(code: &str) -> Vec<&str> {
    let line = code.trim_start();
    let is_const = line.starts_with("const ")
        || line.starts_with("pub const ")
        || line.starts_with("pub(crate) const ");
    if !is_const {
        return Vec::new();
    }
    let Some((_, value)) = line.split_once('=') else {
        return Vec::new();
    };
    let value = value.trim();
    let direct = value.starts_with('"');
    let contract = [
        "BufferContract::new(",
        "PayloadContract::new(",
        "BufferKey::of(",
    ]
    .iter()
    .any(|c| value.contains(c));
    if direct || contract {
        literals(value).into_iter().filter(|l| is_key(l)).collect()
    } else {
        Vec::new()
    }
}

/// Calls whose string arguments are resource names.
const KEY_CALLS: &[&str] = &[
    "ResourceKey::new(",
    "BufferKey::of(",
    "BufferContract::new(",
    "PayloadContract::new(",
    ".read(",
    ".write(",
    "write_color",
    "write_buffer(",
    "write_group(",
    "with_group_extra_usage(",
    "get_view(",
    "get_view_2d(",
    "get_texture(",
    "read_texture_view(",
    "is_demanded(",
    ".demand(",
    "holds_rows(",
    "route_named_texture(",
    "declare_external_input(",
    "registry.contains(",
];

/// The code lines of `text` (comments removed), with each `const` item
/// that spans lines joined into one, so a key defined over several lines
/// reads as one definition.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        let code = code_of(line);
        let trimmed = code.trim_start();
        if open {
            let last = out.last_mut().unwrap();
            last.push(' ');
            last.push_str(trimmed);
            open = !code.contains(';');
            continue;
        }
        let starts_const = trimmed.starts_with("const ")
            || trimmed.starts_with("pub const ")
            || trimmed.starts_with("pub(crate) const ");
        out.push(code.to_string());
        open = starts_const && !code.contains(';');
    }
    out
}

/// The string-literal copies of other owners' keys among `sources`.
pub fn rust_copies(sources: &[Source]) -> Vec<Copy> {
    let mut keys: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for source in sources.iter().filter(|s| s.path.ends_with(".rs")) {
        let Some(owner) = owner_of(&source.path) else {
            continue;
        };
        for line in logical_lines(without_tests(&source.text)) {
            for key in key_definitions(&line) {
                keys.entry(key.to_string())
                    .or_default()
                    .insert((owner.clone(), source.path.clone()));
            }
        }
    }
    let mut copies = Vec::new();
    for source in sources
        .iter()
        .filter(|s| s.path.ends_with(".rs") && is_checked(&s.path))
    {
        let owner = owner_of(&source.path).unwrap_or_default();
        let mut list_depth: Option<i32> = None;
        for line in logical_lines(without_tests(&source.text)) {
            let code = line.as_str();
            let trimmed = code.trim_start();
            // A pass's `reads` / `writes` lists are resource names.
            if list_depth.is_none()
                && ["fn reads(", "fn writes(", "fn static_reads("]
                    .iter()
                    .any(|f| trimmed.contains(f))
            {
                list_depth = Some(0);
            }
            let in_list = list_depth.is_some();
            if let Some(depth) = list_depth.as_mut() {
                *depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
                if *depth <= 0 && code.contains('}') {
                    list_depth = None;
                }
            }
            if trimmed.starts_with("#[")
                || code.contains("same_layout!")
                || !key_definitions(code).is_empty()
                || !(in_list || KEY_CALLS.iter().any(|c| code.contains(c)))
            {
                continue;
            }
            for lit in literals(code) {
                let Some(owners) = keys.get(lit) else {
                    continue;
                };
                if owners.iter().any(|(o, _)| *o == owner) {
                    continue;
                }
                let (_, path) = owners.iter().next().unwrap();
                copies.push(Copy {
                    path: source.path.clone(),
                    symbol: format!("\"{lit}\""),
                    owner: path.clone(),
                });
            }
        }
    }
    copies.sort();
    copies.dedup();
    copies
}

/// Every copy among `sources`, sorted.
pub fn copies(sources: &[Source]) -> Vec<Copy> {
    let mut all = wgsl_copies(sources);
    all.extend(rust_copies(sources));
    all.sort();
    all.dedup();
    all
}

/// The recorded exceptions (`path | symbol | waits on ...` lines; `#`
/// comments): copies that stay until what each names lands.
pub fn exceptions(text: &str) -> BTreeSet<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split('|').map(str::trim);
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
}

/// Reads every `.wgsl` and `.rs` file under `dirs` (relative to `root`).
// The checker reads the engine's own source tree on the developer's
// machine, not a project: the disk directly.
#[allow(clippy::disallowed_methods)]
pub fn read_tree(root: &std::path::Path, dirs: &[&str]) -> Vec<Source> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<Source>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name.starts_with('.') || name == "node_modules" {
                    continue;
                }
                walk(root, &path, out);
            } else if name.ends_with(".wgsl") || name.ends_with(".rs") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(Source { path: rel, text });
                }
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        walk(root, &root.join(dir), &mut out);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
