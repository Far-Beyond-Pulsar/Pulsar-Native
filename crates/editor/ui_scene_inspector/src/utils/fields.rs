//! A component's value as typed fields, and those flattened into the rows
//! the field list draws.
//!
//! A value is held as JSON next to its [`RuntimeTypeInfo`] and turned into
//! rows only where it is open, so a million-element array costs a hundred
//! rows until someone looks inside it. Long lists open to ranges of
//! [`CHUNK`], longer ones to ranges of ranges.

use std::collections::HashSet;

use pulsar_reflection::{RuntimeTypeInfo, TypeStructure, WrapperType};
use pulsar_scenedb::Entity;
use serde_json::Value;

use super::format;

/// Most children a list shows before it splits into ranges.
pub const CHUNK: usize = 100;
/// A top-level value with no more children than this starts open.
const OPEN_BY_DEFAULT: usize = 16;
/// Most values a field search reads.
const SEARCH_LIMIT: usize = 100_000;
const ENTITY_KEY: &str = "$entity";
const ERROR_KEY: &str = "$error";

/// An entity reference among a value's fields; the list makes it a link.
pub fn entity_value(entity: Entity) -> Value {
    serde_json::json!({ ENTITY_KEY: entity.bits() })
}

/// A field that could not be read.
pub fn error_value(error: impl Into<String>) -> Value {
    serde_json::json!({ ERROR_KEY: error.into() })
}

/// Where a component's fields came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldSource {
    /// A registered component class's reflected properties.
    Class,
    /// The type's runtime reflection.
    Reflected,
    /// A scene model type the inspector knows.
    SceneModel,
    /// The verbatim payload of a component this build cannot load.
    Unresolved,
}

impl FieldSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Class => "component class properties",
            Self::Reflected => "reflected type",
            Self::SceneModel => "scene model",
            Self::Unresolved => "unresolved: stored payload",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub label: String,
    pub category: Option<String>,
    pub type_label: String,
    pub ty: Option<&'static RuntimeTypeInfo>,
    pub value: Value,
    /// A string that names one of a closed set (an enum without type info).
    pub variant: bool,
}

impl Field {
    pub fn new(label: impl Into<String>, type_label: impl Into<String>, value: Value) -> Self {
        Self {
            label: label.into(),
            category: None,
            type_label: type_label.into(),
            ty: None,
            value,
            variant: false,
        }
    }

    /// A field typed by reflection.
    pub fn typed(label: impl Into<String>, ty: &'static RuntimeTypeInfo, value: Value) -> Self {
        Self {
            ty: Some(ty),
            ..Self::new(label, format::short_type(ty.type_name), value)
        }
    }

    pub fn as_variant(mut self) -> Self {
        self.variant = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct FieldSet {
    pub source: FieldSource,
    pub fields: Vec<Field>,
}

impl FieldSet {
    /// The value at a row's path (`0/items/3`: the field's position, then a
    /// JSON pointer into it). A range row has no value of its own.
    pub fn value_at(&self, path: &str) -> Option<&Value> {
        let (field, pointer) = match path.split_once('/') {
            Some((field, rest)) => (field, format!("/{rest}")),
            None => (path, String::new()),
        };
        self.fields
            .get(field.parse::<usize>().ok()?)?
            .value
            .pointer(&pointer)
    }

    /// The whole set as one JSON object, entity references written as
    /// `12v1`.
    pub fn to_json(&self) -> Value {
        Value::Object(
            self.fields
                .iter()
                .map(|field| (field.label.clone(), plain(&field.value)))
                .collect(),
        )
    }
}

/// `value` without the inspector's markers.
pub fn plain(value: &Value) -> Value {
    if let Some(entity) = as_entity(value) {
        return Value::String(format::entity(entity));
    }
    if let Some(error) = as_error(value) {
        return Value::String(format!("<{error}>"));
    }
    match value {
        Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), plain(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn marker<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value
        .as_object()
        .filter(|map| map.len() == 1)
        .and_then(|map| map.get(key))
}

fn as_entity(value: &Value) -> Option<Entity> {
    marker(value, ENTITY_KEY)?.as_u64().map(Entity::from_bits)
}

fn as_error(value: &Value) -> Option<&str> {
    marker(value, ERROR_KEY)?.as_str()
}

/// How a row's value is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Display {
    /// A heading over the properties of one category.
    Category,
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    Variant(String),
    /// Linear or display RGBA as stored, and how to write it.
    Color([f32; 4], String),
    /// A short run of numbers, shown in one line.
    Vector(Vec<String>),
    Entity(Entity),
    /// A container's size and first values.
    Summary(String),
    Error(String),
}

impl Display {
    /// The value as text, for search and for copying a single row.
    pub fn text(&self) -> String {
        match self {
            Display::Category => String::new(),
            Display::Null => "null".to_string(),
            Display::Bool(value) => value.to_string(),
            Display::Number(text)
            | Display::Text(text)
            | Display::Variant(text)
            | Display::Summary(text)
            | Display::Error(text) => text.clone(),
            Display::Color(_, text) => text.clone(),
            Display::Vector(items) => items.join(", "),
            Display::Entity(entity) => format::entity(*entity),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FieldRow {
    /// Stable across refreshes: what is open and what changed are keyed by
    /// it. Ranges have a `#start` segment.
    pub path: String,
    pub depth: u16,
    pub label: String,
    pub type_label: String,
    pub display: Display,
    /// `Some(open)` for a row with children.
    pub open: Option<bool>,
}

/// Where a node's own open state differs from its default.
#[derive(Debug, Default)]
pub struct Toggled(HashSet<String>);

impl Toggled {
    pub fn toggle(&mut self, path: &str) {
        if !self.0.remove(path) {
            self.0.insert(path.to_string());
        }
    }

    fn is_open(&self, path: &str, open_by_default: bool) -> bool {
        open_by_default != self.0.contains(path)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// The rows of `set`, or with a `filter` (any case) the rows whose name or
/// value contains it and the way to them. The second value is `true` when a
/// search stopped before reading everything.
pub fn rows(set: &FieldSet, toggled: &Toggled, filter: &str) -> (Vec<FieldRow>, bool) {
    let mut walk = Walk {
        rows: Vec::new(),
        toggled,
        filter: filter.trim().to_lowercase(),
        budget: SEARCH_LIMIT,
    };
    // Categories group the fields that name one, after those that do not.
    let mut order: Vec<usize> = (0..set.fields.len()).collect();
    let mut categories: Vec<&str> = Vec::new();
    for field in &set.fields {
        if let Some(category) = field.category.as_deref() {
            if !categories.contains(&category) {
                categories.push(category);
            }
        }
    }
    let rank = |field: &Field| {
        field.category.as_deref().map_or(0, |category| {
            1 + categories.iter().position(|&c| c == category).unwrap_or(0)
        })
    };
    order.sort_by_key(|&ix| rank(&set.fields[ix]));

    let mut current: Option<&str> = None;
    for ix in order {
        let field = &set.fields[ix];
        let category = field.category.as_deref();
        let heading_at = walk.rows.len();
        let node = Node {
            path: ix.to_string(),
            label: field.label.clone(),
            type_label: field.type_label.clone(),
            ty: field.ty,
            value: &field.value,
            variant: field.variant,
        };
        let added = walk.node(node, 0);
        if added && category != current {
            if let Some(category) = category {
                walk.rows.insert(
                    heading_at,
                    FieldRow {
                        path: format!("category:{category}"),
                        depth: 0,
                        label: category.to_string(),
                        type_label: String::new(),
                        display: Display::Category,
                        open: None,
                    },
                );
            }
            current = category;
        }
    }
    let truncated = walk.budget == 0;
    (walk.rows, truncated)
}

struct Node<'a> {
    path: String,
    label: String,
    type_label: String,
    ty: Option<&'static RuntimeTypeInfo>,
    value: &'a Value,
    variant: bool,
}

struct Walk<'a> {
    rows: Vec<FieldRow>,
    toggled: &'a Toggled,
    filter: String,
    budget: usize,
}

impl Walk<'_> {
    /// Add `node`'s rows. `false` when a filter leaves nothing of it.
    fn node(&mut self, node: Node<'_>, depth: u16) -> bool {
        let searching = !self.filter.is_empty();
        if searching {
            if self.budget == 0 {
                return false;
            }
            self.budget -= 1;
        }
        let ty = node.ty.map(unwrapped);
        let display = display(&node, ty);
        let children = match display {
            Display::Summary(_) => match node.value {
                Value::Array(items) => items.len(),
                Value::Object(map) => map.len(),
                _ => 0,
            },
            _ => 0,
        };
        let own_match = searching
            && (node.label.to_lowercase().contains(&self.filter)
                || (children == 0 && display.text().to_lowercase().contains(&self.filter)));

        let at = self.rows.len();
        let open = (children > 0).then(|| {
            if searching {
                // Opened by the search when something inside matches.
                true
            } else {
                self.toggled
                    .is_open(&node.path, depth == 0 && children <= OPEN_BY_DEFAULT)
            }
        });
        self.rows.push(FieldRow {
            path: node.path.clone(),
            depth,
            label: node.label,
            type_label: node.type_label,
            display,
            open,
        });
        if open != Some(true) {
            if searching && !own_match {
                self.rows.truncate(at);
                return false;
            }
            return true;
        }

        let mut any = false;
        match node.value {
            Value::Array(items) => {
                if searching {
                    for index in 0..items.len() {
                        if self.budget == 0 {
                            break;
                        }
                        any |= self.item(&node.path, ty, items, index, depth + 1);
                    }
                } else {
                    self.range(&node.path, ty, items, 0, items.len(), depth + 1);
                }
            }
            Value::Object(map) => {
                for (key, value) in map {
                    let (child_ty, type_label) = member(ty, key, value);
                    any |= self.node(
                        Node {
                            path: format!("{}/{}", node.path, escape(key)),
                            label: key.clone(),
                            type_label,
                            ty: child_ty,
                            value,
                            variant: false,
                        },
                        depth + 1,
                    );
                }
            }
            _ => {}
        }
        if searching && !any {
            if own_match {
                // The container matched by name: show it closed.
                self.rows.truncate(at + 1);
                self.rows[at].open = Some(false);
                return true;
            }
            self.rows.truncate(at);
            return false;
        }
        true
    }

    /// Items `start..end` of a list: each of them, or ranges of them.
    fn range(
        &mut self,
        parent: &str,
        ty: Option<&'static RuntimeTypeInfo>,
        items: &[Value],
        start: usize,
        end: usize,
        depth: u16,
    ) {
        let count = end - start;
        if count <= CHUNK {
            for index in start..end {
                self.item(parent, ty, items, index, depth);
            }
            return;
        }
        let mut step = CHUNK;
        while count.div_ceil(step) > CHUNK {
            step *= CHUNK;
        }
        for from in (start..end).step_by(step) {
            let to = (from + step).min(end);
            let path = format!("{parent}#{from}+{step}");
            let open = self.toggled.is_open(&path, false);
            self.rows.push(FieldRow {
                path,
                depth,
                label: format!("[{} … {}]", format::count(from), format::count(to - 1)),
                type_label: String::new(),
                display: Display::Summary(format!("{} items", format::count(to - from))),
                open: Some(open),
            });
            if open {
                self.range(parent, ty, items, from, to, depth + 1);
            }
        }
    }

    fn item(
        &mut self,
        parent: &str,
        ty: Option<&'static RuntimeTypeInfo>,
        items: &[Value],
        index: usize,
        depth: u16,
    ) -> bool {
        let value = &items[index];
        let item_ty = ty.and_then(element);
        let type_label = item_ty.map_or_else(
            || json_type(value).to_string(),
            |ty| format::short_type(ty.type_name),
        );
        self.node(
            Node {
                path: format!("{parent}/{index}"),
                label: format!("[{index}]"),
                type_label,
                ty: item_ty,
                value,
                variant: false,
            },
            depth,
        )
    }
}

/// A JSON pointer segment.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Through the wrappers that serialize as what they hold.
fn unwrapped(mut ty: &'static RuntimeTypeInfo) -> &'static RuntimeTypeInfo {
    while let TypeStructure::Wrapper {
        wrapper_kind: WrapperType::Box | WrapperType::Arc | WrapperType::Rc | WrapperType::Option,
        inner,
    } = ty.structure
    {
        ty = inner;
    }
    ty
}

fn element(ty: &'static RuntimeTypeInfo) -> Option<&'static RuntimeTypeInfo> {
    match ty.structure {
        TypeStructure::Wrapper {
            wrapper_kind: WrapperType::Array | WrapperType::Vec | WrapperType::HashSet,
            inner,
        } => Some(inner),
        _ => None,
    }
}

/// An object member's type: a struct's field, or a map's value.
fn member(
    ty: Option<&'static RuntimeTypeInfo>,
    key: &str,
    value: &Value,
) -> (Option<&'static RuntimeTypeInfo>, String) {
    let child = ty.and_then(|ty| match ty.structure {
        TypeStructure::Struct { fields } => fields
            .iter()
            .find(|field| field.name == key)
            .map(|field| field.type_info),
        TypeStructure::Wrapper {
            wrapper_kind: WrapperType::HashMap,
            inner,
        } => Some(inner),
        _ => None,
    });
    let label = child.map_or_else(
        || json_type(value).to_string(),
        |ty| format::short_type(ty.type_name),
    );
    (child, label)
}

fn json_type(value: &Value) -> &'static str {
    if as_entity(value).is_some() {
        return "Entity";
    }
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

fn display(node: &Node<'_>, ty: Option<&'static RuntimeTypeInfo>) -> Display {
    let value = node.value;
    if let Some(entity) = as_entity(value) {
        return Display::Entity(entity);
    }
    if let Some(error) = as_error(value) {
        return Display::Error(error.to_string());
    }
    // Reflection writes a unit enum as its variant's position.
    if let Some(TypeStructure::Enum { variants }) = ty.map(|ty| &ty.structure) {
        let name = match value {
            Value::Number(index) => index
                .as_u64()
                .and_then(|index| variants.get(index as usize))
                .map(|name| name.to_string()),
            Value::String(name) => Some(name.clone()),
            _ => None,
        };
        if let Some(name) = name {
            return Display::Variant(name);
        }
    }
    if let Some(color) = color(&node.label, &node.type_label, value) {
        return color;
    }
    match value {
        Value::Null => Display::Null,
        Value::Bool(value) => Display::Bool(*value),
        Value::Number(number) => Display::Number(number_text(number)),
        Value::String(text) if node.variant => Display::Variant(text.clone()),
        Value::String(text) => Display::Text(text.clone()),
        Value::Array(items) => {
            let numbers = (2..=4).contains(&items.len()) && items.iter().all(Value::is_number);
            if numbers {
                Display::Vector(
                    items
                        .iter()
                        .filter_map(Value::as_number)
                        .map(number_text)
                        .collect(),
                )
            } else if items.is_empty() {
                Display::Summary("empty".to_string())
            } else {
                Display::Summary(format!("{} items", format::count(items.len())))
            }
        }
        Value::Object(map) if map.is_empty() => Display::Summary("{ }".to_string()),
        Value::Object(map) => {
            let mut names: Vec<&str> = map.keys().take(4).map(String::as_str).collect();
            if map.len() > names.len() {
                names.push("…");
            }
            Display::Summary(format!("{{ {} }}", names.join(", ")))
        }
    }
}

fn number_text(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(float) if number.is_f64() => {
            // An f32 widened to f64 prints its widening noise; if the value
            // is exactly an f32, print it as one.
            let narrow = float as f32;
            if f64::from(narrow) == float {
                format!("{narrow:?}")
            } else {
                format!("{float:?}")
            }
        }
        _ => number.to_string(),
    }
}

/// A field or type that says it is a color, holding 3 or 4 numbers.
fn color(label: &str, type_label: &str, value: &Value) -> Option<Display> {
    let says = |text: &str| {
        let text = text.to_ascii_lowercase();
        text.contains("color") || text.contains("colour")
    };
    if !says(label) && !says(type_label) {
        return None;
    }
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => {
            let mut items = vec![map.get("r")?, map.get("g")?, map.get("b")?];
            items.extend(map.get("a"));
            (items.len() == map.len()).then_some(items)?
        }
        _ => return None,
    };
    if !(3..=4).contains(&items.len()) {
        return None;
    }
    let mut rgba = [1.0f32; 4];
    for (slot, item) in rgba.iter_mut().zip(&items) {
        *slot = item.as_f64()? as f32;
    }
    let text = items
        .iter()
        .filter_map(|item| item.as_number().map(number_text))
        .collect::<Vec<_>>()
        .join(", ");
    Some(Display::Color(rgba, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(fields: Vec<Field>) -> FieldSet {
        FieldSet {
            source: FieldSource::Reflected,
            fields,
        }
    }

    fn paths(rows: &[FieldRow]) -> Vec<&str> {
        rows.iter().map(|row| row.path.as_str()).collect()
    }

    #[test]
    fn values_are_drawn_by_what_they_are() {
        let set = set(vec![
            Field::new("flag", "bool", json!(true)),
            Field::new("offset", "[f32; 3]", json!([1.0, 0.5, 0.1f32])),
            Field::new("base_color", "[f32; 4]", json!([1.0, 0.0, 0.0, 1.0])),
            Field::new("target", "Entity", entity_value(Entity::from_bits(5))),
            Field::new("broken", "T", error_value("no serializer")),
        ]);
        let (rows, _) = rows(&set, &Toggled::default(), "");
        assert_eq!(rows[0].display, Display::Bool(true));
        assert_eq!(
            rows[1].display,
            Display::Vector(vec!["1.0".into(), "0.5".into(), "0.1".into()])
        );
        assert!(matches!(rows[2].display, Display::Color([r, ..], _) if r == 1.0));
        assert_eq!(rows[3].display, Display::Entity(Entity::from_bits(5)));
        assert_eq!(rows[4].display, Display::Error("no serializer".into()));
        assert_eq!(set.to_json()["target"], json!("5v0"));
    }

    #[test]
    fn a_huge_list_opens_to_ranges() {
        let set = set(vec![Field::new(
            "samples",
            "Vec<u32>",
            Value::Array((0..25_000u32).map(Value::from).collect()),
        )]);
        let mut toggled = Toggled::default();
        assert_eq!(rows(&set, &toggled, "").0.len(), 1, "closed: one row");

        toggled.toggle("0");
        let (open, _) = rows(&set, &toggled, "");
        // 25,000 items: three ranges of 10,000.
        assert_eq!(paths(&open), ["0", "0#0+10000", "0#10000+10000", "0#20000+10000"]);

        toggled.toggle("0#20000+10000");
        let (deeper, _) = rows(&set, &toggled, "");
        assert_eq!(deeper.len(), 4 + 50);
        toggled.toggle("0#20000+100");
        let (leaves, _) = rows(&set, &toggled, "");
        assert_eq!(leaves.len(), 4 + 50 + 100);
        let leaf = leaves.iter().find(|row| row.path == "0/20042").unwrap();
        assert_eq!(leaf.display, Display::Number("20042".into()));
        assert_eq!(set.value_at("0/20042"), Some(&json!(20042)));
        assert_eq!(set.value_at("0#0+10000"), None);
    }

    #[test]
    fn a_search_shows_matches_and_the_way_to_them() {
        let set = set(vec![
            Field::new(
                "material",
                "Material",
                json!({ "name": "steel", "layers": [{ "roughness": 0.25 }] }),
            ),
            Field::new("other", "u32", json!(3)),
        ]);
        let (found, truncated) = rows(&set, &Toggled::default(), "ROUGH");
        assert!(!truncated);
        assert_eq!(
            paths(&found),
            ["0", "0/layers", "0/layers/0", "0/layers/0/roughness"]
        );
        let (by_value, _) = rows(&set, &Toggled::default(), "steel");
        assert_eq!(paths(&by_value), ["0", "0/name"]);
        assert!(rows(&set, &Toggled::default(), "nothing").0.is_empty());
    }

    #[test]
    fn categories_head_their_fields_and_keys_are_escaped() {
        let mut physics = Field::new("mass", "f32", json!(1.0));
        physics.category = Some("Physics".into());
        let set = set(vec![
            physics,
            Field::new("map", "Map", json!({ "a/b~c": 1 })),
        ]);
        let (rows, _) = rows(&set, &Toggled::default(), "");
        assert_eq!(rows[0].path, "1");
        assert_eq!(rows[2].display, Display::Category);
        assert_eq!(rows[3].label, "mass");
        assert_eq!(set.value_at(&rows[1].path), Some(&json!(1)));
    }
}
