//! Persistent identity for a class's fields.
//!
//! State migration (hot reload, saved state, level overrides) matches
//! variables by stable id, not by name (see `pulsar_script_vm::migrate`). A
//! TypeScript field has no id of its own, so each class keeps a *schema*
//! next to its source (`class.schema.json`) that the compiler maintains:
//!
//! - a field keeps its id for as long as its name does;
//! - renaming a field keeps its id when the new declaration says where it
//!   came from: `@renamedFrom("oldName") newName = 0;`. Without it a rename
//!   is a removed field and a new one, and the old value is not carried;
//! - the class version rises whenever the set of fields, or any field's name
//!   or type, changes, so `migrate(from: int)` runs after such a change.
//!
//! The schema is written back by the plugin when a class compiles; commit it
//! with the source so a clean checkout compiles to the same ids.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassSchema {
    /// The class's schema version; `Module::class_version`.
    pub version: u32,
    pub fields: Vec<SchemaField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaField {
    pub id: String,
    pub name: String,
    /// The script type, as `Type`'s `Display` writes it.
    pub ty: String,
}

/// A field as the source declares it now.
#[derive(Clone, Debug)]
pub struct DeclaredField {
    pub name: String,
    pub ty: String,
    /// From `@renamedFrom("..")`.
    pub renamed_from: Option<String>,
}

impl ClassSchema {
    /// The schema after `declared`, given the `previous` one. `Err` carries
    /// a message per problem (an unknown or doubly claimed `renamedFrom`).
    pub fn reconcile(previous: Option<&ClassSchema>, declared: &[DeclaredField]) -> Result<ClassSchema, Vec<String>> {
        let mut errors = Vec::new();
        let mut claimed: Vec<&str> = Vec::new();
        let mut fields = Vec::with_capacity(declared.len());
        for field in declared {
            let inherited = match (&field.renamed_from, previous) {
                (Some(old), Some(previous)) => match previous.fields.iter().find(|f| f.name == *old) {
                    Some(found) => Some(found),
                    None => {
                        errors.push(format!("`{}` says it was renamed from `{old}`, which the class never had", field.name));
                        None
                    }
                },
                (Some(old), None) => {
                    errors.push(format!("`{}` says it was renamed from `{old}`, but the class has no earlier schema", field.name));
                    None
                }
                (None, previous) => previous.and_then(|p| p.fields.iter().find(|f| f.name == field.name)),
            };
            let id = match inherited {
                Some(found) => {
                    if claimed.contains(&found.id.as_str()) {
                        errors.push(format!("two fields claim the identity of `{}`", found.name));
                    }
                    claimed.push(found.id.as_str());
                    found.id.clone()
                }
                None => uuid::Uuid::new_v4().to_string(),
            };
            fields.push(SchemaField { id, name: field.name.clone(), ty: field.ty.clone() });
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let version = match previous {
            None => 1,
            Some(previous) if same_fields(&previous.fields, &fields) => previous.version,
            Some(previous) => previous.version + 1,
        };
        Ok(ClassSchema { version, fields })
    }

    pub fn id_of(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|f| f.name == name).map(|f| f.id.as_str())
    }
}

fn same_fields(a: &[SchemaField], b: &[SchemaField]) -> bool {
    let key = |f: &SchemaField| (f.id.clone(), f.name.clone(), f.ty.clone());
    let (mut a, mut b): (Vec<_>, Vec<_>) = (a.iter().map(key).collect(), b.iter().map(key).collect());
    a.sort();
    b.sort();
    a == b
}
