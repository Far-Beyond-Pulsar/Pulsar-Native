//! The widget asset: one `.widget` file.

use std::collections::HashSet;

use serde_json::{json, Map, Value as Json};

use super::geom::{json_array2, num, Vec2};
use super::id::{NodeId, ParamId, WidgetId};
use super::node::Node;
use super::schema::{Kind, SlotKind, ValueClass};
use super::Asset;

/// The file format version this build writes.
pub const FORMAT: u32 = 1;

/// A property of a node that a parameter drives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub node: NodeId,
    pub prop: String,
}

/// A value of the widget that whoever uses it can set. Each parameter drives
/// one or more node properties of the same type.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// Stable; instances set the parameter by this, so renaming is safe.
    pub id: ParamId,
    pub name: String,
    pub class: ValueClass,
    pub default: Json,
    pub description: String,
    pub targets: Vec<Target>,
}

impl Parameter {
    pub fn new(id: ParamId, name: impl Into<String>, class: ValueClass) -> Self {
        Self { id, name: name.into(), class, default: class.default_value(), description: String::new(), targets: Vec::new() }
    }

    fn to_json(&self) -> Json {
        let mut obj = Map::new();
        obj.insert("id".into(), json!(self.id.0));
        obj.insert("name".into(), json!(self.name));
        obj.insert("type".into(), json!(self.class.key()));
        obj.insert("default".into(), self.default.clone());
        if !self.description.is_empty() {
            obj.insert("description".into(), json!(self.description));
        }
        if !self.targets.is_empty() {
            obj.insert(
                "targets".into(),
                Json::Array(self.targets.iter().map(|t| json!({ "node": t.node.0, "prop": t.prop })).collect()),
            );
        }
        Json::Object(obj)
    }

    fn from_json(value: &Json, notes: &mut Vec<String>) -> Option<Parameter> {
        let obj = value.as_object()?;
        let name = obj.get("name").and_then(Json::as_str)?.to_string();
        let class = match obj.get("type").and_then(Json::as_str).and_then(ValueClass::from_key) {
            Some(class) => class,
            None => {
                notes.push(format!("parameter `{name}` has an unknown type and was read as text"));
                ValueClass::Text
            }
        };
        let default = match obj.get("default").and_then(|d| class.coerce(d)) {
            Some(d) => d,
            None => {
                if obj.contains_key("default") {
                    notes.push(format!("parameter `{name}` had an invalid default and was reset"));
                }
                class.default_value()
            }
        };
        let targets = obj
            .get("targets")
            .and_then(Json::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|t| {
                        Some(Target {
                            node: NodeId(t.get("node")?.as_u64()? as u32),
                            prop: t.get("prop")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Parameter {
            id: ParamId(obj.get("id").and_then(Json::as_u64).unwrap_or(0) as u32),
            name,
            class,
            default,
            description: obj.get("description").and_then(Json::as_str).unwrap_or("").to_string(),
            targets,
        })
    }
}

/// A widget: a tree of nodes, the size it is designed for, and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetAsset {
    pub id: WidgetId,
    /// The resolution the layout is designed at.
    pub design_size: Vec2,
    pub description: String,
    pub parameters: Vec<Parameter>,
    pub root: Node,
    next_node_id: u32,
    next_param_id: u32,
    /// The file had no valid id when read; one was made and has to be written.
    needs_stamp: bool,
    /// Top-level fields this version doesn't know.
    extra: Map<String, Json>,
}

impl WidgetAsset {
    pub const DEFAULT_SIZE: Vec2 = Vec2::new(1920.0, 1080.0);

    pub fn new() -> Self {
        let mut widget = Self {
            id: WidgetId::new(),
            design_size: Self::DEFAULT_SIZE,
            description: String::new(),
            parameters: Vec::new(),
            root: Node::new(NodeId(1), "Root", Kind::Canvas),
            next_node_id: 2,
            next_param_id: 1,
            needs_stamp: false,
            extra: Map::new(),
        };
        widget.root.name = "Root".into();
        widget
    }

    /// A node id nothing in this widget has used.
    pub fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// A parameter id nothing in this widget has used.
    pub fn alloc_param_id(&mut self) -> ParamId {
        let id = ParamId(self.next_param_id);
        self.next_param_id += 1;
        id
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.root.find(id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.root.find_mut(id)
    }

    pub fn parameter(&self, id: ParamId) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.id == id)
    }

    pub fn parameter_named(&self, name: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name == name)
    }

    /// The names of this widget's named slots, in tree order.
    pub fn slot_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        self.root.walk(&mut |n| {
            if n.kind == Kind::NamedSlot {
                names.push(n.name.clone());
            }
        });
        names
    }

    /// Whether the id was freshly made on load and not yet on disk.
    pub fn needs_stamp(&self) -> bool {
        self.needs_stamp
    }

    /// Give the widget a new id (for a copied file that kept its original's).
    pub fn regenerate_id(&mut self) {
        self.id = WidgetId::new();
        self.needs_stamp = false;
    }

    pub fn mark_stamped(&mut self) {
        self.needs_stamp = false;
    }

    // ── File ───────────────────────────────────────────────────────────────

    pub fn to_json(&self) -> Json {
        let mut obj = Map::new();
        obj.insert("format".into(), json!(FORMAT));
        obj.insert("id".into(), json!(self.id.as_str()));
        obj.insert("design_size".into(), json!([num(self.design_size.x), num(self.design_size.y)]));
        if !self.description.is_empty() {
            obj.insert("description".into(), json!(self.description));
        }
        if !self.parameters.is_empty() {
            obj.insert("parameters".into(), Json::Array(self.parameters.iter().map(Parameter::to_json).collect()));
        }
        obj.insert("next_node_id".into(), json!(self.next_node_id));
        if !self.parameters.is_empty() {
            obj.insert("next_param_id".into(), json!(self.next_param_id));
        }
        obj.insert("root".into(), self.root.to_json());
        for (k, v) in &self.extra {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
        Json::Object(obj)
    }

    /// The widget's id in `text`, without parsing the rest. Used to index a
    /// project's widgets cheaply.
    pub fn peek_id(text: &str) -> Option<WidgetId> {
        let value: Json = serde_json::from_str(text).ok()?;
        WidgetId::parse(value.get("id")?.as_str()?)
    }
}

impl Default for WidgetAsset {
    fn default() -> Self {
        Self::new()
    }
}

impl Asset for WidgetAsset {
    const KIND: &'static str = "widget";

    fn new_default() -> Self {
        Self::new()
    }

    fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        // An empty file is a new widget, as the file manager creates it.
        if text.trim().is_empty() {
            let mut widget = Self::new();
            widget.needs_stamp = true;
            return Ok((widget, Vec::new()));
        }
        let value: Json = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let obj = value.as_object().ok_or_else(|| "the file isn't a widget (expected a JSON object)".to_string())?;
        let mut notes = Vec::new();

        if let Some(format) = obj.get("format").and_then(Json::as_u64) {
            if format > FORMAT as u64 {
                notes.push(format!("written by a newer version (format {format}); fields this version doesn't know are kept"));
            }
        }

        let (id, needs_stamp) = match obj.get("id").and_then(Json::as_str).and_then(WidgetId::parse) {
            Some(id) => (id, false),
            None => {
                notes.push("had no valid id; a new stable id was assigned".to_string());
                (WidgetId::new(), true)
            }
        };

        let design_size = obj
            .get("design_size")
            .and_then(json_array2)
            .filter(|s| s[0] >= 1.0 && s[1] >= 1.0)
            .map_or(Self::DEFAULT_SIZE, |s| Vec2::new(s[0], s[1]));

        let root = match obj.get("root").and_then(|r| Node::from_json(r, SlotKind::None, &mut notes)) {
            Some(root) => root,
            None => {
                if obj.contains_key("root") {
                    notes.push("the root node was unusable and was replaced with an empty canvas".to_string());
                }
                Node::new(NodeId(1), "Root", Kind::Canvas)
            }
        };

        let parameters = obj
            .get("parameters")
            .and_then(Json::as_array)
            .map(|list| list.iter().filter_map(|p| Parameter::from_json(p, &mut notes)).collect())
            .unwrap_or_default();

        const KNOWN: [&str; 8] =
            ["format", "id", "design_size", "description", "parameters", "next_node_id", "next_param_id", "root"];
        let mut widget = WidgetAsset {
            id,
            design_size,
            description: obj.get("description").and_then(Json::as_str).unwrap_or("").to_string(),
            parameters,
            root,
            next_node_id: obj.get("next_node_id").and_then(Json::as_u64).unwrap_or(0) as u32,
            next_param_id: obj.get("next_param_id").and_then(Json::as_u64).unwrap_or(0) as u32,
            needs_stamp,
            extra: obj.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        widget.repair(&mut notes);
        Ok((widget, notes))
    }

    fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    fn stamp(&mut self) -> bool {
        std::mem::take(&mut self.needs_stamp)
    }
}

impl WidgetAsset {
    /// Make the tree internally consistent after reading: unique node ids,
    /// names that are never empty, parameters that point at real nodes.
    fn repair(&mut self, notes: &mut Vec<String>) {
        // Unique, non-zero node ids.
        let mut max = 0;
        self.root.walk(&mut |n| max = max.max(n.id.0));
        self.next_node_id = self.next_node_id.max(max + 1).max(2);
        let mut seen = HashSet::new();
        let mut next = self.next_node_id;
        let mut reassigned = 0;
        fn visit(node: &mut Node, seen: &mut HashSet<u32>, next: &mut u32, reassigned: &mut usize) {
            if node.id.0 == 0 || !seen.insert(node.id.0) {
                node.id = NodeId(*next);
                *next += 1;
                seen.insert(node.id.0);
                *reassigned += 1;
            }
            for child in &mut node.children {
                visit(child, seen, next, reassigned);
            }
        }
        visit(&mut self.root, &mut seen, &mut next, &mut reassigned);
        self.next_node_id = next;
        if reassigned > 0 {
            notes.push(format!("{reassigned} node(s) shared or lacked an id and were given new ones"));
        }

        // Nodes need names to be told apart in the hierarchy.
        fn name_all(node: &mut Node) {
            if node.name.trim().is_empty() {
                node.name = node.info().label.to_string();
            }
            for child in &mut node.children {
                name_all(child);
            }
        }
        name_all(&mut self.root);

        // Unique, non-zero parameter ids.
        let top = self.parameters.iter().map(|p| p.id.0).max().unwrap_or(0);
        self.next_param_id = self.next_param_id.max(top + 1).max(1);
        let mut seen_params = HashSet::new();
        let mut renumbered = 0;
        for p in &mut self.parameters {
            if p.id.0 == 0 || !seen_params.insert(p.id.0) {
                p.id = ParamId(self.next_param_id);
                self.next_param_id += 1;
                seen_params.insert(p.id.0);
                renumbered += 1;
            }
        }
        if renumbered > 0 {
            notes.push(format!("{renumbered} parameter(s) shared or lacked an id and were given new ones"));
        }

        // Parameters can't point at nodes that aren't there.
        let ids: HashSet<NodeId> = {
            let mut set = HashSet::new();
            self.root.walk(&mut |n| {
                set.insert(n.id);
            });
            set
        };
        for p in &mut self.parameters {
            let before = p.targets.len();
            p.targets.retain(|t| ids.contains(&t.node));
            if p.targets.len() != before {
                notes.push(format!("parameter `{}` pointed at nodes that no longer exist; those links were dropped", p.name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_widget_round_trips() {
        let widget = WidgetAsset::new();
        let (back, notes) = WidgetAsset::parse(&widget.to_text()).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, widget);
        assert!(!back.needs_stamp());
    }

    #[test]
    fn an_empty_file_is_a_new_widget_that_needs_an_id_written() {
        let (mut widget, _) = WidgetAsset::parse("").unwrap();
        assert!(widget.needs_stamp());
        assert!(widget.stamp());
        assert!(!widget.stamp(), "stamping happens once");
    }

    #[test]
    fn a_file_without_an_id_gets_one_and_says_so() {
        let (widget, notes) = WidgetAsset::parse(r#"{ "format": 1, "root": { "id": 1, "name": "R", "kind": "canvas" } }"#).unwrap();
        assert!(widget.needs_stamp());
        assert!(notes.iter().any(|n| n.contains("stable id")));
    }

    #[test]
    fn the_id_survives_everything_else_changing() {
        let mut widget = WidgetAsset::new();
        let id = widget.id.clone();
        widget.root.name = "Renamed".into();
        widget.design_size = Vec2::new(800.0, 600.0);
        let (back, _) = WidgetAsset::parse(&widget.to_text()).unwrap();
        assert_eq!(back.id, id);
    }

    #[test]
    fn unusable_files_are_errors_and_odd_ones_are_repaired() {
        assert!(WidgetAsset::parse("{ nope").is_err());
        assert!(WidgetAsset::parse("[1, 2]").is_err());
        let text = r#"{ "id": "6f1f0a52-3a79-4a0c-9d2a-9e5b0e3a1c11", "future": 1,
            "root": { "id": 1, "name": "", "kind": "canvas", "children": [
               { "id": 1, "name": "A", "kind": "text" }, { "id": 0, "name": "B", "kind": "text" } ] },
            "parameters": [ { "name": "P", "type": "text", "default": "x", "targets": [ {"node": 99, "prop": "text"} ] } ] }"#;
        let (mut widget, notes) = WidgetAsset::parse(text).unwrap();
        let mut ids = HashSet::new();
        widget.root.walk(&mut |n| assert!(ids.insert(n.id), "ids are unique"));
        assert_eq!(widget.root.name, "Canvas Panel", "empty names are filled in");
        assert!(widget.parameters[0].targets.is_empty());
        assert!(notes.len() >= 2, "{notes:?}");
        assert_eq!(widget.to_json()["future"], json!(1), "unknown top-level fields are kept");
        let fresh = widget.alloc_id();
        assert!(!ids.contains(&fresh));
    }

    #[test]
    fn peek_reads_only_the_id() {
        let widget = WidgetAsset::new();
        assert_eq!(WidgetAsset::peek_id(&widget.to_text()), Some(widget.id));
        assert_eq!(WidgetAsset::peek_id("garbage"), None);
        assert_eq!(WidgetAsset::peek_id(r#"{"id": 5}"#), None);
    }
}
