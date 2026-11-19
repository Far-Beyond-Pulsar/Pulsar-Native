//! Which language a class is, and the one-language-per-class rule.
//!
//! A class belongs to the language whose class marker is in its directory
//! (`pulsar_language::Language::class_markers`; the registry holds the enabled
//! languages). A class has exactly one language: two languages' markers in one
//! directory are a [`ClassProblem::MultipleLanguages`], and a compiled module
//! that the module store says another language wrote is a
//! [`ClassProblem::ModuleFromOtherLanguage`]. The host owns the rule, raised
//! when classes are scanned ([`crate::ClassRegistry::problems`]) and by
//! [`class_problems`]; a language's plugin enumerates its classes with
//! [`is_owned_by`] and so is handed only classes it owns. No language names
//! another.

use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use pulsar_language::{Language, LanguageRegistry};

use crate::modules::ModuleStore;
use crate::registry::{class_name_of_dir, find_class_dirs, project_root_of_class_dir};

/// Why a class dir does not have one language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassProblem {
    /// More than one language's marker file is in the directory.
    MultipleLanguages {
        class: String,
        dir: PathBuf,
        /// The languages (ids) whose markers are present.
        languages: Vec<String>,
        /// Their display names, in the same order.
        language_names: Vec<String>,
        /// Those markers, in the same order.
        markers: Vec<String>,
    },
    /// The module store says another language wrote the class's module.
    ModuleFromOtherLanguage {
        class: String,
        dir: PathBuf,
        /// The one language the registry resolved (id and display name).
        language: String,
        language_name: String,
        /// The id of the language that wrote the module.
        module_language: String,
    },
}

impl ClassProblem {
    /// The class's name.
    pub fn class(&self) -> &str {
        match self {
            Self::MultipleLanguages { class, .. } | Self::ModuleFromOtherLanguage { class, .. } => {
                class
            }
        }
    }

    /// The class directory.
    pub fn dir(&self) -> &Path {
        match self {
            Self::MultipleLanguages { dir, .. } | Self::ModuleFromOtherLanguage { dir, .. } => dir,
        }
    }

    /// The file the problem is about, for the Problems drawer.
    pub fn file(&self) -> PathBuf {
        match self {
            Self::MultipleLanguages { dir, markers, .. } => {
                markers.first().map_or_else(|| dir.clone(), |marker| dir.join(marker))
            }
            Self::ModuleFromOtherLanguage { dir, .. } => dir.clone(),
        }
    }
}

impl std::fmt::Display for ClassProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MultipleLanguages {
                class, markers, language_names, ..
            } => {
                let found: Vec<String> = markers
                    .iter()
                    .zip(language_names)
                    .map(|(marker, language)| format!("`{marker}` ({language})"))
                    .collect();
                write!(
                    f,
                    "class `{class}` has the files of more than one language, {}: a class has one \
                     language, so remove all but one of them",
                    found.join(" and ")
                )
            }
            Self::ModuleFromOtherLanguage {
                class,
                language_name,
                module_language,
                ..
            } => write!(
                f,
                "class `{class}`: its compiled module was written by the `{module_language}` \
                 language; delete its module from the project's {} to compile it as {language_name}",
                crate::modules::STORE_DIR
            ),
        }
    }
}

/// The one language `dir` is a class of, or why it is not one: `Ok(None)` for
/// a directory that is no language's class (a prefab-only class, a plain
/// folder).
pub fn language_of(dir: &Path) -> Result<Option<&'static Language>, ClassProblem> {
    language_in(&LanguageRegistry::current(), dir)
}

fn language_in(
    registry: &LanguageRegistry,
    dir: &Path,
) -> Result<Option<&'static Language>, ClassProblem> {
    let languages = registry.class_languages(dir);
    let class = || class_name_of_dir(dir);
    let language = match languages.as_slice() {
        [] => return Ok(None),
        [one] => *one,
        many => {
            let present = |language: &Language| {
                language
                    .class_markers
                    .iter()
                    .find(|marker| virtual_fs::is_file(&dir.join(marker)))
                    .copied()
                    .unwrap_or_default()
                    .to_owned()
            };
            return Err(ClassProblem::MultipleLanguages {
                class: class(),
                dir: dir.to_path_buf(),
                languages: many.iter().map(|l| l.id.to_owned()).collect(),
                language_names: many.iter().map(|l| l.display_name.to_owned()).collect(),
                markers: many.iter().map(|l| present(l)).collect(),
            });
        }
    };
    if let Some(module_language) = module_language(dir) {
        if module_language != language.id {
            return Err(ClassProblem::ModuleFromOtherLanguage {
                class: class(),
                dir: dir.to_path_buf(),
                language: language.id.to_owned(),
                language_name: language.display_name.to_owned(),
                module_language,
            });
        }
    }
    Ok(Some(language))
}

/// The language the module store says wrote `dir`'s module (nothing is
/// written: a class without an id has no module).
fn module_language(dir: &Path) -> Option<String> {
    let id = crate::id::read_class_id(dir)?;
    let root = project_root_of_class_dir(dir)?;
    ModuleStore::of_project(&root).language(&id)
}

/// Whether `dir` is a class of exactly the language `language_id`, with no
/// problem: what a language's plugin asks to enumerate only its own classes.
pub fn is_owned_by(dir: &Path, language_id: &str) -> bool {
    matches!(language_of(dir), Ok(Some(language)) if language.id == language_id)
}

/// `Ok` when the language `language_id` may compile `dir`; `Err` carries the
/// host's refusal ([`ClassProblem`]'s text) for a class that is another
/// language's or both, so a refusal never overwrites the other language's
/// artifacts.
pub fn check_owned(dir: &Path, language_id: &str) -> Result<(), String> {
    match language_of(dir) {
        Ok(Some(language)) if language.id == language_id => Ok(()),
        Ok(Some(language)) => Err(format!(
            "class `{}` is a {} class, not this language's",
            class_name_of_dir(dir),
            language.display_name
        )),
        Ok(None) => Err(format!(
            "class `{}` has no source file of this language",
            class_name_of_dir(dir)
        )),
        Err(problem) => Err(problem.to_string()),
    }
}

/// The problems of the classes under `project_root` (one per class), in
/// class order. Nothing is written.
pub fn class_problems(project_root: &Path) -> Vec<ClassProblem> {
    problems_of_dirs(&find_class_dirs(project_root))
}

pub(crate) fn problems_of_dirs(dirs: &[PathBuf]) -> Vec<ClassProblem> {
    let registry = LanguageRegistry::current();
    dirs.iter()
        .filter_map(|dir| language_in(&registry, dir).err())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(root: &Path, name: &str, files: &[&str]) -> PathBuf {
        let dir = root.join("src").join("classes").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), "{}").unwrap();
        }
        dir
    }

    #[test]
    fn a_class_with_two_markers_is_one_multiple_languages_problem() {
        let root = tempfile::tempdir().unwrap();
        let both = class(root.path(), "Both", &["graph_save.json", "class.ts"]);
        let one = class(root.path(), "One", &["graph_save.json"]);

        let problems = class_problems(root.path());
        assert_eq!(problems.len(), 1, "{problems:?}");
        match &problems[0] {
            ClassProblem::MultipleLanguages { class, dir, languages, .. } => {
                assert_eq!(class, "Both");
                assert_eq!(dir, &both);
                assert_eq!(languages, &["test.blueprint", "test.text"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(problems[0].to_string().contains("more than one language"));

        // The same problem is raised at scan, and neither language owns it.
        let registry = crate::ClassRegistry::scan(root.path());
        assert_eq!(registry.problems().len(), 1);
        assert!(!is_owned_by(&both, "test.blueprint"));
        assert!(!is_owned_by(&both, "test.text"));
        assert!(check_owned(&both, "test.text").is_err());
        assert!(is_owned_by(&one, "test.blueprint"));
        assert!(check_owned(&one, "test.blueprint").is_ok());
        assert!(check_owned(&one, "test.text").is_err());
    }

    #[test]
    fn a_module_another_language_wrote_is_a_problem_of_the_class() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Pulsar")).unwrap();
        let dir = class(root.path(), "Door", &["class.ts"]);
        let registry = crate::ClassRegistry::scan(root.path());
        assert!(registry.problems().is_empty());
        let entry = registry.by_name("Door").unwrap();
        ModuleStore::of_project(root.path())
            .write(entry, "test.blueprint", String::new(), b"{}")
            .unwrap();
        let problems = class_problems(root.path());
        assert!(
            matches!(&problems[..], [ClassProblem::ModuleFromOtherLanguage { module_language, .. }] if module_language == "test.blueprint"),
            "{problems:?}"
        );
        assert!(!is_owned_by(&dir, "test.text"));
    }
}
