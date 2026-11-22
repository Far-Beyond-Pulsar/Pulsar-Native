//! Filtered, sorted reads of the indexed file rows.

use anyhow::{Context, Result};
use rusqlite::{params_from_iter, types::Value};

use super::*;

impl ProjectFileIndex {
    #[track_caller]
    pub fn query(&self, query: &FileIndexQuery) -> Result<Vec<IndexedFile>> {
        editor_task_queue::blocking_call("file_index::query");
        let mut sql = String::from(
            "SELECT path, name, extension, file_type_id, is_dir, size, created, modified FROM files WHERE 1=1",
        );
        let mut values = Vec::<Value>::new();
        if !query.include_directories {
            sql.push_str(" AND is_dir = 0");
        }
        if let Some(file_type) = query.file_type_id.as_deref() {
            sql.push_str(" AND file_type_id = ?");
            values.push(Value::Text(file_type.to_owned()));
        }
        if let Some(extension) = query.extension.as_deref() {
            sql.push_str(" AND extension = ?");
            values.push(Value::Text(
                extension.trim_start_matches('.').to_ascii_lowercase(),
            ));
        }
        if let Some(after) = query.modified_after {
            sql.push_str(" AND modified >= ?");
            values.push(Value::Integer(after));
        }
        if let Some(before) = query.modified_before {
            sql.push_str(" AND modified <= ?");
            values.push(Value::Integer(before));
        }
        if let Some(name) = query
            .name_contains
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            sql.push_str(" AND instr(lower(name), lower(?)) > 0");
            values.push(Value::Text(name.to_owned()));
        }
        if let Some(name) = query.name_equals.as_deref() {
            sql.push_str(" AND name = ? COLLATE NOCASE");
            values.push(Value::Text(name.to_owned()));
        }
        if let Some(folder) = query.folder.as_deref() {
            let folder = normalize_relative_path(folder)
                .trim_end_matches('/')
                .to_owned();
            if folder.is_empty() {
                // All entries are already relative to the project root.
            } else {
                sql.push_str(" AND (path = ? OR substr(path, 1, length(?) + 1) = ? || '/')");
                values.extend([
                    Value::Text(folder.clone()),
                    Value::Text(folder.clone()),
                    Value::Text(folder),
                ]);
            }
        }
        if let Some(folder) = query.direct_children_of.as_deref() {
            let folder = normalize_relative_path(folder)
                .trim_end_matches('/')
                .to_owned();
            if folder.is_empty() {
                sql.push_str(" AND instr(path, '/') = 0");
            } else {
                sql.push_str(
                    " AND substr(path, 1, length(?) + 1) = ? || '/'\
                      AND instr(substr(path, length(?) + 2), '/') = 0",
                );
                values.extend([
                    Value::Text(folder.clone()),
                    Value::Text(folder.clone()),
                    Value::Text(folder),
                ]);
            }
        }
        if let Some(min_size) = query.min_size {
            sql.push_str(" AND size >= ?");
            values.push(Value::Integer(i64::try_from(min_size).unwrap_or(i64::MAX)));
        }
        if let Some(max_size) = query.max_size {
            sql.push_str(" AND size <= ?");
            values.push(Value::Integer(i64::try_from(max_size).unwrap_or(i64::MAX)));
        }
        let order = match query.sort {
            FileIndexSort::Path => "path",
            FileIndexSort::Name => "name COLLATE NOCASE",
            FileIndexSort::Extension => "extension",
            FileIndexSort::FileType => "file_type_id",
            FileIndexSort::Size => "size",
            FileIndexSort::Created => "created",
            FileIndexSort::Modified => "modified",
        };
        sql.push_str(" ORDER BY ");
        sql.push_str(order);
        sql.push_str(if query.descending { " DESC" } else { " ASC" });
        sql.push_str(", path ASC");
        if let Some(limit) = query.limit {
            sql.push_str(" LIMIT ?");
            values.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        }

        let connection = self.connection.lock();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(IndexedFile {
                path: row.get(0)?,
                name: row.get(1)?,
                extension: row.get(2)?,
                file_type_id: row.get(3)?,
                is_dir: row.get(4)?,
                size: u64::try_from(row.get::<_, i64>(5)?).unwrap_or_default(),
                created: row.get(6)?,
                modified: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query project file index")
    }
}
