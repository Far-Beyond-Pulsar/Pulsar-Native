//! Editing operations on a widget: structural changes to the tree and to the
//! parameters. Each leaves the widget consistent (parameters never point at
//! deleted nodes, slot properties always belong to the parent's slot kind),
//! so the editor can call them without further checks.

use std::collections::HashSet;

use serde_json::Value as Json;

use super::id::{NodeId, ParamId, WidgetId};
use super::node::{Arg, Instance, Node};
use super::schema::{Children, Kind, SlotKind, ValueClass};
use super::widget::{Parameter, Target, WidgetAsset};

/// Why an operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpError {
    NotFound,
    /// The root can't be removed, moved or duplicated.
    IsRoot,
    /// This kind of node holds no children.
    NoChildren(&'static str),
    /// A single-child container that already has its child.
    Full(&'static str),
    /// Moving a node into itself or one of its own descendants.
    IntoSelf,
    /// The nodes to wrap don't share a parent.
    NotSiblings,
    BadName,
    /// The property doesn't exist or isn't the parameter's type.
    BadTarget,
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::NotFound => f.write_str("that widget isn't in the tree"),
            OpError::IsRoot => f.write_str("the root can't be removed or moved"),
            OpError::NoChildren(kind) => write!(f, "{kind} can't hold other widgets"),
            OpError::Full(kind) => write!(f, "{kind} holds only one widget"),
            OpError::IntoSelf => f.write_str("a widget can't be placed inside itself"),
            OpError::NotSiblings => f.write_str("only widgets with the same parent can be grouped"),
            OpError::BadName => f.write_str("names can't be empty"),
            OpError::BadTarget => f.write_str("that property can't be driven by this parameter"),
        }
    }
}

impl WidgetAsset {
    // ── Queries ────────────────────────────────────────────────────────────

    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        fn find(node: &Node, id: NodeId) -> Option<NodeId> {
            if node.children.iter().any(|c| c.id == id) {
                return Some(node.id);
            }
            node.children.iter().find_map(|c| find(c, id))
        }
        find(&self.root, id)
    }

    /// Root to `id`, inclusive.
    pub fn path_to(&self, id: NodeId) -> Option<Vec<NodeId>> {
        fn go(node: &Node, id: NodeId, path: &mut Vec<NodeId>) -> bool {
            path.push(node.id);
            if node.id == id || node.children.iter().any(|c| go(c, id, path)) {
                return true;
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        go(&self.root, id, &mut path).then_some(path)
    }

    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.node(self.parent_of(id)?)?;
        parent.children.iter().position(|c| c.id == id)
    }

    /// Whether `ancestor` is `node` or above it.
    pub fn is_within(&self, ancestor: NodeId, node: NodeId) -> bool {
        self.node(ancestor).is_some_and(|a| a.find(node).is_some())
    }

    /// The slot kind `id`'s own slot properties belong to.
    pub fn slot_kind_of(&self, id: NodeId) -> SlotKind {
        self.parent_of(id).and_then(|p| self.node(p)).map_or(SlotKind::None, |p| p.info().slot)
    }

    pub fn all_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        self.root.walk(&mut |n| {
            names.insert(n.name.clone());
        });
        names
    }

    /// `base` if no node has it, else `base_1`, `base_2`, ...
    pub fn unique_name(&self, base: &str) -> String {
        let names = self.all_names();
        let base = base.trim();
        let stem = match base.rsplit_once('_') {
            Some((stem, n)) if !stem.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => stem,
            _ => base,
        };
        if !names.contains(base) {
            return base.to_string();
        }
        (1..).map(|n| format!("{stem}_{n}")).find(|c| !names.contains(c)).expect("an unused name exists")
    }

    // ── Creating ───────────────────────────────────────────────────────────

    /// A new, unattached node of `kind` with a unique id and name.
    pub fn new_node(&mut self, kind: Kind) -> Node {
        let name = self.unique_name(&kind.info().label.replace(' ', ""));
        let id = self.alloc_id();
        Node::new(id, name, kind)
    }

    /// A new, unattached instance of `widget`.
    pub fn new_instance(&mut self, widget: WidgetId, name_hint: &str) -> Node {
        let name = self.unique_name(&sanitize_name(name_hint, "Widget"));
        let id = self.alloc_id();
        let mut node = Node::new(id, name, Kind::Instance);
        node.instance = Some(Instance { widget, name_hint: name_hint.to_string(), overrides: Default::default() });
        node
    }

    /// A copy of `source` and everything under it with fresh ids, and names
    /// that don't collide with this widget's. Parameter links are not copied.
    pub fn fresh_copy(&mut self, source: &Node) -> Node {
        let mut copy = source.clone();
        let mut taken = self.all_names();
        fn visit(node: &mut Node, widget: &mut WidgetAsset, taken: &mut HashSet<String>) {
            node.id = widget.alloc_id();
            node.name = next_free(&node.name, taken);
            taken.insert(node.name.clone());
            for child in &mut node.children {
                visit(child, widget, taken);
            }
        }
        visit(&mut copy, self, &mut taken);
        copy
    }

    // ── Structure ──────────────────────────────────────────────────────────

    /// Whether `parent` can take another child.
    pub fn can_accept(&self, parent: NodeId) -> Result<(), OpError> {
        let parent = self.node(parent).ok_or(OpError::NotFound)?;
        let info = parent.info();
        match info.children {
            Children::None => Err(OpError::NoChildren(info.label)),
            Children::One if !parent.children.is_empty() => Err(OpError::Full(info.label)),
            _ => Ok(()),
        }
    }

    /// Put `node` under `parent` at `index` (the end when `None`).
    pub fn add(&mut self, parent: NodeId, index: Option<usize>, mut node: Node) -> Result<NodeId, OpError> {
        self.can_accept(parent)?;
        let (slot, takes_slots) = {
            let p = self.node(parent).ok_or(OpError::NotFound)?;
            (p.info().slot, p.info().children == Children::Slots)
        };
        retain_compatible_slot(&mut node, slot);
        if !takes_slots {
            node.for_slot = None;
        }
        let id = node.id;
        let p = self.node_mut(parent).ok_or(OpError::NotFound)?;
        let at = index.unwrap_or(p.children.len()).min(p.children.len());
        p.children.insert(at, node);
        Ok(id)
    }

    /// Remove `id` and its subtree, and every parameter link into it.
    pub fn remove(&mut self, id: NodeId) -> Result<Node, OpError> {
        if id == self.root.id {
            return Err(OpError::IsRoot);
        }
        let parent = self.parent_of(id).ok_or(OpError::NotFound)?;
        let p = self.node_mut(parent).ok_or(OpError::NotFound)?;
        let at = p.children.iter().position(|c| c.id == id).ok_or(OpError::NotFound)?;
        let removed = p.children.remove(at);
        let mut gone = HashSet::new();
        removed.walk(&mut |n| {
            gone.insert(n.id);
        });
        for param in &mut self.parameters {
            param.targets.retain(|t| !gone.contains(&t.node));
        }
        Ok(removed)
    }

    /// Move `id` under `parent` at `index`.
    pub fn move_node(&mut self, id: NodeId, parent: NodeId, index: Option<usize>) -> Result<(), OpError> {
        if id == self.root.id {
            return Err(OpError::IsRoot);
        }
        if self.is_within(id, parent) {
            return Err(OpError::IntoSelf);
        }
        let old_parent = self.parent_of(id).ok_or(OpError::NotFound)?;
        if old_parent != parent {
            self.can_accept(parent)?;
        } else if self.node(parent).ok_or(OpError::NotFound)?.info().children == Children::None {
            return Err(OpError::NotFound);
        }
        let old_index = self.index_in_parent(id).ok_or(OpError::NotFound)?;

        // Detach without touching parameter links: the node is only moving.
        let p = self.node_mut(old_parent).ok_or(OpError::NotFound)?;
        let node = p.children.remove(old_index);

        let new_slot = self.node(parent).ok_or(OpError::NotFound)?.info().slot;
        let takes_slots = self.node(parent).ok_or(OpError::NotFound)?.info().children == Children::Slots;
        let mut node = node;
        retain_compatible_slot(&mut node, new_slot);
        if !takes_slots {
            node.for_slot = None;
        }
        let p = self.node_mut(parent).ok_or(OpError::NotFound)?;
        let mut at = index.unwrap_or(p.children.len());
        if old_parent == parent && old_index < at {
            at -= 1;
        }
        let at = at.min(p.children.len());
        p.children.insert(at, node);
        Ok(())
    }

    /// Copy `id` (with its subtree) to just after itself.
    pub fn duplicate(&mut self, id: NodeId) -> Result<NodeId, OpError> {
        if id == self.root.id {
            return Err(OpError::IsRoot);
        }
        let parent = self.parent_of(id).ok_or(OpError::NotFound)?;
        self.can_accept(parent)?;
        let source = self.node(id).ok_or(OpError::NotFound)?.clone();
        let copy = self.fresh_copy(&source);
        let at = self.index_in_parent(id).map(|i| i + 1);
        self.add(parent, at, copy)
    }

    /// Put `ids` (siblings) inside a new container of `kind`, which takes
    /// their place.
    pub fn wrap(&mut self, ids: &[NodeId], kind: Kind) -> Result<NodeId, OpError> {
        let first = *ids.first().ok_or(OpError::NotFound)?;
        let container_info = kind.info();
        match container_info.children {
            Children::None | Children::Slots => return Err(OpError::NoChildren(container_info.label)),
            Children::One if ids.len() > 1 => return Err(OpError::Full(container_info.label)),
            _ => {}
        }
        let is_root = first == self.root.id;
        if is_root && ids.len() > 1 {
            return Err(OpError::NotSiblings);
        }
        let parent = if is_root { None } else { Some(self.parent_of(first).ok_or(OpError::NotFound)?) };
        if let Some(parent) = parent {
            if ids.iter().any(|&i| self.parent_of(i) != Some(parent)) {
                return Err(OpError::NotSiblings);
            }
        }

        let mut container = self.new_node(kind);
        let container_id = container.id;
        if is_root {
            let old_root = std::mem::replace(&mut self.root, Node::new(container_id, "", kind));
            container.children.push(old_root);
            retain_compatible_slot(&mut container.children[0], container_info.slot);
            let new_root = &mut container;
            std::mem::swap(&mut self.root, new_root);
            return Ok(container_id);
        }

        let parent = parent.expect("non-root has a parent");
        let parent_node = self.node(parent).ok_or(OpError::NotFound)?;
        // Keep the siblings' order, and put the container where the first was.
        let mut order: Vec<(usize, NodeId)> = ids
            .iter()
            .filter_map(|&i| parent_node.children.iter().position(|c| c.id == i).map(|p| (p, i)))
            .collect();
        order.sort_unstable();
        let at = order[0].0;
        // The container takes the first node's place in the parent's layout.
        container.slot = parent_node.children[at].slot.clone();
        container.for_slot = parent_node.children[at].for_slot.clone();

        let mut inner = Vec::new();
        for &(_, id) in &order {
            inner.push(self.remove_keeping_links(id)?);
        }
        for mut child in inner {
            child.slot = Default::default();
            child.for_slot = None;
            container.children.push(child);
        }
        self.add(parent, Some(at), container)?;
        Ok(container_id)
    }

    /// Replace the container `id` with its children.
    pub fn unwrap(&mut self, id: NodeId) -> Result<(), OpError> {
        if id == self.root.id {
            return Err(OpError::IsRoot);
        }
        let parent = self.parent_of(id).ok_or(OpError::NotFound)?;
        let node = self.node(id).ok_or(OpError::NotFound)?;
        if node.children.is_empty() {
            return Err(OpError::NotFound);
        }
        let parent_node = self.node(parent).ok_or(OpError::NotFound)?;
        if parent_node.info().children == Children::One && node.children.len() > 1 {
            return Err(OpError::Full(parent_node.info().label));
        }
        let at = self.index_in_parent(id).ok_or(OpError::NotFound)?;
        let slot_of_container = node.slot.clone();
        let for_slot = node.for_slot.clone();
        let mut removed = self.remove_keeping_links(id)?;
        let children = std::mem::take(&mut removed.children);
        for (offset, mut child) in children.into_iter().enumerate() {
            // The first child inherits the container's place.
            if offset == 0 {
                child.slot = slot_of_container.clone();
                child.for_slot = for_slot.clone();
            } else {
                child.slot = Default::default();
            }
            self.add(parent, Some(at + offset), child)?;
        }
        // The container itself is gone: its own links go with it.
        for param in &mut self.parameters {
            param.targets.retain(|t| t.node != id);
        }
        Ok(())
    }

    fn remove_keeping_links(&mut self, id: NodeId) -> Result<Node, OpError> {
        let parent = self.parent_of(id).ok_or(OpError::NotFound)?;
        let p = self.node_mut(parent).ok_or(OpError::NotFound)?;
        let at = p.children.iter().position(|c| c.id == id).ok_or(OpError::NotFound)?;
        Ok(p.children.remove(at))
    }

    pub fn rename(&mut self, id: NodeId, name: &str) -> Result<(), OpError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(OpError::BadName);
        }
        self.node_mut(id).ok_or(OpError::NotFound)?.name = name.to_string();
        Ok(())
    }

    // ── Parameters ─────────────────────────────────────────────────────────

    /// A new parameter named `name` (made unique) of `class`.
    pub fn add_parameter(&mut self, name: &str, class: ValueClass) -> ParamId {
        let taken: HashSet<String> = self.parameters.iter().map(|p| p.name.clone()).collect();
        let name = next_free(&sanitize_name(name, "Parameter"), &taken);
        let id = self.alloc_param_id();
        self.parameters.push(Parameter::new(id, name, class));
        id
    }

    /// Remove a parameter, and every pass-through of it by this widget's
    /// instances.
    pub fn remove_parameter(&mut self, id: ParamId) {
        self.parameters.retain(|p| p.id != id);
        fn clear(node: &mut Node, id: ParamId) {
            if let Some(instance) = &mut node.instance {
                instance.overrides.retain(|_, arg| *arg != Arg::Bind(id));
            }
            for child in &mut node.children {
                clear(child, id);
            }
        }
        clear(&mut self.root, id);
    }

    /// Make `param` drive `prop` of `node`. The property must exist and have
    /// the parameter's type.
    pub fn link_parameter(&mut self, param: ParamId, node: NodeId, prop: &str) -> Result<(), OpError> {
        let class = self.parameter(param).ok_or(OpError::NotFound)?.class;
        let def = self.node(node).ok_or(OpError::NotFound)?.info().def(prop).ok_or(OpError::BadTarget)?;
        if def.ty.class() != class {
            return Err(OpError::BadTarget);
        }
        // A property has one driver.
        for p in &mut self.parameters {
            p.targets.retain(|t| !(t.node == node && t.prop == prop));
        }
        let p = self.parameters.iter_mut().find(|p| p.id == param).ok_or(OpError::NotFound)?;
        p.targets.push(Target { node, prop: prop.to_string() });
        Ok(())
    }

    pub fn unlink_property(&mut self, node: NodeId, prop: &str) {
        for p in &mut self.parameters {
            p.targets.retain(|t| !(t.node == node && t.prop == prop));
        }
    }

    /// The parameter driving `prop` of `node`, if any.
    pub fn driver_of(&self, node: NodeId, prop: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.targets.iter().any(|t| t.node == node && t.prop == prop))
    }

    /// Set a parameter's default from a property's current value, so
    /// linking it doesn't change what the widget looks like.
    pub fn adopt_value(&mut self, param: ParamId, value: Json) {
        if let Some(p) = self.parameters.iter_mut().find(|p| p.id == param) {
            if let Some(v) = p.class.coerce(&value) {
                p.default = v;
            }
        }
    }
}

/// Drop slot properties that don't exist in, or are invalid for, `slot`.
pub fn retain_compatible_slot(node: &mut Node, slot: SlotKind) {
    node.slot.0.retain(|key, value| slot.def(key).is_some_and(|d| d.ty.coerce(value).is_some()));
}

/// A name usable for a node or parameter: letters, digits and underscores.
pub fn sanitize_name(text: &str, fallback: &str) -> String {
    let cleaned: String = text
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// `base`, or `base_1`, `base_2`, ... the first that isn't in `taken`.
fn next_free(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    let stem = match base.rsplit_once('_') {
        Some((stem, n)) if !stem.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => stem,
        _ => base,
    };
    (1..).map(|n| format!("{stem}_{n}")).find(|c| !taken.contains(c)).expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Root canvas with a vertical box holding two texts and a button.
    fn sample() -> (WidgetAsset, NodeId, NodeId, NodeId, NodeId) {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let vbox = w.new_node(Kind::VerticalBox);
        let vbox_id = vbox.id;
        w.add(root, None, vbox).unwrap();
        let a = w.new_node(Kind::Text);
        let a_id = a.id;
        w.add(vbox_id, None, a).unwrap();
        let b = w.new_node(Kind::Text);
        let b_id = b.id;
        w.add(vbox_id, None, b).unwrap();
        let button = w.new_node(Kind::Button);
        let button_id = button.id;
        w.add(vbox_id, None, button).unwrap();
        (w, vbox_id, a_id, b_id, button_id)
    }

    #[test]
    fn new_nodes_get_unique_ids_and_names() {
        let (w, _, a, b, _) = sample();
        assert_ne!(a, b);
        assert_eq!(w.node(a).unwrap().name, "Text");
        assert_eq!(w.node(b).unwrap().name, "Text_1");
        assert_eq!(w.unique_name("Text_1"), "Text_2");
        assert_eq!(w.unique_name("Fresh"), "Fresh");
    }

    #[test]
    fn containers_enforce_their_capacity() {
        let (mut w, _, a, _, button) = sample();
        let extra = w.new_node(Kind::Text);
        w.add(button, None, extra).unwrap();
        let second = w.new_node(Kind::Text);
        assert_eq!(w.add(button, None, second.clone()), Err(OpError::Full("Button")));
        assert_eq!(w.add(a, None, second), Err(OpError::NoChildren("Text")));
    }

    #[test]
    fn removing_a_node_removes_the_parameter_links_into_it() {
        let (mut w, vbox, a, b, _) = sample();
        let p = w.add_parameter("Title", ValueClass::Text);
        w.link_parameter(p, a, "text").unwrap();
        w.link_parameter(p, b, "text").unwrap();
        assert_eq!(w.parameter(p).unwrap().targets.len(), 2);
        w.remove(a).unwrap();
        assert_eq!(w.parameter(p).unwrap().targets.len(), 1);
        w.remove(vbox).unwrap();
        assert!(w.parameter(p).unwrap().targets.is_empty());
        assert_eq!(w.remove(w.root.id), Err(OpError::IsRoot));
    }

    #[test]
    fn moving_reorders_and_reparents_and_refuses_cycles() {
        let (mut w, vbox, a, b, button) = sample();
        w.move_node(b, vbox, Some(0)).unwrap();
        let order: Vec<_> = w.node(vbox).unwrap().children.iter().map(|c| c.id).collect();
        assert_eq!(order, [b, a, button]);
        // Moving to the end after itself accounts for its own removal.
        w.move_node(b, vbox, Some(3)).unwrap();
        let order: Vec<_> = w.node(vbox).unwrap().children.iter().map(|c| c.id).collect();
        assert_eq!(order, [a, button, b]);
        // Into a button, then it can't be taken again.
        w.move_node(a, button, None).unwrap();
        assert_eq!(w.parent_of(a), Some(button));
        assert_eq!(w.move_node(b, button, None), Err(OpError::Full("Button")));
        assert_eq!(w.move_node(vbox, button, None), Err(OpError::IntoSelf));
        assert_eq!(w.move_node(vbox, vbox, None), Err(OpError::IntoSelf));
    }

    #[test]
    fn moving_between_parents_drops_slot_properties_that_no_longer_apply() {
        let (mut w, vbox, a, _, _) = sample();
        let canvas = w.root.id;
        w.node_mut(a).unwrap().set_slot(SlotKind::Box, "padding", json!([4.0, 4.0, 4.0, 4.0]));
        w.node_mut(a).unwrap().set_slot(SlotKind::Box, "fill_weight", json!(3.0));
        w.move_node(a, canvas, None).unwrap();
        let slot = &w.node(a).unwrap().slot;
        assert!(slot.get("fill_weight").is_none(), "box-only property is gone");
        assert!(slot.get("padding").is_none(), "canvas slots have no padding");
        w.move_node(a, vbox, None).unwrap();
        assert!(w.node(a).unwrap().slot.is_empty());
    }

    #[test]
    fn duplicates_get_fresh_ids_and_names_and_sit_after_the_original() {
        let (mut w, vbox, a, b, _) = sample();
        let c = w.duplicate(a).unwrap();
        let kids = &w.node(vbox).unwrap().children;
        assert_eq!(kids[1].id, c);
        assert_ne!(c, a);
        assert_ne!(kids[1].name, kids[0].name);
        assert_ne!(c, b);
        // A subtree duplicates with every id renewed.
        let vcopy = w.duplicate(vbox).unwrap();
        let mut ids = HashSet::new();
        w.root.walk(&mut |n| assert!(ids.insert(n.id), "duplicate id {}", n.id));
        let mut names = HashSet::new();
        w.root.walk(&mut |n| assert!(names.insert(n.name.clone()), "duplicate name {}", n.name));
        assert_eq!(w.node(vcopy).unwrap().children.len(), 4);
    }

    #[test]
    fn wrapping_and_unwrapping_preserve_the_children() {
        let (mut w, vbox, a, b, button) = sample();
        let border = w.wrap(&[a], Kind::Border).unwrap();
        assert_eq!(w.parent_of(a), Some(border));
        assert_eq!(w.parent_of(border), Some(vbox));
        assert_eq!(w.index_in_parent(border), Some(0));

        let hbox = w.wrap(&[b, button], Kind::HorizontalBox).unwrap();
        let kids: Vec<_> = w.node(hbox).unwrap().children.iter().map(|c| c.id).collect();
        assert_eq!(kids, [b, button]);
        assert_eq!(w.wrap(&[a, b], Kind::Border), Err(OpError::Full("Border")));
        assert_eq!(w.wrap(&[a, hbox], Kind::VerticalBox), Err(OpError::NotSiblings));

        w.unwrap(hbox).unwrap();
        assert_eq!(w.parent_of(b), Some(vbox));
        assert_eq!(w.parent_of(button), Some(vbox));
        assert!(w.node(hbox).is_none());

        // The root can be wrapped; the old root becomes its child.
        let old_root = w.root.id;
        let new_root = w.wrap(&[old_root], Kind::Overlay).unwrap();
        assert_eq!(w.root.id, new_root);
        assert_eq!(w.root.children[0].id, old_root);
    }

    #[test]
    fn parameters_link_only_to_properties_of_their_type() {
        let (mut w, _, a, _, _) = sample();
        let text = w.add_parameter("Label", ValueClass::Text);
        let number = w.add_parameter("Size", ValueClass::Float);
        w.link_parameter(text, a, "text").unwrap();
        assert_eq!(w.link_parameter(text, a, "font_size"), Err(OpError::BadTarget));
        assert_eq!(w.link_parameter(text, a, "nope"), Err(OpError::BadTarget));
        w.link_parameter(number, a, "font_size").unwrap();
        assert_eq!(w.driver_of(a, "text").unwrap().id, text);
        // A property has one driver: linking it elsewhere takes it over.
        let other = w.add_parameter("Other", ValueClass::Text);
        w.link_parameter(other, a, "text").unwrap();
        assert_eq!(w.driver_of(a, "text").unwrap().id, other);
        assert!(w.parameter(text).unwrap().targets.is_empty());
        w.unlink_property(a, "text");
        assert!(w.driver_of(a, "text").is_none());
    }

    #[test]
    fn parameter_names_are_made_unique_and_ids_never_reused() {
        let mut w = WidgetAsset::new();
        let a = w.add_parameter("Title", ValueClass::Text);
        let b = w.add_parameter("Title", ValueClass::Text);
        assert_eq!(w.parameter(b).unwrap().name, "Title_1");
        w.remove_parameter(a);
        let c = w.add_parameter("Title", ValueClass::Text);
        assert_ne!(c, a, "a removed parameter's id is not handed out again");
    }

    #[test]
    fn removing_a_parameter_clears_its_pass_throughs() {
        let (mut w, vbox, ..) = sample();
        let p = w.add_parameter("Title", ValueClass::Text);
        let inst = w.new_instance(WidgetId::new(), "Card");
        let inst_id = inst.id;
        w.add(vbox, None, inst).unwrap();
        w.node_mut(inst_id).unwrap().instance.as_mut().unwrap().overrides.insert(ParamId(9), Arg::Bind(p));
        w.remove_parameter(p);
        assert!(w.node(inst_id).unwrap().instance.as_ref().unwrap().overrides.is_empty());
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("  Hello World! ", "X"), "Hello_World");
        assert_eq!(sanitize_name("***", "X"), "X");
    }
}
