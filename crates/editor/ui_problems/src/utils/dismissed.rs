//! The findings a project dismissed: `<project>/.pulsar/dismissed_findings.txt`,
//! one item id (`<source>|<key>`) per line. A dismissal is the project's
//! decision that a finding is an accepted trade-off, so it is kept with the
//! project. An id no source publishes any more is dropped when the file is
//! next written.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The file of `project`.
pub fn file_of(project: &Path) -> PathBuf {
    project.join(".pulsar").join("dismissed_findings.txt")
}

/// The ids `project` dismissed (none when the file is missing or unreadable).
pub fn load(project: &Path) -> BTreeSet<String> {
    match engine_fs::virtual_fs::read_to_string(&file_of(project)) {
        Ok(text) => parse(&text),
        Err(_) => BTreeSet::new(),
    }
}

pub fn parse(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

pub fn format(ids: &BTreeSet<String>) -> String {
    let mut text = String::from(
        "# Findings this project dismissed in the Problems panel, one per line.\n",
    );
    for id in ids {
        text.push_str(id);
        text.push('\n');
    }
    text
}

/// Write `ids` for `project` (an empty set removes the file).
pub fn save(project: &Path, ids: &BTreeSet<String>) -> Result<(), String> {
    let file = file_of(project);
    if ids.is_empty() {
        if engine_fs::virtual_fs::exists(&file).unwrap_or(false) {
            engine_fs::virtual_fs::delete_path(&file).map_err(|error| format!("{error:#}"))?;
        }
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        engine_fs::virtual_fs::create_dir_all(dir).map_err(|error| format!("{error:#}"))?;
    }
    engine_fs::virtual_fs::write_file_atomically(&file, format(ids).as_bytes())
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_comments_are_skipped() {
        let ids: BTreeSet<String> = ["Budgets#check|pool".into(), "Plugins|pulsar.x".into()].into();
        assert_eq!(parse(&format(&ids)), ids);
        assert!(parse("# only a comment\n\n").is_empty());
    }
}
