//! The widget tree: nodes, their properties and instances of other widgets.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::{json, Map, Value as Json};

use super::geom::{json_array2, json_array4, json_f32, Color, Insets, Vec2};
use super::id::{AssetPath, NodeId, ParamId, WidgetId};
use super::schema::{Kind, KindInfo, PropDef, SlotKind};

/// Property values by key. Sparse: a key that is absent has its default.
/// Keys the schema doesn't know are kept as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props(pub BTreeMap<String, Json>);

impl Props {
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.0.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn remove(&mut self, key: &str) -> Option<Json> {
        self.0.remove(key)
    }

    /// Store `value`. A value equal to the default is removed instead, so
    /// files only carry what was changed.
    pub fn set(&mut self, def: &PropDef, value: Json) {
        if !def.optional && value == def.default.json() {
            self.0.remove(def.key);
        } else {
            self.0.insert(def.key.to_string(), value);
        }
    }

    fn to_json(&self) -> Json {
        Json::Object(self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// Where a property's definition is looked up.
#[derive(Clone, Copy)]
pub enum Table {
    Kind(&'static KindInfo),
    Slot(SlotKind),
}

impl Table {
    pub fn def(&self, key: &str) -> Option<&'static PropDef> {
        match self {
            Table::Kind(info) => info.def(key),
            Table::Slot(slot) => slot.def(key),
        }
    }
}

/// Typed reads of a [`Props`] against its schema. A stored value of the
/// wrong type reads as the default.
pub struct View<'a> {
    pub props: &'a Props,
    pub table: Table,
}

impl View<'_> {
    pub fn json(&self, key: &str) -> Cow<'_, Json> {
        let def = self.table.def(key);
        match (self.props.get(key), def) {
            (Some(v), Some(d)) => match d.ty.coerce(v) {
                Some(valid) => Cow::Owned(valid),
                None => Cow::Owned(d.default.json()),
            },
            (Some(v), None) => Cow::Borrowed(v),
            (None, Some(d)) => Cow::Owned(d.default.json()),
            (None, None) => Cow::Owned(Json::Null),
        }
    }

    /// Whether an optional property is switched on.
    pub fn is_set(&self, key: &str) -> bool {
        self.props.get(key).is_some_and(|v| !v.is_null())
    }

    pub fn f32(&self, key: &str) -> f32 {
        json_f32(&self.json(key)).unwrap_or(0.0)
    }

    pub fn opt_f32(&self, key: &str) -> Option<f32> {
        self.props.get(key).and_then(|v| self.table.def(key).and_then(|d| d.ty.coerce(v))).and_then(|v| json_f32(&v))
    }

    pub fn i32(&self, key: &str) -> i32 {
        self.f32(key).round() as i32
    }

    pub fn bool(&self, key: &str) -> bool {
        self.json(key).as_bool().unwrap_or(false)
    }

    pub fn text(&self, key: &str) -> String {
        self.json(key).as_str().unwrap_or_default().to_string()
    }

    pub fn choice(&self, key: &str) -> String {
        self.text(key)
    }

    pub fn color(&self, key: &str) -> Color {
        self.json(key).as_str().and_then(Color::from_hex).unwrap_or(Color::TRANSPARENT)
    }

    pub fn vec2(&self, key: &str) -> Vec2 {
        json_array2(&self.json(key)).map_or(Vec2::ZERO, |a| Vec2::new(a[0], a[1]))
    }

    pub fn quad(&self, key: &str) -> [f32; 4] {
        json_array4(&self.json(key)).unwrap_or([0.0; 4])
    }

    pub fn insets(&self, key: &str) -> Insets {
        let [l, t, r, b] = self.quad(key);
        Insets::new(l, t, r, b)
    }

    pub fn image(&self, key: &str) -> AssetPath {
        AssetPath(self.text(key))
    }
}

/// What a parameter hands an instance's parameter: a value, or the
/// instancing widget's own parameter passed through.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Value(Json),
    Bind(ParamId),
}

impl Arg {
    fn to_json(&self) -> Json {
        match self {
            Arg::Value(v) => v.clone(),
            Arg::Bind(param) => json!({ "bind": param.0 }),
        }
    }

    fn from_json(v: &Json) -> Arg {
        if let Some(obj) = v.as_object() {
            if obj.len() == 1 {
                if let Some(param) = obj.get("bind").and_then(Json::as_u64) {
                    return Arg::Bind(ParamId(param as u32));
                }
            }
        }
        Arg::Value(v.clone())
    }
}

/// A node that is another widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// The widget, by id. This is the only thing that identifies it.
    pub widget: WidgetId,
    /// What the widget was called when last seen. Shown if it can't be found;
    /// never used to look it up.
    pub name_hint: String,
    /// Values for the widget's parameters, by parameter id.
    pub overrides: BTreeMap<ParamId, Arg>,
}

/// Editor-only state saved with the node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorState {
    /// Hidden in the designer (not in the game).
    pub hidden: bool,
    /// Can't be selected or moved in the designer.
    pub locked: bool,
    /// Open in the hierarchy.
    pub expanded: bool,
}

impl Default for EditorState {
    fn default() -> Self {
        Self { hidden: false, locked: false, expanded: true }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub kind: Kind,
    pub props: Props,
    /// Layout properties given by the parent (see [`SlotKind`]).
    pub slot: Props,
    pub children: Vec<Node>,
    pub instance: Option<Instance>,
    /// For a child of an instance: the named slot of that widget it fills.
    pub for_slot: Option<String>,
    pub editor: EditorState,
    /// The kind's name as written, for [`Kind::Unknown`].
    pub unknown_kind: Option<String>,
    /// Fields of the node this version doesn't know.
    pub extra: Map<String, Json>,
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>, kind: Kind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            props: Props::default(),
            slot: Props::default(),
            children: Vec::new(),
            instance: None,
            for_slot: None,
            editor: EditorState::default(),
            unknown_kind: None,
            extra: Map::new(),
        }
    }

    pub fn info(&self) -> &'static KindInfo {
        self.kind.info()
    }

    /// Typed reads of this node's properties.
    pub fn view(&self) -> View<'_> {
        View { props: &self.props, table: Table::Kind(self.info()) }
    }

    /// Typed reads of this node's slot properties, as `parent`'s child.
    pub fn slot_view(&self, parent: SlotKind) -> View<'_> {
        View { props: &self.slot, table: Table::Slot(parent) }
    }

    pub fn set_prop(&mut self, key: &str, value: Json) {
        if let Some(def) = self.info().def(key) {
            self.props.set(def, value);
        }
    }

    pub fn set_slot(&mut self, parent: SlotKind, key: &str, value: Json) {
        if let Some(def) = parent.def(key) {
            self.slot.set(def, value);
        }
    }

    pub fn reset_prop(&mut self, key: &str) {
        self.props.remove(key);
    }

    pub fn find(&self, id: NodeId) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// This node and everything under it, parents before children.
    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a Node)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }

    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(Node::depth).max().unwrap_or(0)
    }

    /// Whether the node takes part in layout and drawing at all.
    pub fn is_collapsed(&self) -> bool {
        self.view().choice("visibility") == "collapsed"
    }

    // ── JSON ───────────────────────────────────────────────────────────────

    pub fn to_json(&self) -> Json {
        let mut obj = Map::new();
        obj.insert("id".into(), json!(self.id.0));
        obj.insert("name".into(), json!(self.name));
        obj.insert("kind".into(), json!(self.unknown_kind.as_deref().unwrap_or(self.kind.key())));
        if !self.props.is_empty() {
            obj.insert("props".into(), self.props.to_json());
        }
        if !self.slot.is_empty() {
            obj.insert("slot".into(), self.slot.to_json());
        }
        if let Some(instance) = &self.instance {
            obj.insert("widget".into(), json!(instance.widget.as_str()));
            if !instance.name_hint.is_empty() {
                obj.insert("widget_name".into(), json!(instance.name_hint));
            }
            if !instance.overrides.is_empty() {
                obj.insert(
                    "overrides".into(),
                    Json::Object(instance.overrides.iter().map(|(k, v)| (k.0.to_string(), v.to_json())).collect()),
                );
            }
        }
        if let Some(slot) = &self.for_slot {
            obj.insert("for_slot".into(), json!(slot));
        }
        let mut editor = Map::new();
        if self.editor.hidden {
            editor.insert("hidden".into(), json!(true));
        }
        if self.editor.locked {
            editor.insert("locked".into(), json!(true));
        }
        if !self.editor.expanded {
            editor.insert("expanded".into(), json!(false));
        }
        if !editor.is_empty() {
            obj.insert("editor".into(), Json::Object(editor));
        }
        if !self.children.is_empty() {
            obj.insert("children".into(), Json::Array(self.children.iter().map(Node::to_json).collect()));
        }
        for (k, v) in &self.extra {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
        Json::Object(obj)
    }

    /// Read a node leniently: what can't be used is dropped and reported in
    /// `notes`. `parent_slot` says which slot properties are valid.
    /// `None` when `value` isn't a node at all.
    pub fn from_json(value: &Json, parent_slot: SlotKind, notes: &mut Vec<String>) -> Option<Node> {
        let obj = value.as_object()?;
        let name = obj.get("name").and_then(Json::as_str).unwrap_or("").to_string();
        let kind_key = obj.get("kind").and_then(Json::as_str).unwrap_or("");
        let (kind, unknown_kind) = match Kind::from_key(kind_key) {
            Some(kind) => (kind, None),
            None => {
                notes.push(format!("`{name}` has the unknown kind `{kind_key}`; it is kept but not drawn"));
                (Kind::Unknown, Some(kind_key.to_string()))
            }
        };
        let id = obj.get("id").and_then(Json::as_u64).map_or(NodeId(0), |n| NodeId(n as u32));
        let mut node = Node::new(id, name, kind);
        node.unknown_kind = unknown_kind;

        node.props = read_props(obj.get("props"), Table::Kind(kind.info()), &node.name, notes);
        node.slot = read_props(obj.get("slot"), Table::Slot(parent_slot), &node.name, notes);

        if kind == Kind::Instance {
            match obj.get("widget").and_then(Json::as_str).and_then(WidgetId::parse) {
                Some(widget) => {
                    let overrides = obj
                        .get("overrides")
                        .and_then(Json::as_object)
                        .map(|o| o.iter().filter_map(|(k, v)| Some((ParamId(k.parse().ok()?), Arg::from_json(v)))).collect())
                        .unwrap_or_default();
                    node.instance = Some(Instance {
                        widget,
                        name_hint: obj.get("widget_name").and_then(Json::as_str).unwrap_or("").to_string(),
                        overrides,
                    });
                }
                None => {
                    notes.push(format!("`{}` refers to a widget with no valid id; it will show as missing", node.name));
                    node.instance = Some(Instance {
                        widget: WidgetId::parse("00000000-0000-0000-0000-000000000000").expect("nil uuid parses"),
                        name_hint: obj.get("widget_name").and_then(Json::as_str).unwrap_or("").to_string(),
                        overrides: BTreeMap::new(),
                    });
                }
            }
        }
        node.for_slot = obj.get("for_slot").and_then(Json::as_str).map(str::to_string);
        if let Some(editor) = obj.get("editor").and_then(Json::as_object) {
            node.editor.hidden = editor.get("hidden").and_then(Json::as_bool).unwrap_or(false);
            node.editor.locked = editor.get("locked").and_then(Json::as_bool).unwrap_or(false);
            node.editor.expanded = editor.get("expanded").and_then(Json::as_bool).unwrap_or(true);
        }

        let child_slot = kind.info().slot;
        if let Some(children) = obj.get("children").and_then(Json::as_array) {
            for child in children {
                match Node::from_json(child, child_slot, notes) {
                    Some(child) => node.children.push(child),
                    None => notes.push(format!("skipped an entry under `{}` that isn't a widget node", node.name)),
                }
            }
        }

        const KNOWN: [&str; 13] = [
            "id", "name", "kind", "props", "slot", "children", "widget", "widget_name", "overrides", "for_slot", "editor", "extra", "_",
        ];
        node.extra = obj.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        Some(node)
    }
}

fn read_props(value: Option<&Json>, table: Table, owner: &str, notes: &mut Vec<String>) -> Props {
    let Some(obj) = value.and_then(Json::as_object) else { return Props::default() };
    let mut props = Props::default();
    for (key, raw) in obj {
        match table.def(key) {
            Some(def) if def.optional && raw.is_null() => {}
            Some(def) => match def.ty.coerce(raw) {
                Some(valid) => {
                    props.0.insert(key.clone(), valid);
                }
                None => notes.push(format!("`{owner}`: {} had an invalid value and was reset", def.label)),
            },
            // Not ours to interpret; keep it for the version that wrote it.
            None => {
                props.0.insert(key.clone(), raw.clone());
            }
        }
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: Json) -> (Node, Vec<String>) {
        let mut notes = Vec::new();
        let node = Node::from_json(&value, SlotKind::Canvas, &mut notes).unwrap();
        (node, notes)
    }

    #[test]
    fn defaults_come_from_the_schema_and_stored_values_win() {
        let mut node = Node::new(NodeId(1), "T", Kind::Text);
        assert_eq!(node.view().f32("font_size"), 16.0);
        assert_eq!(node.view().color("color"), Color::WHITE);
        node.set_prop("font_size", json!(24.0));
        assert_eq!(node.view().f32("font_size"), 24.0);
        // Setting the default again removes the key.
        node.set_prop("font_size", json!(16.0));
        assert!(node.props.is_empty());
        // Optional properties are on only when present.
        let mut size = Node::new(NodeId(2), "S", Kind::SizeBox);
        assert_eq!(size.view().opt_f32("width"), None);
        size.set_prop("width", json!(200.0));
        assert_eq!(size.view().opt_f32("width"), Some(200.0));
        assert!(size.view().is_set("width"));
    }

    #[test]
    fn slot_values_read_against_the_parents_slot_kind() {
        let mut node = Node::new(NodeId(1), "T", Kind::Text);
        assert_eq!(node.slot_view(SlotKind::Canvas).quad("offsets"), [0.0, 0.0, 100.0, 30.0]);
        node.set_slot(SlotKind::Canvas, "offsets", json!([5.0, 6.0, 7.0, 8.0]));
        assert_eq!(node.slot_view(SlotKind::Canvas).quad("offsets"), [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(node.slot_view(SlotKind::Box).choice("h_align"), "fill");
    }

    #[test]
    fn a_node_round_trips_and_keeps_what_it_does_not_understand() {
        let source = json!({
            "id": 4, "name": "Title", "kind": "text",
            "props": { "text": "Hi", "future_prop": [1, 2] },
            "slot": { "offsets": [1.0, 2.0, 3.0, 4.0] },
            "editor": { "locked": true },
            "children": [],
            "from_the_future": { "a": 1 }
        });
        let (node, notes) = parse(source);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(node.editor.locked);
        let out = node.to_json();
        assert_eq!(out["props"]["future_prop"], json!([1, 2]));
        assert_eq!(out["from_the_future"], json!({ "a": 1 }));
        assert_eq!(out["editor"], json!({ "locked": true }));
        assert!(out.get("children").is_none(), "empty lists are not written");
    }

    #[test]
    fn invalid_values_are_reset_and_reported() {
        let (node, notes) = parse(json!({
            "id": 1, "name": "T", "kind": "text",
            "props": { "font_size": "huge", "color": "red", "justify": "sideways", "text": "ok" }
        }));
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert_eq!(node.view().f32("font_size"), 16.0);
        assert_eq!(node.view().text("text"), "ok");
    }

    #[test]
    fn unknown_kinds_survive_a_round_trip() {
        let (node, notes) = parse(json!({ "id": 1, "name": "X", "kind": "hologram", "props": { "q": 1 } }));
        assert_eq!(node.kind, Kind::Unknown);
        assert_eq!(notes.len(), 1);
        assert_eq!(node.to_json()["kind"], json!("hologram"));
        assert_eq!(node.to_json()["props"]["q"], json!(1));
    }

    #[test]
    fn instances_keep_their_widget_id_overrides_and_bindings() {
        let id = WidgetId::new();
        let source = json!({
            "id": 2, "name": "Card", "kind": "widget", "widget": id.as_str(), "widget_name": "ItemCard",
            "overrides": { "1": "Sword", "2": { "bind": 7 } },
            "children": [ { "id": 3, "name": "Body", "kind": "text", "for_slot": "Content" } ]
        });
        let (node, notes) = parse(source);
        assert!(notes.is_empty(), "{notes:?}");
        let instance = node.instance.as_ref().unwrap();
        assert_eq!(instance.widget, id);
        assert_eq!(instance.overrides[&ParamId(1)], Arg::Value(json!("Sword")));
        assert_eq!(instance.overrides[&ParamId(2)], Arg::Bind(ParamId(7)));
        assert_eq!(node.children[0].for_slot.as_deref(), Some("Content"));
        let out = node.to_json();
        assert_eq!(out["widget"], json!(id.as_str()));
        assert_eq!(out["overrides"]["2"], json!({ "bind": 7 }));
    }
}
