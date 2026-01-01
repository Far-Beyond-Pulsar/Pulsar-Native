use anyhow::{Result, anyhow};
use rusqlite::{Connection, params, Row, ToSql};
use parking_lot::RwLock;
use std::sync::Arc;
use std::collections::HashMap;
use serde_json::Value;
use crate::reflection::{TypeSchema, SqlType};

#[derive(Debug, Clone)]
pub struct CellValue {
    pub value: Value,
    pub display: String,
}

impl CellValue {
    pub fn new(value: Value) -> Self {
        let display = match &value {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => value.to_string(),
        };

        Self { value, display }
    }

    pub fn from_row(row: &Row, idx: usize) -> Result<Self> {
        let value = row.get_ref(idx)?;

        let json_value = match value {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(i) => Value::Number(i.into()),
            rusqlite::types::ValueRef::Real(f) => {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
            rusqlite::types::ValueRef::Text(t) => {
                Value::String(String::from_utf8_lossy(t).to_string())
            }
            rusqlite::types::ValueRef::Blob(b) => {
                Value::String(format!("<blob {} bytes>", b.len()))
            }
        };

        Ok(CellValue::new(json_value))
    }
}

#[derive(Debug, Clone)]
pub struct RowData {
    pub id: i64,
    pub cells: Vec<CellValue>,
}

pub struct DatabaseManager {
    connection: Arc<RwLock<Connection>>,
    schemas: Arc<RwLock<HashMap<String, TypeSchema>>>,
}

impl DatabaseManager {
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let connection = Connection::open(path)?;

        Ok(Self {
            connection: Arc::new(RwLock::new(connection)),
            schemas: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;

        Ok(Self {
            connection: Arc::new(RwLock::new(connection)),
            schemas: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn register_type(&self, schema: TypeSchema) -> Result<()> {
        let create_sql = schema.to_create_table_sql();

        {
            let conn = self.connection.write();
            conn.execute(&create_sql, [])?;
        }

        {
            let mut schemas = self.schemas.write();
            schemas.insert(schema.table_name.clone(), schema);
        }

        Ok(())
    }

    pub fn get_schema(&self, table_name: &str) -> Option<TypeSchema> {
        let schemas = self.schemas.read();
        
        // First try exact match
        if let Some(schema) = schemas.get(table_name) {
            return Some(schema.clone());
        }
        
        // If no exact match, try to find a schema whose table_name matches
        // This handles cases where someone passes the type name instead of table name
        for schema in schemas.values() {
            if schema.table_name == table_name {
                return Some(schema.clone());
            }
        }
        
        tracing::error!("No schema found for table '{}'. Available schemas: {:?}", 
                  table_name, schemas.keys().collect::<Vec<_>>());
        None
    }

    pub fn list_tables(&self) -> Result<Vec<String>> {
        let conn = self.connection.read();
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )?;

        let tables = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;

        Ok(tables)
    }
    
    pub fn introspect_and_register_schemas(&self) -> Result<()> {
        let tables = self.list_tables()?;
        
        for table_name in tables {
            // Skip if schema already registered
            if self.get_schema(&table_name).is_some() {
                continue;
            }
            
            // Get table structure from SQLite
            let conn = self.connection.read();
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table_name))?;
            
            let columns: Vec<(String, String, bool)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?, // column name
                        row.get::<_, String>(2)?, // column type
                        row.get::<_, i32>(3)? == 0, // nullable (notnull == 0 means nullable)
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            
            drop(stmt);
            drop(conn);
            
            // Create schema from introspected columns
            let mut schema = TypeSchema::new(table_name.clone());
            schema.table_name = table_name.clone(); // Override to use exact table name
            
            for (col_name, col_type, nullable) in columns {
                // Skip 'id' column as it's automatically added
                if col_name == "id" {
                    continue;
                }
                
                // Map SQLite types to our SqlType
                let sql_type = match col_type.to_uppercase().as_str() {
                    t if t.contains("INT") => crate::reflection::SqlType::Integer,
                    t if t.contains("REAL") || t.contains("FLOAT") || t.contains("DOUBLE") => crate::reflection::SqlType::Real,
                    t if t.contains("TEXT") || t.contains("CHAR") || t.contains("CLOB") => crate::reflection::SqlType::Text,
                    t if t.contains("BLOB") => crate::reflection::SqlType::Blob,
                    t if t.contains("BOOL") => crate::reflection::SqlType::Boolean,
                    _ => crate::reflection::SqlType::Text, // Default to Text for unknown types
                };
                
                schema.add_field(col_name, sql_type, nullable);
            }
            
            // Register the schema
            let mut schemas = self.schemas.write();
            schemas.insert(table_name.clone(), schema);
        }
        
        Ok(())
    }

    pub fn get_row_count(&self, table_name: &str) -> Result<usize> {
        let conn = self.connection.read();
        let count: usize = conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", table_name),
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn fetch_rows(&self, table_name: &str, offset: usize, limit: usize) -> Result<Vec<RowData>> {
        let schema = self
            .get_schema(table_name)
            .ok_or_else(|| anyhow!("Schema not found for table: {}", table_name))?;

        let conn = self.connection.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, {} FROM {} ORDER BY id LIMIT ? OFFSET ?",
            schema.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "),
            table_name
        ))?;

        let rows = stmt.query_map(params![limit, offset], |row| {
            let id: i64 = row.get(0)?;
            let mut cells = Vec::new();

            for i in 1..=schema.fields.len() {
                cells.push(CellValue::from_row(row, i).unwrap());
            }

            Ok(RowData { id, cells })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }

        Ok(result)
    }

    pub fn insert_row(&self, table_name: &str, values: Vec<Value>) -> Result<i64> {
        let schema = self
            .get_schema(table_name)
            .ok_or_else(|| anyhow!("Schema not found for table: {}", table_name))?;

        if values.len() != schema.fields.len() {
            return Err(anyhow!(
                "Value count mismatch: expected {}, got {}",
                schema.fields.len(),
                values.len()
            ));
        }

        let placeholders = vec!["?"; values.len()].join(", ");
        let field_names = schema
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name, field_names, placeholders
        );

        let conn = self.connection.write();
        let params: Vec<Box<dyn ToSql>> = values
            .iter()
            .map(|v| {
                let param: Box<dyn ToSql> = match v {
                    Value::Null => Box::new(None::<String>),
                    Value::Bool(b) => Box::new(*b as i32),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            Box::new(i)
                        } else if let Some(f) = n.as_f64() {
                            Box::new(f)
                        } else {
                            Box::new(None::<String>)
                        }
                    }
                    Value::String(s) => Box::new(s.clone()),
                    _ => Box::new(v.to_string()),
                };
                param
            })
            .collect();

        conn.execute(&sql, rusqlite::params_from_iter(params.iter()))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_cell(
        &self,
        table_name: &str,
        row_id: i64,
        field_name: &str,
        value: Value,
    ) -> Result<()> {
        let sql = format!(
            "UPDATE {} SET {} = ? WHERE id = ?",
            table_name, field_name
        );

        let param: Box<dyn ToSql> = match value {
            Value::Null => Box::new(None::<String>),
            Value::Bool(b) => Box::new(b as i32),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Box::new(i)
                } else if let Some(f) = n.as_f64() {
                    Box::new(f)
                } else {
                    Box::new(None::<String>)
                }
            }
            Value::String(s) => Box::new(s),
            _ => Box::new(value.to_string()),
        };

        let conn = self.connection.write();
        conn.execute(&sql, params![&param, row_id])?;
        Ok(())
    }

    pub fn delete_row(&self, table_name: &str, row_id: i64) -> Result<()> {
        let sql = format!("DELETE FROM {} WHERE id = ?", table_name);
        let conn = self.connection.write();
        conn.execute(&sql, params![row_id])?;
        Ok(())
    }

    pub fn execute_query(&self, sql: &str) -> Result<Vec<Vec<CellValue>>> {
        let conn = self.connection.read();
        let mut stmt = conn.prepare(sql)?;
        let column_count = stmt.column_count();

        let rows = stmt.query_map([], |row| {
            let mut cells = Vec::new();
            for i in 0..column_count {
                cells.push(CellValue::from_row(row, i).unwrap());
            }
            Ok(cells)
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }

        Ok(result)
    }

    pub fn get_foreign_key_options(&self, table_name: &str) -> Result<Vec<(i64, String)>> {
        let conn = self.connection.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, * FROM {} ORDER BY id",
            table_name
        ))?;

        let column_count = stmt.column_count();
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let mut display_parts = Vec::new();

            for i in 1..column_count {
                if let Ok(value) = CellValue::from_row(row, i) {
                    if value.display != "NULL" {
                        display_parts.push(value.display);
                        if display_parts.len() >= 3 {
                            break;
                        }
                    }
                }
            }

            let display = if display_parts.is_empty() {
                format!("ID: {}", id)
            } else {
                display_parts.join(" - ")
            };

            Ok((id, display))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }

        Ok(result)
    }
}

impl Clone for DatabaseManager {
    fn clone(&self) -> Self {
        Self {
            connection: self.connection.clone(),
            schemas: self.schemas.clone(),
        }
    }
}
