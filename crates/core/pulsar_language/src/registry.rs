//! The registry of the languages the project has: every [`Language`] whose
//! plugin is enabled.
//!
//! [`LanguageRegistry::current`] is a snapshot cached against
//! `pulsar_plugins::generation()`: enabling or disabling a plugin (or opening
//! a project with another plugin set) yields a new one, so a disabled
//! plugin's language, class markers and server are gone from it. Nothing here
//! names a language.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{extension_len, Language, LanguageServerSpec, ScriptLanguage};

/// The enabled languages, by id.
#[derive(Clone, Default)]
pub struct LanguageRegistry {
    languages: Vec<&'static Language>,
}

static CACHE: Mutex<Option<(u64, Arc<LanguageRegistry>)>> = Mutex::new(None);

impl LanguageRegistry {
    /// The languages of the enabled plugins now (cached until the plugin set
    /// changes).
    pub fn current() -> Arc<Self> {
        let generation = pulsar_plugins::generation();
        {
            let cache = CACHE.lock().unwrap_or_else(|poison| poison.into_inner());
            if let Some((cached, registry)) = cache.as_ref() {
                if *cached == generation {
                    return registry.clone();
                }
            }
        }
        // Built outside the lock: reading the registrations runs the linked
        // plugins' builds, which must not be able to deadlock on the cache.
        let registry = Arc::new(Self::from_languages(
            pulsar_plugins::registered::<Language>().collect(),
        ));
        *CACHE.lock().unwrap_or_else(|poison| poison.into_inner()) =
            Some((generation, registry.clone()));
        registry
    }

    /// A registry of these languages (what [`LanguageRegistry::current`]
    /// builds from the registrations, and what tests build by hand). Sorted
    /// by id; a second language with the same id is ignored and logged.
    pub fn from_languages(mut languages: Vec<&'static Language>) -> Self {
        languages.sort_by_key(|language| language.id);
        let mut unique: Vec<&'static Language> = Vec::with_capacity(languages.len());
        for language in languages {
            if unique.last().is_some_and(|last| last.id == language.id) {
                tracing::error!(
                    id = language.id,
                    origin = language.origin,
                    "language registered twice; the second is ignored"
                );
                continue;
            }
            unique.push(language);
        }
        Self { languages: unique }
    }

    pub fn all(&self) -> &[&'static Language] {
        &self.languages
    }

    pub fn by_id(&self, id: &str) -> Option<&'static Language> {
        self.languages
            .iter()
            .copied()
            .find(|language| language.id == id)
    }

    /// The language that owns `path` by its extension (the longest match
    /// wins when two languages claim `d.ts` and `ts`).
    pub fn for_path(&self, path: &Path) -> Option<&'static Language> {
        self.languages
            .iter()
            .copied()
            .filter_map(|language| {
                extension_len(language.file_extensions, path).map(|len| (len, language))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, language)| language)
    }

    /// The languages whose class marker is in `dir`: none for a directory that
    /// is not a language's class, more than one for a class that is two
    /// languages' (a problem: `pulsar_class::ClassProblem::MultipleLanguages`).
    pub fn class_languages(&self, dir: &Path) -> Vec<&'static Language> {
        self.languages
            .iter()
            .copied()
            .filter(|language| language.owns_class_dir(dir))
            .collect()
    }

    /// Whether `dir` holds any language's class marker.
    pub fn is_class_dir(&self, dir: &Path) -> bool {
        self.languages
            .iter()
            .any(|language| language.owns_class_dir(dir))
    }

    /// Every class marker file name of every language.
    pub fn class_markers(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.languages
            .iter()
            .flat_map(|language| language.class_markers.iter().copied())
    }

    /// The language that has `file_name` as a class marker.
    pub fn language_of_marker(&self, file_name: &str) -> Option<&'static Language> {
        self.languages
            .iter()
            .copied()
            .find(|language| language.class_markers.contains(&file_name))
    }

    /// The languages that compile scripts, with their compilers.
    pub fn compilers(&self) -> Vec<(&'static Language, Arc<dyn ScriptLanguage>)> {
        self.languages
            .iter()
            .copied()
            .filter_map(|language| language.compiler.map(|create| (language, create())))
            .collect()
    }

    /// The languages that have a language server.
    pub fn with_servers(
        &self,
    ) -> impl Iterator<Item = (&'static Language, &'static LanguageServerSpec)> + '_ {
        self.languages
            .iter()
            .copied()
            .filter_map(|language| language.server.map(|spec| (language, spec)))
    }

    /// The language and server spec that handle `path`.
    pub fn server_for_path(
        &self,
        path: &Path,
    ) -> Option<(&'static Language, &'static LanguageServerSpec)> {
        let language = self.for_path(path)?;
        language.server.map(|spec| (language, spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn language(
        id: &'static str,
        extensions: &'static [&'static str],
        markers: &'static [&'static str],
    ) -> Language {
        Language {
            origin: "engine:test",
            id,
            display_name: id,
            file_extensions: extensions,
            language_ids: &[("tsx", "typescriptreact")],
            highlight_id: "",
            class_markers: markers,
            config_files: &[],
            compiler: None,
            server: None,
        }
    }

    static TS: Language = language("ts", &["ts", "mts"], &["class.ts"]);
    static DTS: Language = language("dts", &["d.ts"], &[]);
    static GRAPH: Language = language("graph", &[], &["graph.json"]);

    fn registry() -> LanguageRegistry {
        LanguageRegistry::from_languages(vec![&TS, &DTS, &GRAPH])
    }

    #[test]
    fn a_path_goes_to_the_language_with_the_longest_extension() {
        let registry = registry();
        assert_eq!(registry.for_path(Path::new("/p/a.ts")).map(|l| l.id), Some("ts"));
        assert_eq!(registry.for_path(Path::new("/p/a.MTS")).map(|l| l.id), Some("ts"));
        assert_eq!(registry.for_path(Path::new("/p/a.d.ts")).map(|l| l.id), Some("dts"));
        assert!(registry.for_path(Path::new("/p/ts")).is_none());
        assert!(registry.for_path(Path::new("/p/.ts")).is_none());
        assert!(registry.for_path(Path::new("/p/readme.md")).is_none());
    }

    #[test]
    fn language_ids_follow_the_extension() {
        assert_eq!(TS.language_id_of(Path::new("a.tsx")), "typescriptreact");
        assert_eq!(TS.language_id_of(Path::new("a.ts")), "ts");
    }

    #[test]
    fn a_language_registered_twice_counts_once() {
        let registry = LanguageRegistry::from_languages(vec![&TS, &TS]);
        assert_eq!(registry.all().len(), 1);
    }

    #[test]
    fn markers_make_a_class_dir_and_name_their_language() {
        let dir =
            std::env::temp_dir().join(format!("pulsar_language_markers_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let registry = registry();
        assert!(!registry.is_class_dir(&dir));
        std::fs::write(dir.join("class.ts"), "").unwrap();
        assert!(registry.is_class_dir(&dir));
        assert_eq!(registry.class_languages(&dir).len(), 1);
        std::fs::write(dir.join("graph.json"), "").unwrap();
        let ids: Vec<_> = registry.class_languages(&dir).iter().map(|l| l.id).collect();
        assert_eq!(ids, ["graph", "ts"]);
        assert_eq!(
            registry.language_of_marker("graph.json").map(|l| l.id),
            Some("graph")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
