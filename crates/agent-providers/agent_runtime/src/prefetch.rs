//! Speculative context prefetch: while the user types, a cheap local lookup
//! suggests a handful of files or symbols they might want to attach.
//!
//! No model is called and no tokens are spent. The lookup matches words of the
//! draft message against file names and against definitions and mentions
//! inside source files of the workspace. Nothing is ever attached on its own:
//! the panel shows the result as a tray, and a click turns a suggestion into
//! an `@file` reference the user can see and delete.
//!
//! It is bounded: at most [`MAX_FILES`] files are listed, [`MAX_FILE_BYTES`]
//! per file and [`MAX_TOTAL_BYTES`] overall are read, and the search stops at
//! [`TIME_BUDGET`]. Off by default (`.pulsar/agent_prefetch.json`).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

pub const PREFETCH_SETTINGS_FILE: &str = "agent_prefetch.json";
pub const MAX_SUGGESTIONS: usize = 5;
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 14;
const MAX_FILE_BYTES: u64 = 200_000;
const MAX_TOTAL_BYTES: u64 = 6_000_000;
const TIME_BUDGET: Duration = Duration::from_millis(180);
const MIN_SCORE: u32 = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefetchSettings {
    /// Off by default.
    pub enabled: bool,
}

impl PrefetchSettings {
    pub fn path(project_root: &Path) -> PathBuf {
        project_root.join(".pulsar").join(PREFETCH_SETTINGS_FILE)
    }

    pub fn load(project_root: &Path) -> Self {
        std::fs::read_to_string(Self::path(project_root))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, project_root: &Path) -> std::io::Result<()> {
        let path = Self::path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }
}

/// One thing the user might want to attach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Workspace-relative, `/`-separated.
    pub path: String,
    /// 1-based line of the best match, when it came from file contents.
    pub line: Option<usize>,
    /// The definition that matched (`fn camera_update`).
    pub symbol: Option<String>,
    /// Why it was suggested, for the tooltip.
    pub reason: String,
    pub score: u32,
}

impl Suggestion {
    /// The reference the panel inserts; `expand_file_references` resolves it.
    pub fn reference(&self) -> String {
        format!("@{}", self.path)
    }

    /// Short chip text.
    pub fn label(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        match &self.symbol {
            Some(symbol) => format!("{name} \u{b7} {symbol}"),
            None => name.to_string(),
        }
    }
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "how", "what", "where", "when", "why", "this", "that", "from", "into",
    "please", "can", "you", "could", "would", "should", "does", "are", "was", "were", "not", "but",
    "all", "any", "has", "have", "had", "its", "our", "your", "out", "use", "using", "make", "need",
    "want", "like", "just", "also", "then", "than", "there", "here", "some", "more", "get", "set",
    "let", "file", "files", "code", "show", "tell", "about", "which", "while", "they", "them",
];

const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", "build", "out", "vendor", "third-party"];
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "txt", "json", "ron", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "c", "cc", "cpp", "h",
    "hpp", "cs", "java", "go", "lua", "glsl", "wgsl", "hlsl", "shader", "vert", "frag", "comp", "sh", "ps1",
];
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn", "struct", "enum", "trait", "type", "const", "static", "mod", "class", "def", "function", "interface",
    "impl",
];

/// Words of `text` worth searching for: lowercase, at least three characters,
/// not common English, `@references` left out. Snake_case and camelCase
/// identifiers are kept whole and also split into their parts.
pub fn keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |word: String| {
        if word.chars().count() >= 3 && !STOPWORDS.contains(&word.as_str()) && !out.contains(&word) {
            out.push(word);
        }
    };
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '@' || c == '/' || c == '.')) {
        if raw.starts_with('@') || raw.is_empty() {
            continue;
        }
        let raw = raw.trim_matches(|c| c == '.' || c == '/');
        let whole = raw.to_lowercase();
        if whole.is_empty() {
            continue;
        }
        let parts = split_identifier(raw);
        if parts.len() > 1 {
            push(whole);
        }
        for part in parts {
            push(part);
        }
    }
    out.truncate(8);
    out
}

fn split_identifier(raw: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in raw.chars() {
        if c == '_' || c == '.' || c == '/' {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

struct Candidate {
    rel: String,
    name_score: u32,
    size: u64,
}

fn list_files(root: &Path) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if files.len() >= MAX_FILES {
                return files;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_DEPTH && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push((entry.path(), depth + 1));
                }
            } else if kind.is_file() {
                let is_text = Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| TEXT_EXTENSIONS.contains(&e.to_lowercase().as_str()));
                if !is_text {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/"))
                else {
                    continue;
                };
                files.push((rel, entry.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    files
}

fn name_score(rel: &str, keywords: &[String]) -> u32 {
    let lower = rel.to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let mut score = 0;
    for word in keywords {
        if stem == word {
            score += 12;
        } else if stem.contains(word.as_str()) {
            score += 6;
        } else if lower.contains(word.as_str()) {
            score += 2;
        }
    }
    score
}

/// `fn name`, `pub struct Name`, `class Name`, ... -> the defined identifier
/// and the keyword.
fn definition_on(line: &str) -> Option<(String, String)> {
    let mut words = line.split_whitespace().peekable();
    // Visibility and modifiers.
    while let Some(word) = words.peek() {
        if word.starts_with("pub") || matches!(*word, "async" | "unsafe" | "export" | "default" | "extern" | "public" | "private" | "static" | "final") {
            words.next();
        } else {
            break;
        }
    }
    let keyword = words.next()?;
    if !DEFINITION_KEYWORDS.contains(&keyword) {
        return None;
    }
    let ident: String = words
        .next()?
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!ident.is_empty()).then(|| (keyword.to_string(), ident))
}

/// Up to `limit` files or symbols that look relevant to `query`. `exclude`
/// holds paths that are already referenced in the draft.
pub fn suggest(root: &Path, query: &str, exclude: &[String], limit: usize) -> Vec<Suggestion> {
    let words = keywords(query);
    if words.is_empty() {
        return Vec::new();
    }
    let started = Instant::now();
    let excluded: HashSet<String> = exclude
        .iter()
        .map(|p| p.trim_start_matches("./").replace('\\', "/"))
        .collect();

    let mut candidates: Vec<Candidate> = list_files(root)
        .into_iter()
        .filter(|(rel, _)| !excluded.contains(rel))
        .map(|(rel, size)| Candidate {
            name_score: name_score(&rel, &words),
            rel,
            size,
        })
        .collect();
    // Name matches first, then smaller files in source folders (cheap to read,
    // most likely to hold definitions).
    candidates.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.name_score),
            !c.rel.contains("/src/") && !c.rel.starts_with("src/"),
            c.size,
        )
    });

    let mut found: Vec<Suggestion> = Vec::new();
    let mut read_bytes = 0u64;
    for candidate in candidates {
        let mut score = candidate.name_score;
        let mut symbol: Option<(String, usize, u32)> = None;
        let mut mentions = 0u32;
        let mut first_mention: Option<usize> = None;

        let within_budget = started.elapsed() < TIME_BUDGET && read_bytes < MAX_TOTAL_BYTES;
        if within_budget && candidate.size > 0 && candidate.size <= MAX_FILE_BYTES {
            if let Ok(text) = std::fs::read_to_string(root.join(&candidate.rel)) {
                read_bytes += candidate.size;
                for (index, line) in text.lines().enumerate() {
                    let lower = line.to_lowercase();
                    if !words.iter().any(|w| lower.contains(w.as_str())) {
                        continue;
                    }
                    if let Some((keyword, ident)) = definition_on(line.trim_start()) {
                        let ident_lower = ident.to_lowercase();
                        let weight = words
                            .iter()
                            .map(|w| {
                                if ident_lower == *w {
                                    14
                                } else if ident_lower.contains(w.as_str()) {
                                    8
                                } else {
                                    0
                                }
                            })
                            .max()
                            .unwrap_or(0);
                        if weight > 0 && symbol.as_ref().is_none_or(|(_, _, best)| weight > *best) {
                            symbol = Some((format!("{keyword} {ident}"), index + 1, weight));
                        }
                    }
                    if mentions < 5 {
                        mentions += 1;
                        first_mention.get_or_insert(index + 1);
                    }
                }
            }
        }
        score += mentions * 2;
        if let Some((_, _, weight)) = &symbol {
            score += weight;
        }
        if score < MIN_SCORE {
            continue;
        }
        let reason = match (&symbol, candidate.name_score > 0) {
            (Some((definition, line, _)), _) => format!("defines `{definition}` (line {line})"),
            (None, true) => "file name matches your message".to_string(),
            (None, false) => "mentions words from your message".to_string(),
        };
        found.push(Suggestion {
            line: symbol.as_ref().map(|(_, line, _)| *line).or(first_mention),
            symbol: symbol.map(|(definition, _, _)| definition),
            path: candidate.rel,
            reason,
            score,
        });
    }
    found.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    found.truncate(limit);
    found
}

/// Paths of the `@references` already in a draft.
pub fn referenced_paths(draft: &str) -> Vec<String> {
    draft
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(
            root.join("src/camera.rs"),
            "pub struct Camera {\n}\n\npub fn update_camera_position(c: &mut Camera) {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/audio.rs"), "pub fn play_sound() {}\n").unwrap();
        std::fs::write(
            root.join("src/player.rs"),
            "use crate::camera::Camera;\nfn follow() { /* follows the camera */ }\n",
        )
        .unwrap();
        std::fs::write(root.join("target/debug/camera.rs"), "pub struct Camera;\n").unwrap();
        std::fs::write(root.join(".git/camera.rs"), "pub struct Camera;\n").unwrap();
        std::fs::write(root.join("logo.png"), [0u8; 8]).unwrap();
        dir
    }

    #[test]
    fn keywords_drop_noise_and_split_identifiers() {
        let words = keywords("How do I fix the updateCameraPosition bug in @src/other.rs?");
        assert!(words.contains(&"updatecameraposition".to_string()));
        assert!(words.contains(&"camera".to_string()));
        assert!(words.contains(&"position".to_string()));
        assert!(words.contains(&"bug".to_string()));
        assert!(!words.contains(&"the".to_string()));
        assert!(!words.iter().any(|w| w.contains("other")), "{words:?}");
    }

    #[test]
    fn finds_files_and_symbols_without_a_model() {
        let dir = workspace();
        let found = suggest(dir.path(), "the camera position is wrong", &[], MAX_SUGGESTIONS);
        assert!(!found.is_empty());
        assert_eq!(found[0].path, "src/camera.rs");
        assert!(found[0].symbol.as_deref().unwrap().to_lowercase().contains("camera"));
        assert_eq!(found[0].reference(), "@src/camera.rs");
        // The mention in another file is found, unrelated files and build
        // output, hidden folders and binaries are not.
        let paths: Vec<&str> = found.iter().map(|s| s.path.as_str()).collect();
        assert!(paths.contains(&"src/player.rs"), "{paths:?}");
        assert!(!paths.contains(&"src/audio.rs"));
        assert!(!paths.iter().any(|p| p.starts_with("target") || p.starts_with(".git") || p.ends_with(".png")));
        assert!(found.len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn already_referenced_files_and_empty_queries_give_nothing() {
        let dir = workspace();
        let found = suggest(
            dir.path(),
            "camera",
            &["src/camera.rs".to_string(), "src/player.rs".to_string()],
            5,
        );
        assert!(found.is_empty(), "{found:?}");
        assert!(suggest(dir.path(), "how is it", &[], 5).is_empty());
        assert_eq!(referenced_paths("look at @src/a.rs and @b.rs now"), vec!["src/a.rs", "b.rs"]);
    }

    #[test]
    fn settings_default_off_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!PrefetchSettings::load(dir.path()).enabled);
        PrefetchSettings { enabled: true }.save(dir.path()).unwrap();
        assert!(PrefetchSettings::load(dir.path()).enabled);
    }
}
