//! The engine's git pins, carried into a generated game.
//!
//! A game is built outside the engine's workspace, so it has a `Cargo.lock` of
//! its own. A git dependency named by branch (the engine's wgpu fork:
//! `branch = "helio/v30.0.1"`) floats until a lockfile pins it, and a game's
//! lockfile is made once and then kept: it fell behind the engine's, and the game
//! resolved a wgpu without the acceleration-structure methods Helio calls
//! (`Blas::sizes`, `request_rebuild`): "no method named `sizes` found for
//! struct `wgpu::Blas`" in `helio-core`.
//!
//! So the engine's resolution travels with the game, two ways, both from the
//! engine's own `Cargo.lock`:
//!
//! 1. The baked `[patch]` and `[dependencies]` entries that name a git source by
//!    branch are written with the commit the engine's lock pins (`rev = "<sha>"`):
//!    the manifest itself cannot float ([`pin_spec`]).
//! 2. A game's existing `Cargo.lock` has every git package's commit set to the
//!    engine's when the same package from the same repository is pinned there
//!    ([`align_lock`]), so one repository is one commit in the editor and the
//!    game.
//!
//! This file is used by `build.rs` (to bake the pins) and by the generator (to
//! align a lockfile); it depends on `toml` only.

// Each side uses a part: `build.rs` bakes (`parse_lock`, `pin_spec`, `render`), the
// generator aligns (`parse_rendered`, `align_lock`).
#![allow(dead_code)]

/// One git package the engine's lockfile pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPin {
    pub name: String,
    /// The repository, [`normalize_url`]d.
    pub url: String,
    /// The full commit.
    pub commit: String,
}

/// A repository URL in a form two spellings of the same repository compare
/// equal in: no `git+` prefix, query or fragment, no trailing `.git` or `/`, the
/// doubled slash some manifests use to make a distinct source (`org//repo`)
/// collapsed, lower case.
pub fn normalize_url(url: &str) -> String {
    let url = url.strip_prefix("git+").unwrap_or(url);
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let mut rest = rest.to_owned();
    while rest.contains("//") {
        rest = rest.replace("//", "/");
    }
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    format!("{scheme}://{rest}").to_ascii_lowercase()
}

/// Every git package of a `Cargo.lock`, with its pinned commit.
pub fn parse_lock(lock: &str) -> Vec<GitPin> {
    let Ok(value) = toml::from_str::<toml::Value>(lock) else {
        return Vec::new();
    };
    let Some(packages) = value.get("package").and_then(toml::Value::as_array) else {
        return Vec::new();
    };
    packages
        .iter()
        .filter_map(|package| {
            let name = package.get("name")?.as_str()?;
            let source = package.get("source")?.as_str()?;
            let rest = source.strip_prefix("git+")?;
            let (location, commit) = rest.split_once('#')?;
            Some(GitPin {
                name: name.to_owned(),
                url: normalize_url(location),
                commit: commit.to_owned(),
            })
        })
        .collect()
}

/// `spec` (a dependency or patch entry), with a git source named by branch
/// rewritten to the commit the engine pinned for `name`. An entry that is not
/// a git table, already names a `rev` or `tag`, or has no pin, is returned
/// unchanged (a path patch is not a git source; a rev already cannot float).
pub fn pin_spec(spec: &toml::Value, name: &str, pins: &[GitPin]) -> toml::Value {
    let Some(table) = spec.as_table() else {
        return spec.clone();
    };
    let Some(git) = table.get("git").and_then(toml::Value::as_str) else {
        return spec.clone();
    };
    if table.contains_key("rev") || table.contains_key("tag") {
        return spec.clone();
    }
    let wanted = normalize_url(git);
    let package = table
        .get("package")
        .and_then(toml::Value::as_str)
        .unwrap_or(name);
    let Some(pin) = pins
        .iter()
        .find(|pin| pin.name == package && pin.url == wanted)
    else {
        return spec.clone();
    };
    let mut pinned = table.clone();
    pinned.remove("branch");
    pinned.insert("rev".into(), toml::Value::String(pin.commit.clone()));
    toml::Value::Table(pinned)
}

/// The pins as the text `build.rs` bakes: one `name<TAB>url<TAB>commit` per line.
pub fn render(pins: &[GitPin]) -> String {
    pins.iter()
        .map(|pin| format!("{}\t{}\t{}\n", pin.name, pin.url, pin.commit))
        .collect()
}

/// [`render`]'s inverse.
pub fn parse_rendered(text: &str) -> Vec<GitPin> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(GitPin {
                name: fields.next()?.to_owned(),
                url: fields.next()?.to_owned(),
                commit: fields.next()?.to_owned(),
            })
        })
        .collect()
}

/// A game's `Cargo.lock` with every git package the engine also pins moved to
/// the engine's commit. Returns the text and how many packages moved; the text
/// is unchanged (and the count 0) when the lock already agrees, is empty or is
/// not a lockfile.
pub fn align_lock(lock: &str, pins: &[GitPin]) -> (String, usize) {
    let mut out = String::with_capacity(lock.len());
    let mut moved = 0;
    let mut name: Option<String> = None;
    for line in lock.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            name = None;
        } else if let Some(value) = trimmed.strip_prefix("name = \"") {
            name = value.strip_suffix('"').map(str::to_owned);
        } else if let (Some(package), Some(source)) = (
            name.as_deref(),
            trimmed
                .strip_prefix("source = \"git+")
                .and_then(|rest| rest.strip_suffix('"')),
        ) {
            if let Some((location, commit)) = source.split_once('#') {
                let url = normalize_url(location);
                if let Some(pin) = pins
                    .iter()
                    .find(|pin| pin.name == package && pin.url == url)
                {
                    if pin.commit != commit {
                        let indent = &line[..line.len() - line.trim_start().len()];
                        let newline = &line[line.trim_end().len()..];
                        out.push_str(&format!(
                            "{indent}source = \"git+{location}#{}\"{newline}",
                            pin.commit
                        ));
                        moved += 1;
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
    }
    (out, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINE_LOCK: &str = r#"
version = 4

[[package]]
name = "wgpu"
version = "30.0.1"
source = "git+https://github.com/Far-Beyond-Pulsar/wgpu?branch=helio%2Fv30.0.1#205c219ee4ec66c18b9e7bca70ec5d82914c6cdf"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "pulsar_scenedb"
version = "0.1.0"
source = "git+https://github.com//Far-Beyond-Pulsar/SceneDB?rev=f9589128fa018570c768663ed6820c2f6874a00f#f9589128fa018570c768663ed6820c2f6874a00f"
"#;

    #[test]
    fn urls_compare_across_spellings() {
        assert_eq!(
            normalize_url("git+https://github.com//Far-Beyond-Pulsar/SceneDB?rev=x#y"),
            normalize_url("https://github.com/Far-Beyond-Pulsar/SceneDB.git/")
        );
        assert_ne!(
            normalize_url("https://github.com/a/wgpu"),
            normalize_url("https://github.com/a/naga")
        );
    }

    #[test]
    fn only_git_packages_are_pinned() {
        let pins = parse_lock(ENGINE_LOCK);
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "wgpu");
        assert_eq!(pins[0].commit, "205c219ee4ec66c18b9e7bca70ec5d82914c6cdf");
        assert_eq!(parse_rendered(&render(&pins)), pins);
    }

    #[test]
    fn a_branch_patch_becomes_the_engines_commit() {
        let pins = parse_lock(ENGINE_LOCK);
        let spec: toml::Value = toml::from_str(
            r#"x = { git = "https://github.com/Far-Beyond-Pulsar/wgpu", branch = "helio/v30.0.1" }"#,
        )
        .map(|v: toml::Value| v["x"].clone())
        .unwrap();
        let pinned = pin_spec(&spec, "wgpu", &pins);
        assert_eq!(
            pinned["rev"].as_str(),
            Some("205c219ee4ec66c18b9e7bca70ec5d82914c6cdf")
        );
        assert!(pinned.get("branch").is_none(), "a rev replaces the branch");
        // Not pinned in the lock: unchanged. A rev, a path: unchanged.
        assert_eq!(pin_spec(&spec, "naga", &pins), spec);
        let path: toml::Value = toml::from_str(r#"x = { path = "/a" }"#)
            .map(|v: toml::Value| v["x"].clone())
            .unwrap();
        assert_eq!(pin_spec(&path, "wgpu", &pins), path);
        assert_eq!(pin_spec(&pinned, "wgpu", &pins), pinned);
    }

    #[test]
    fn a_stale_game_lock_is_moved_to_the_engines_commit() {
        let pins = parse_lock(ENGINE_LOCK);
        let stale = ENGINE_LOCK.replace(
            "205c219ee4ec66c18b9e7bca70ec5d82914c6cdf",
            "2344ca70355200f886113d39434e5f1d4274828c",
        );
        let (fixed, moved) = align_lock(&stale, &pins);
        assert_eq!(moved, 1);
        assert_eq!(fixed, ENGINE_LOCK, "only the wgpu commit changed");
        let (again, moved) = align_lock(&fixed, &pins);
        assert_eq!((again.as_str(), moved), (ENGINE_LOCK, 0));
    }

    #[test]
    fn a_package_from_another_repository_is_left_alone() {
        let pins = parse_lock(ENGINE_LOCK);
        let other = ENGINE_LOCK.replace("Far-Beyond-Pulsar/wgpu", "someone/wgpu");
        let (kept, moved) = align_lock(&other, &pins);
        assert_eq!((kept.as_str(), moved), (other.as_str(), 0));
    }
}
