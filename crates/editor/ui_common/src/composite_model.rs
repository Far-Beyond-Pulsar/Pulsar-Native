//! Tree model behind the composite (array / struct / map) property editor.
//!
//! A composite value is shown as a tree of rows.  The tree is derived from the
//! value's JSON plus its [`RuntimeTypeInfo`], so this module needs no knowledge
//! of Rust types or widgets: [`flatten`] walks only the branches the user has
//! expanded and returns one [`TreeRow`] per visible node, which the editor then
//! feeds to a virtual list.  Nothing here is proportional to the number of
//! nodes that are *not* shown.

use pulsar_reflection::{RuntimeTypeInfo, TypeStructure, WrapperType};
use serde_json::Value;
use smallvec::SmallVec;
use std::any::TypeId;
use std::collections::HashSet;
use std::sync::Arc;

/// One step from a node to its child.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Seg {
    Index(u32),
    Key(Arc<str>),
}

/// Location of a node under the composite's root (empty = the root itself).
pub type Path = SmallVec<[Seg; 3]>;

/// What a type looks like to the tree.
#[derive(Clone, Copy)]
pub enum Shape {
    /// Edited by its own registered editor, or shown read-only.
    Leaf,
    /// JSON array of `inner` (`Vec`, `[T; N]`, `HashSet`).
    Seq(&'static RuntimeTypeInfo),
    /// JSON object of `inner` values (`HashMap`).
    Map(&'static RuntimeTypeInfo),
    /// JSON object with named, individually typed fields.
    Record(&'static [pulsar_reflection::FieldInfo]),
}

/// Unwrap transparent wrappers (`Box`, `Arc`, `Rc`, `Option`, `Result`) until
/// the type either has its own editor or is a real container.
pub fn resolve(
    mut ty: &'static RuntimeTypeInfo,
    has_editor: &dyn Fn(TypeId) -> bool,
) -> (&'static RuntimeTypeInfo, Shape) {
    loop {
        if has_editor(ty.type_id) {
            return (ty, Shape::Leaf);
        }
        match &ty.structure {
            TypeStructure::Wrapper { wrapper_kind, inner } => match wrapper_kind {
                WrapperType::Array | WrapperType::Vec | WrapperType::HashSet => {
                    return (ty, Shape::Seq(inner));
                }
                WrapperType::HashMap => return (ty, Shape::Map(inner)),
                WrapperType::Box
                | WrapperType::Arc
                | WrapperType::Rc
                | WrapperType::Option
                | WrapperType::Result => ty = inner,
                WrapperType::Custom(_) => return (ty, Shape::Leaf),
            },
            TypeStructure::Struct { fields } => return (ty, Shape::Record(fields)),
            _ => return (ty, Shape::Leaf),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// A value with its own editor.
    Leaf,
    /// A value nothing can edit (no editor, no structure, or `null`): shown as text.
    ReadOnly,
    /// A container; `len` children, expanded or not.
    Branch { len: usize, expanded: bool },
}

#[derive(Clone)]
pub struct TreeRow {
    pub path: Path,
    pub depth: u16,
    /// The resolved type of the node (wrappers already unwrapped).
    pub ty: &'static RuntimeTypeInfo,
    pub kind: RowKind,
    /// The node sits directly in a `Vec` / array / set: it can be removed.
    pub removable: bool,
}

impl TreeRow {
    pub fn label(&self) -> String {
        match self.path.last() {
            Some(Seg::Index(i)) => format!("[{i}]"),
            Some(Seg::Key(k)) => k.to_string(),
            None => String::new(),
        }
    }
}

/// The value at `path`, if the path exists.
pub fn get<'a>(root: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    path.iter().try_fold(root, |value, seg| match (seg, value) {
        (Seg::Index(i), Value::Array(items)) => items.get(*i as usize),
        (Seg::Key(k), Value::Object(map)) => map.get(&**k),
        _ => None,
    })
}

pub fn get_mut<'a>(root: &'a mut Value, path: &[Seg]) -> Option<&'a mut Value> {
    path.iter().try_fold(root, |value, seg| match (seg, value) {
        (Seg::Index(i), Value::Array(items)) => items.get_mut(*i as usize),
        (Seg::Key(k), Value::Object(map)) => map.get_mut(&**k),
        _ => None,
    })
}

/// The child type reached from a node of shape `shape` by `seg`.
fn child_type(shape: Shape, seg: &Seg) -> Option<&'static RuntimeTypeInfo> {
    match (shape, seg) {
        (Shape::Seq(inner), Seg::Index(_)) | (Shape::Map(inner), Seg::Key(_)) => Some(inner),
        (Shape::Record(fields), Seg::Key(k)) => {
            fields.iter().find(|f| f.name == &**k).map(|f| f.type_info)
        }
        _ => None,
    }
}

/// Rows for everything visible under the root: the root's children, and the
/// children of every expanded branch, depth first.  Cost is proportional to the
/// rows returned, never to the size of collapsed subtrees.
pub fn flatten(
    root: &Value,
    root_ty: &'static RuntimeTypeInfo,
    expanded: &HashSet<Path>,
    has_editor: &dyn Fn(TypeId) -> bool,
) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let (_, shape) = resolve(root_ty, has_editor);
    push_children(root, shape, &Path::new(), 0, expanded, has_editor, &mut rows);
    rows
}

fn push_children(
    value: &Value,
    shape: Shape,
    path: &Path,
    depth: u16,
    expanded: &HashSet<Path>,
    has_editor: &dyn Fn(TypeId) -> bool,
    rows: &mut Vec<TreeRow>,
) {
    let in_seq = matches!(shape, Shape::Seq(_));
    let mut visit = |seg: Seg, child: &Value, rows: &mut Vec<TreeRow>| {
        let Some(ty) = child_type(shape, &seg) else {
            return;
        };
        let (ty, child_shape) = resolve(ty, has_editor);
        let mut child_path = path.clone();
        child_path.push(seg);
        let kind = match child_shape {
            Shape::Leaf if child.is_null() => RowKind::ReadOnly,
            Shape::Leaf if has_editor(ty.type_id) => RowKind::Leaf,
            Shape::Leaf => RowKind::ReadOnly,
            Shape::Seq(_) | Shape::Map(_) | Shape::Record(_) => {
                let len = match child {
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    _ => 0,
                };
                RowKind::Branch {
                    len,
                    expanded: expanded.contains(&child_path),
                }
            }
        };
        let descend = matches!(kind, RowKind::Branch { expanded: true, .. });
        rows.push(TreeRow {
            path: child_path.clone(),
            depth,
            ty,
            kind,
            removable: in_seq,
        });
        if descend {
            push_children(
                child,
                child_shape,
                &child_path,
                depth + 1,
                expanded,
                has_editor,
                rows,
            );
        }
    };
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                visit(Seg::Index(i as u32), item, rows);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                visit(Seg::Key(Arc::from(key.as_str())), item, rows);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::{Reflectable, RUNTIME_TYPE_REGISTRY};
    use serde_json::json;

    fn editable(ty: TypeId) -> bool {
        ty == f32::type_info().type_id || ty == i32::type_info().type_id
    }

    #[test]
    fn vec_of_leaves_lists_every_element_unexpanded_nowhere() {
        let ty = <Vec<i32> as Reflectable>::type_info();
        let value = json!([1, 2, 3]);
        let rows = flatten(&value, ty, &HashSet::new(), &editable);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.kind == RowKind::Leaf && r.removable));
        assert_eq!(rows[1].label(), "[1]");
    }

    #[test]
    fn nested_vec_only_descends_into_expanded_branches() {
        let ty = <Vec<Vec<i32>> as Reflectable>::type_info();
        let value = json!([[1, 2], [3], []]);
        let collapsed = flatten(&value, ty, &HashSet::new(), &editable);
        assert_eq!(collapsed.len(), 3);
        assert_eq!(collapsed[0].kind, RowKind::Branch { len: 2, expanded: false });

        let mut open = HashSet::new();
        open.insert(Path::from_vec(vec![Seg::Index(0)]));
        let rows = flatten(&value, ty, &open, &editable);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].path.len(), 2);
    }

    #[test]
    fn get_and_get_mut_follow_paths() {
        let mut value = json!({"a": [10, 20]});
        let path = [Seg::Key(Arc::from("a")), Seg::Index(1)];
        assert_eq!(get(&value, &path), Some(&json!(20)));
        *get_mut(&mut value, &path).unwrap() = json!(21);
        assert_eq!(value, json!({"a": [10, 21]}));
        assert!(get(&value, &[Seg::Index(0)]).is_none());
    }

    #[test]
    fn option_none_is_read_only_and_unregistered_leaf_too() {
        let ty = <Vec<Option<i32>> as Reflectable>::type_info();
        let rows = flatten(&json!([null, 4]), ty, &HashSet::new(), &editable);
        assert_eq!(rows[0].kind, RowKind::ReadOnly);
        assert_eq!(rows[1].kind, RowKind::Leaf);
        let _ = &RUNTIME_TYPE_REGISTRY;
    }

    #[test]
    fn huge_collapsed_tree_costs_only_its_top_level() {
        let ty = <Vec<Vec<i32>> as Reflectable>::type_info();
        let value = Value::Array((0..1000).map(|_| Value::Array(vec![json!(1); 1000])).collect());
        let rows = flatten(&value, ty, &HashSet::new(), &editable);
        assert_eq!(rows.len(), 1000);
    }
}
