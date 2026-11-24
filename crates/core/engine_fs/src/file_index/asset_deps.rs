//! Typed dependency edges between assets, recorded by format extractors.

use anyhow::{Context, Result};
use rusqlite::params;
use std::path::Path;

use super::*;

impl ProjectFileIndex {
    /// Replace every dependency edge recorded for the asset `source_id` of
    /// `source_kind` stored at `path`. Extractors (one per asset kind, owned by
    /// the plugin that owns the format) compute `deps` when the asset is
    /// saved, imported or refreshed; this table is derived data and is never
    /// written into an asset. Each dep's `source_*` fields must match the
    /// arguments.
    #[track_caller]
    pub fn replace_asset_deps(
        &self,
        path: &Path,
        source_kind: &str,
        source_id: &str,
        deps: &[AssetDep],
    ) -> Result<()> {
        editor_task_queue::blocking_call("file_index::replace_asset_deps");
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset dependency path is outside the indexed project root");
        };
        if let Some(bad) = deps
            .iter()
            .find(|d| d.source_kind != source_kind || d.source_id != source_id)
        {
            anyhow::bail!(
                "dependency from {}:{} recorded for {source_kind}:{source_id}",
                bad.source_kind,
                bad.source_id
            );
        }
        let mut connection = self.connection.lock();
        let tx = connection.transaction()?;
        tx.execute(
            "DELETE FROM asset_deps WHERE source_path = ?1 AND source_kind = ?2",
            params![relative, source_kind],
        )?;
        for dep in deps {
            tx.execute(
                "INSERT OR REPLACE INTO asset_deps(source_path, source_kind, source_id, target_kind, target_id, role, via, expect)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    relative,
                    dep.source_kind,
                    dep.source_id,
                    dep.target_kind,
                    dep.target_id,
                    dep.role,
                    dep.via,
                    dep.expect
                ],
            )
            .context("record asset dependency (is the file indexed?)")?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forget the dependency edges of the asset at `path`.
    #[track_caller]
    pub fn clear_asset_deps(&self, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::clear_asset_deps");
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        let connection = self.connection.lock();
        connection.execute("DELETE FROM asset_deps WHERE source_path = ?1", [&relative])?;
        Ok(())
    }

    /// What an asset depends on, ordered for stable output.
    #[track_caller]
    pub fn asset_deps_of(&self, source_kind: &str, source_id: &str) -> Result<Vec<AssetDep>> {
        editor_task_queue::blocking_call("file_index::asset_deps_of");
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT source_kind, source_id, target_kind, target_id, role, via, expect
             FROM asset_deps WHERE source_kind = ?1 AND source_id = ?2
             ORDER BY target_kind, target_id, via",
        )?;
        let rows = statement.query_map(params![source_kind, source_id], |row| {
            Ok(AssetDep {
                source_kind: row.get(0)?,
                source_id: row.get(1)?,
                target_kind: row.get(2)?,
                target_id: row.get(3)?,
                role: row.get(4)?,
                via: row.get(5)?,
                expect: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query asset dependencies")
    }

    /// The reverse query behind "Used By": every recorded edge that points at
    /// the asset `target_id` of `target_kind`, with the project path of the
    /// asset holding the reference. Computed, never stored on the target.
    #[track_caller]
    pub fn asset_used_by(&self, target_kind: &str, target_id: &str) -> Result<Vec<AssetDependent>> {
        editor_task_queue::blocking_call("file_index::asset_used_by");
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT source_path, source_kind, source_id, target_kind, target_id, role, via, expect
             FROM asset_deps WHERE target_kind = ?1 AND target_id = ?2
             ORDER BY source_path, via",
        )?;
        let rows = statement.query_map(params![target_kind, target_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                AssetDep {
                    source_kind: row.get(1)?,
                    source_id: row.get(2)?,
                    target_kind: row.get(3)?,
                    target_id: row.get(4)?,
                    role: row.get(5)?,
                    via: row.get(6)?,
                    expect: row.get(7)?,
                },
            ))
        })?;
        rows.map(|row| {
            row.map(|(relative, dep)| AssetDependent {
                path: self
                    .project_root
                    .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
                dep,
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query asset dependents")
    }
}
