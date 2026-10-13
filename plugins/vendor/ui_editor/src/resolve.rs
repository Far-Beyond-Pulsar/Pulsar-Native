//! Expanding a widget into the tree that is laid out and drawn.
//!
//! The stored tree has instances of other widgets by id and properties
//! driven by parameters. Resolving applies the parameter values, replaces each
//! instance with the widget it refers to (recursively, with that widget's
//! own parameters set from the instance's values), and fills named slots with
//! the content the instance supplies.
//!
//! Every resolved node remembers the node of the *edited* widget it belongs to
//! (its [`ResolvedNode::owner`]), so clicking inside an instance selects the
//! instance, and clicking slot content selects that content.

use std::collections::HashMap;

use serde_json::Value as Json;

use crate::model::geom::{Affine, Rect, Vec2};
use crate::model::node::{Arg, Props};
use crate::model::schema::{Kind, SlotKind};
use crate::model::validate::WidgetSource;
use crate::model::{NodeId, ParamId, WidgetAsset, WidgetId};

/// How deep instances can nest before resolving stops (a guard against a
/// pathological library; real cycles are caught separately).
pub const MAX_INSTANCE_DEPTH: usize = 16;

/// What became of an instance when resolving.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceStatus {
    Ok,
    /// No widget has this id.
    Missing { hint: String },
    /// More than one file claims the id.
    Ambiguous { hint: String },
    /// The widget would contain itself.
    Cycle { hint: String },
    TooDeep,
}

#[derive(Clone, Debug)]
pub struct ResolvedNode {
    /// The node of the edited widget this belongs to: itself for the widget's
    /// own nodes, the instance for anything inside one.
    pub owner: NodeId,
    /// Unique among resolved nodes: the chain of node ids from the edited
    /// widget down.
    pub key: Vec<NodeId>,
    pub name: String,
    pub kind: Kind,
    /// Properties with parameter values applied.
    pub props: Props,
    pub slot: Props,
    /// Which slot properties `slot` holds (the parent's).
    pub slot_kind: SlotKind,
    pub children: Vec<ResolvedNode>,
    /// Hidden in the designer only.
    pub design_hidden: bool,
    /// Part of the edited widget itself (not inside an instance).
    pub own: bool,
    /// Present on instance nodes.
    pub status: Option<InstanceStatus>,
    /// A named slot nothing fills.
    pub is_empty_slot: bool,
    /// Size the node wants, set by layout.
    pub desired: Vec2,
    /// Where layout put it, in its parent's space.
    pub rect: Rect,
    /// Its own render transform (and a scale box's scaling), in parent space.
    pub local: Affine,
    /// Parent space to widget space, set after layout.
    pub world: Affine,
}

impl ResolvedNode {
    pub fn view(&self) -> crate::model::node::View<'_> {
        crate::model::node::View { props: &self.props, table: crate::model::node::Table::Kind(self.kind.info()) }
    }

    pub fn slot_view(&self) -> crate::model::node::View<'_> {
        crate::model::node::View { props: &self.slot, table: crate::model::node::Table::Slot(self.slot_kind) }
    }

    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a ResolvedNode)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }
}

type Env = HashMap<ParamId, Json>;

struct Ctx<'a> {
    lib: &'a dyn WidgetSource,
    /// Widgets being expanded, outermost first.
    stack: Vec<WidgetId>,
}

/// The values every parameter of `widget` has when nothing sets them.
pub fn default_env(widget: &WidgetAsset) -> Env {
    widget.parameters.iter().map(|p| (p.id, p.default.clone())).collect()
}

/// Resolve `widget` for display, with its parameters at their defaults.
pub fn resolve(widget: &WidgetAsset, lib: &dyn WidgetSource) -> ResolvedNode {
    resolve_with(widget, &default_env(widget), lib)
}

/// Resolve `widget` with the given parameter values.
pub fn resolve_with(widget: &WidgetAsset, env: &Env, lib: &dyn WidgetSource) -> ResolvedNode {
    let mut ctx = Ctx { lib, stack: vec![widget.id.clone()] };
    let mut tree = widget.root.clone();
    apply_parameters(widget, env, &mut tree);
    expand(&tree, widget, env, &mut ctx, SlotKind::None, &Frame::top(widget.root.id))
}

fn apply_parameters(widget: &WidgetAsset, env: &Env, tree: &mut crate::model::Node) {
    for param in &widget.parameters {
        let value = env.get(&param.id).unwrap_or(&param.default);
        for target in &param.targets {
            let Some(node) = tree.find_mut(target.node) else { continue };
            let Some(def) = node.info().def(&target.prop) else { continue };
            if let Some(valid) = def.ty.coerce(value) {
                node.props.0.insert(target.prop.clone(), valid);
            }
        }
    }
}

/// Where in the nesting a node is being expanded.
struct Frame {
    /// The edited widget's node everything here belongs to; `None` at the top
    /// level, where each node owns itself.
    owner: Option<NodeId>,
    /// Chain of node ids leading to this level.
    prefix: Vec<NodeId>,
    /// Slot content supplied by the instance one level up, by slot name.
    fills: HashMap<String, Vec<ResolvedNode>>,
}

impl Frame {
    fn top(_root: NodeId) -> Self {
        Frame { owner: None, prefix: Vec::new(), fills: HashMap::new() }
    }
}

fn expand(node: &crate::model::Node, widget: &WidgetAsset, env: &Env, ctx: &mut Ctx, slot_kind: SlotKind, frame: &Frame) -> ResolvedNode {
    let owner = frame.owner.unwrap_or(node.id);
    let mut key = frame.prefix.clone();
    key.push(node.id);
    let mut out = ResolvedNode {
        owner,
        key: key.clone(),
        name: node.name.clone(),
        kind: node.kind,
        props: node.props.clone(),
        slot: node.slot.clone(),
        slot_kind,
        children: Vec::new(),
        design_hidden: frame.owner.is_none() && node.editor.hidden,
        own: frame.owner.is_none(),
        status: None,
        is_empty_slot: false,
        desired: Vec2::ZERO,
        rect: Rect::default(),
        local: Affine::IDENTITY,
        world: Affine::IDENTITY,
    };

    match node.kind {
        Kind::Instance => expand_instance(node, widget, env, ctx, frame, &key, &mut out),
        Kind::NamedSlot => {
            // Content supplied by an enclosing instance, else an empty hole.
            match frame.fills.get(&node.name) {
                Some(content) if !content.is_empty() => out.children = content.clone(),
                _ => out.is_empty_slot = true,
            }
        }
        _ => {
            let child_slot = node.info().slot;
            let inner = Frame { owner: frame.owner, prefix: key, fills: frame.fills.clone() };
            out.children = node.children.iter().map(|c| expand(c, widget, env, ctx, child_slot, &inner)).collect();
        }
    }
    out
}

fn expand_instance(
    node: &crate::model::Node,
    widget: &WidgetAsset,
    env: &Env,
    ctx: &mut Ctx,
    frame: &Frame,
    key: &[NodeId],
    out: &mut ResolvedNode,
) {
    let Some(instance) = &node.instance else {
        out.status = Some(InstanceStatus::Missing { hint: String::new() });
        return;
    };
    let hint = if instance.name_hint.is_empty() { String::new() } else { instance.name_hint.clone() };
    let target = if ctx.lib.is_ambiguous(&instance.widget) {
        out.status = Some(InstanceStatus::Ambiguous { hint });
        return;
    } else if ctx.stack.contains(&instance.widget) {
        out.status = Some(InstanceStatus::Cycle { hint });
        return;
    } else if ctx.stack.len() >= MAX_INSTANCE_DEPTH {
        out.status = Some(InstanceStatus::TooDeep);
        return;
    } else {
        match ctx.lib.widget(&instance.widget) {
            Some(target) => target,
            None => {
                out.status = Some(InstanceStatus::Missing { hint });
                return;
            }
        }
    };
    out.status = Some(InstanceStatus::Ok);

    // The target's parameter values: what the instance sets, else defaults.
    let mut inner_env = Env::new();
    for param in &target.parameters {
        let value = match instance.overrides.get(&param.id) {
            Some(Arg::Value(v)) => param.class.coerce(v),
            Some(Arg::Bind(outer)) => {
                let outer_value = env.get(outer).or_else(|| widget.parameter(*outer).map(|p| &p.default));
                outer_value.and_then(|v| param.class.coerce(v))
            }
            None => None,
        };
        inner_env.insert(param.id, value.unwrap_or_else(|| param.default.clone()));
    }

    // Content for the target's named slots is resolved in this widget's context.
    let content_frame = Frame { owner: frame.owner, prefix: key.to_vec(), fills: frame.fills.clone() };
    let mut fills: HashMap<String, Vec<ResolvedNode>> = HashMap::new();
    for child in &node.children {
        let Some(slot) = &child.for_slot else { continue };
        let mut resolved = expand(child, widget, env, ctx, SlotKind::Single, &content_frame);
        // Content of an instance is selectable as itself, even inside another instance.
        if frame.owner.is_none() {
            resolved.own = true;
        }
        fills.entry(slot.clone()).or_default().push(resolved);
    }

    let mut tree = target.root.clone();
    apply_parameters(&target, &inner_env, &mut tree);
    ctx.stack.push(target.id.clone());
    let inner = Frame { owner: Some(frame.owner.unwrap_or(node.id)), prefix: key.to_vec(), fills };
    let root = expand(&tree, &target, &inner_env, ctx, SlotKind::Single, &inner);
    ctx.stack.pop();
    out.children.push(root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::schema::ValueClass;
    use crate::model::validate::NoWidgets;
    use serde_json::json;
    use std::sync::Arc;

    type Lib = HashMap<WidgetId, Arc<WidgetAsset>>;

    fn lib_of(widgets: &[&WidgetAsset]) -> Lib {
        widgets.iter().map(|w| (w.id.clone(), Arc::new((*w).clone()))).collect()
    }

    /// A card: a border holding a text driven by the `Title` parameter, and a
    /// named slot called `Body`.
    fn card() -> (WidgetAsset, ParamId) {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let v = w.new_node(Kind::VerticalBox);
        let v_id = v.id;
        w.add(root, None, v).unwrap();
        let mut text = w.new_node(Kind::Text);
        text.set_prop("text", json!("Default title"));
        let text_id = text.id;
        w.add(v_id, None, text).unwrap();
        let mut slot = w.new_node(Kind::NamedSlot);
        slot.name = "Body".into();
        w.add(v_id, None, slot).unwrap();
        let p = w.add_parameter("Title", ValueClass::Text);
        w.link_parameter(p, text_id, "text").unwrap();
        w.adopt_value(p, json!("Default title"));
        (w, p)
    }

    fn find<'a>(tree: &'a ResolvedNode, name: &str) -> &'a ResolvedNode {
        let mut found = None;
        tree.walk(&mut |n| {
            if n.name == name && found.is_none() {
                found = Some(n);
            }
        });
        found.unwrap_or_else(|| panic!("no node named {name}"))
    }

    #[test]
    fn a_widget_resolves_to_its_own_tree_with_parameter_defaults_applied() {
        let (mut card, p) = card();
        card.parameters[0].default = json!("From default");
        let tree = resolve(&card, &NoWidgets);
        assert_eq!(find(&tree, "Text").view().text("text"), "From default");
        assert!(tree.own);
        assert_eq!(p, card.parameters[0].id);
    }

    #[test]
    fn an_instance_expands_the_referenced_widget_with_its_values() {
        let (card, p) = card();
        let mut host = WidgetAsset::new();
        let mut inst = host.new_instance(card.id.clone(), "Card");
        inst.instance.as_mut().unwrap().overrides.insert(p, Arg::Value(json!("Sword")));
        let inst_id = inst.id;
        let root = host.root.id;
        host.add(root, None, inst).unwrap();

        let lib = lib_of(&[&card]);
        let tree = resolve(&host, &lib);
        let text = find(&tree, "Text");
        assert_eq!(text.view().text("text"), "Sword");
        assert_eq!(text.owner, inst_id, "clicking inside an instance selects the instance");
        assert!(!text.own);
        assert_eq!(find(&tree, "Card").status, Some(InstanceStatus::Ok));
    }

    #[test]
    fn bound_values_pass_the_outer_parameter_through() {
        let (card, card_title) = card();
        let mut menu = WidgetAsset::new();
        let heading = menu.add_parameter("Heading", ValueClass::Text);
        menu.parameters[0].default = json!("Menu default");
        let mut inst = menu.new_instance(card.id.clone(), "Card");
        inst.instance.as_mut().unwrap().overrides.insert(card_title, Arg::Bind(heading));
        let root = menu.root.id;
        menu.add(root, None, inst).unwrap();

        let lib = lib_of(&[&card]);
        assert_eq!(find(&resolve(&menu, &lib), "Text").view().text("text"), "Menu default");

        // Someone instancing the menu sets Heading; it reaches the card's text.
        let mut screen = WidgetAsset::new();
        let mut m = screen.new_instance(menu.id.clone(), "Menu");
        m.instance.as_mut().unwrap().overrides.insert(heading, Arg::Value(json!("Inventory")));
        let root = screen.root.id;
        screen.add(root, None, m).unwrap();
        let lib = lib_of(&[&card, &menu]);
        assert_eq!(find(&resolve(&screen, &lib), "Text").view().text("text"), "Inventory");
    }

    #[test]
    fn named_slots_are_filled_with_the_instances_children() {
        let (card, _) = card();
        let mut host = WidgetAsset::new();
        let inst = host.new_instance(card.id.clone(), "Card");
        let inst_id = inst.id;
        let root = host.root.id;
        host.add(root, None, inst).unwrap();
        let mut content = host.new_node(Kind::Button);
        content.name = "Buy".into();
        content.for_slot = Some("Body".into());
        let content_id = content.id;
        host.add(inst_id, None, content).unwrap();

        let lib = lib_of(&[&card]);
        let tree = resolve(&host, &lib);
        let buy = find(&tree, "Buy");
        assert_eq!(buy.owner, content_id, "slot content is selectable as itself");
        assert!(buy.own);
        let slot = find(&tree, "Body");
        assert!(!slot.is_empty_slot);
        assert_eq!(slot.children.len(), 1);

        // Without content the slot is an empty hole.
        let mut bare = WidgetAsset::new();
        let inst = bare.new_instance(card.id.clone(), "Card");
        let root = bare.root.id;
        bare.add(root, None, inst).unwrap();
        assert!(find(&resolve(&bare, &lib), "Body").is_empty_slot);
    }

    #[test]
    fn missing_ambiguous_and_cyclic_instances_become_placeholders() {
        let mut host = WidgetAsset::new();
        let ghost = host.new_instance(WidgetId::new(), "Ghost");
        let root = host.root.id;
        host.add(root, None, ghost).unwrap();
        let tree = resolve(&host, &NoWidgets);
        assert!(matches!(find(&tree, "Ghost").status, Some(InstanceStatus::Missing { ref hint }) if hint == "Ghost"));

        // a contains b contains a.
        let mut a = WidgetAsset::new();
        let mut b = WidgetAsset::new();
        let to_b = a.new_instance(b.id.clone(), "B");
        let r = a.root.id;
        a.add(r, None, to_b).unwrap();
        let to_a = b.new_instance(a.id.clone(), "A");
        let r = b.root.id;
        b.add(r, None, to_a).unwrap();
        let lib = lib_of(&[&a, &b]);
        let tree = resolve(&a, &lib);
        let mut cycles = 0;
        tree.walk(&mut |n| {
            if matches!(n.status, Some(InstanceStatus::Cycle { .. })) {
                cycles += 1;
            }
        });
        assert_eq!(cycles, 1, "the loop is cut where it closes");
    }

    #[test]
    fn invalid_values_fall_back_to_the_parameters_default() {
        let (card, p) = card();
        let mut host = WidgetAsset::new();
        let mut inst = host.new_instance(card.id.clone(), "Card");
        inst.instance.as_mut().unwrap().overrides.insert(p, Arg::Value(json!(42)));
        let root = host.root.id;
        host.add(root, None, inst).unwrap();
        let lib = lib_of(&[&card]);
        assert_eq!(find(&resolve(&host, &lib), "Text").view().text("text"), "Default title");
    }

    #[test]
    fn keys_are_unique_across_repeated_instances() {
        let (card, _) = card();
        let mut host = WidgetAsset::new();
        for _ in 0..3 {
            let inst = host.new_instance(card.id.clone(), "Card");
            let root = host.root.id;
            host.add(root, None, inst).unwrap();
        }
        let lib = lib_of(&[&card]);
        let tree = resolve(&host, &lib);
        let mut keys = std::collections::HashSet::new();
        tree.walk(&mut |n| assert!(keys.insert(n.key.clone()), "duplicate key {:?}", n.key));
    }
}
