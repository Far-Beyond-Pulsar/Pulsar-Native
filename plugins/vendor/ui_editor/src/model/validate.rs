//! Checking a widget for problems, and the one-click repairs for the ones
//! that have an obvious fix.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::id::{NodeId, ParamId, WidgetId};
use super::node::{Arg, Node};
use super::schema::{Children, Kind, SlotKind};
use super::widget::WidgetAsset;
use super::{Issue, Severity};

/// Where other widgets come from: the project's library in the editor, a
/// loaded set in tests or at runtime.
pub trait WidgetSource {
    /// The widget with this id, if it exists and is unambiguous.
    fn widget(&self, id: &WidgetId) -> Option<Arc<WidgetAsset>>;

    /// What the widget is called (its file name).
    fn name(&self, id: &WidgetId) -> Option<String>;

    /// Whether more than one file claims this id.
    fn is_ambiguous(&self, _id: &WidgetId) -> bool {
        false
    }
}

/// A project with no other widgets.
pub struct NoWidgets;

impl WidgetSource for NoWidgets {
    fn widget(&self, _: &WidgetId) -> Option<Arc<WidgetAsset>> {
        None
    }

    fn name(&self, _: &WidgetId) -> Option<String> {
        None
    }
}

impl WidgetSource for HashMap<WidgetId, Arc<WidgetAsset>> {
    fn widget(&self, id: &WidgetId) -> Option<Arc<WidgetAsset>> {
        self.get(id).cloned()
    }

    fn name(&self, id: &WidgetId) -> Option<String> {
        self.get(id).map(|_| id.short().to_string())
    }
}

/// How deep a tree can nest before it is worth a warning.
pub const MAX_COMFORTABLE_DEPTH: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetFix {
    RenameNode { node: NodeId, name: String },
    RenameParameter { param: ParamId, name: String },
    RemoveTarget { param: ParamId, node: NodeId, prop: String },
    RemoveOverride { node: NodeId, param: ParamId },
    ResetParameterDefault { param: ParamId },
    AssignSlot { node: NodeId, slot: Option<String> },
    UpdateNameHint { node: NodeId, name: String },
    /// Needs the library; handled by the editor.
    NewWidgetId,
}

impl WidgetFix {
    pub fn label(&self) -> String {
        match self {
            WidgetFix::RenameNode { name, .. } | WidgetFix::RenameParameter { name, .. } => format!("Rename to {name}"),
            WidgetFix::RemoveTarget { .. } => "Unlink".to_string(),
            WidgetFix::RemoveOverride { .. } => "Remove value".to_string(),
            WidgetFix::ResetParameterDefault { .. } => "Reset default".to_string(),
            WidgetFix::AssignSlot { slot: Some(s), .. } => format!("Fill slot {s}"),
            WidgetFix::AssignSlot { slot: None, .. } => "Unassign".to_string(),
            WidgetFix::UpdateNameHint { .. } => "Update name".to_string(),
            WidgetFix::NewWidgetId => "Give this widget a new id".to_string(),
        }
    }
}

/// Whether adding an instance of `candidate` to the widget `this` would make
/// widgets contain each other.
pub fn would_cycle(lib: &dyn WidgetSource, this: &WidgetId, candidate: &WidgetId) -> bool {
    if candidate == this {
        return true;
    }
    let mut seen = HashSet::new();
    let mut stack = vec![candidate.clone()];
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(widget) = lib.widget(&id) else { continue };
        for dep in widget.dependencies() {
            if &dep == this {
                return true;
            }
            stack.push(dep);
        }
    }
    false
}

impl WidgetAsset {
    /// The widgets this one instances, without repeats.
    pub fn dependencies(&self) -> Vec<WidgetId> {
        let mut out = Vec::new();
        self.root.walk(&mut |n| {
            if let Some(instance) = &n.instance {
                if !out.contains(&instance.widget) {
                    out.push(instance.widget.clone());
                }
            }
        });
        out
    }

    pub fn validate(&self, lib: &dyn WidgetSource) -> Vec<Issue<WidgetFix>> {
        let mut issues = Vec::new();
        self.check_names(&mut issues);
        self.check_structure(&mut issues);
        self.check_parameters(&mut issues);
        self.check_instances(lib, &mut issues);
        issues.sort_by_key(|i| std::cmp::Reverse(i.severity));
        issues
    }

    fn check_names(&self, issues: &mut Vec<Issue<WidgetFix>>) {
        let mut seen: HashMap<&str, NodeId> = HashMap::new();
        let mut dupes = Vec::new();
        self.root.walk(&mut |n| {
            if seen.insert(n.name.as_str(), n.id).is_some() {
                dupes.push((n.id, n.name.clone()));
            }
        });
        for (id, name) in dupes {
            let better = self.unique_name(&name);
            issues.push(
                Issue::new(Severity::Warning, name.clone(), format!("Another widget is also called `{name}`; names should be unique"))
                    .on(id)
                    .with_fix(WidgetFix::RenameNode { node: id, name: better }),
            );
        }
        let mut slot_names = HashSet::new();
        self.root.walk(&mut |n| {
            if n.kind == Kind::NamedSlot && !slot_names.insert(n.name.clone()) {
                issues.push(
                    Issue::new(Severity::Error, n.name.clone(), "Two named slots share a name, so users of this widget can't tell them apart")
                        .on(n.id),
                );
            }
        });
    }

    fn check_structure(&self, issues: &mut Vec<Issue<WidgetFix>>) {
        let depth = self.root.depth();
        if depth > MAX_COMFORTABLE_DEPTH {
            issues.push(Issue::new(Severity::Warning, self.root.name.clone(), format!("The tree is {depth} levels deep; deep nesting is slow to lay out")));
        }
        fn visit(node: &Node, parent: Option<&Node>, issues: &mut Vec<Issue<WidgetFix>>) {
            let info = node.info();
            if info.children == Children::One && node.children.len() > 1 {
                issues.push(
                    Issue::new(Severity::Error, node.name.clone(), format!("{} holds one widget but has {}", info.label, node.children.len())).on(node.id),
                );
            }
            if info.children == Children::None && !node.children.is_empty() {
                issues.push(Issue::new(Severity::Error, node.name.clone(), format!("{} can't hold widgets", info.label)).on(node.id));
            }
            match node.kind {
                Kind::Text if node.view().text("text").is_empty() => {
                    issues.push(Issue::new(Severity::Info, node.name.clone(), "The text is empty").on(node.id));
                }
                Kind::Image if node.view().image("source").is_empty() => {
                    issues.push(Issue::new(Severity::Info, node.name.clone(), "No image is chosen").on(node.id));
                }
                Kind::Unknown => {
                    issues.push(Issue::new(Severity::Warning, node.name.clone(), "Made by a newer version; it is kept but can't be shown").on(node.id));
                }
                _ => {}
            }
            if let Some(parent) = parent {
                if parent.info().slot == SlotKind::Canvas && !node.slot_view(SlotKind::Canvas).bool("auto_size") {
                    let v = node.slot_view(SlotKind::Canvas);
                    let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
                    let o = v.quad("offsets");
                    let wide = amin.x != amax.x || o[2] > 0.0;
                    let tall = amin.y != amax.y || o[3] > 0.0;
                    if !wide || !tall {
                        issues.push(Issue::new(Severity::Warning, node.name.clone(), "It has no size on the canvas, so it can't be seen").on(node.id));
                    }
                }
            }
            for child in &node.children {
                visit(child, Some(node), issues);
            }
        }
        visit(&self.root, None, issues);
    }

    fn check_parameters(&self, issues: &mut Vec<Issue<WidgetFix>>) {
        let mut seen = HashSet::new();
        for p in &self.parameters {
            if p.name.trim().is_empty() {
                issues.push(Issue::new(Severity::Error, format!("Parameter #{}", p.id.0), "A parameter has no name"));
            } else if !seen.insert(p.name.as_str()) {
                let taken: HashSet<String> = self.parameters.iter().map(|q| q.name.clone()).collect();
                let mut n = 1;
                let better = loop {
                    let c = format!("{}_{n}", p.name);
                    if !taken.contains(&c) {
                        break c;
                    }
                    n += 1;
                };
                issues.push(
                    Issue::new(Severity::Error, p.name.clone(), "Two parameters share this name")
                        .with_fix(WidgetFix::RenameParameter { param: p.id, name: better }),
                );
            }
            if p.class.coerce(&p.default).is_none() {
                issues.push(
                    Issue::new(Severity::Warning, p.name.clone(), "The default value isn't a valid value for the parameter's type")
                        .with_fix(WidgetFix::ResetParameterDefault { param: p.id }),
                );
            }
            for t in &p.targets {
                let problem = match self.node(t.node) {
                    None => Some("drives a widget that no longer exists".to_string()),
                    Some(n) => match n.info().def(&t.prop) {
                        None => Some(format!("drives `{}` on {}, which has no such property", t.prop, n.name)),
                        Some(d) if d.ty.class() != p.class => Some(format!("drives `{}` on {}, which is a different type", d.label, n.name)),
                        Some(_) => None,
                    },
                };
                if let Some(problem) = problem {
                    issues.push(
                        Issue::new(Severity::Error, p.name.clone(), format!("This parameter {problem}"))
                            .with_fix(WidgetFix::RemoveTarget { param: p.id, node: t.node, prop: t.prop.clone() }),
                    );
                }
            }
        }
    }

    fn check_instances(&self, lib: &dyn WidgetSource, issues: &mut Vec<Issue<WidgetFix>>) {
        let mut nodes = Vec::new();
        self.root.walk(&mut |n| {
            if n.instance.is_some() {
                nodes.push(n);
            }
        });
        for node in nodes {
            let instance = node.instance.as_ref().expect("filtered");
            let subject = node.name.clone();
            if lib.is_ambiguous(&instance.widget) {
                issues.push(
                    Issue::new(Severity::Error, subject.clone(), "More than one file claims the id of the widget this refers to, so it can't tell which one is meant")
                        .on(node.id),
                );
                continue;
            }
            let Some(target) = lib.widget(&instance.widget) else {
                let hint = if instance.name_hint.is_empty() { String::new() } else { format!(" (it was `{}`)", instance.name_hint) };
                issues.push(
                    Issue::new(Severity::Error, subject, format!("The widget this refers to can't be found{hint}; it may have been deleted"))
                        .on(node.id),
                );
                continue;
            };
            if let Some(name) = lib.name(&instance.widget) {
                if name != instance.name_hint {
                    issues.push(
                        Issue::new(Severity::Info, subject.clone(), format!("The widget is now called `{name}`"))
                            .on(node.id)
                            .with_fix(WidgetFix::UpdateNameHint { node: node.id, name }),
                    );
                }
            }
            if would_cycle(lib, &self.id, &instance.widget) {
                issues.push(
                    Issue::new(Severity::Error, subject.clone(), "This would put the widget inside itself, directly or through other widgets")
                        .on(node.id),
                );
                continue;
            }
            for (param, arg) in &instance.overrides {
                match target.parameter(*param) {
                    None => issues.push(
                        Issue::new(Severity::Warning, subject.clone(), "A value is set for a parameter the widget no longer has")
                            .on(node.id)
                            .with_fix(WidgetFix::RemoveOverride { node: node.id, param: *param }),
                    ),
                    Some(p) => {
                        let bad = match arg {
                            Arg::Value(v) => p.class.coerce(v).is_none(),
                            Arg::Bind(outer) => self.parameter(*outer).is_none_or(|o| o.class != p.class),
                        };
                        if bad {
                            issues.push(
                                Issue::new(Severity::Warning, subject.clone(), format!("The value for `{}` doesn't match its type", p.name))
                                    .on(node.id)
                                    .with_fix(WidgetFix::RemoveOverride { node: node.id, param: *param }),
                            );
                        }
                    }
                }
            }
            let slots = target.slot_names();
            for child in &node.children {
                match &child.for_slot {
                    None => issues.push(
                        Issue::new(Severity::Warning, child.name.clone(), "It is placed in a widget but isn't assigned to any of its slots, so it isn't shown")
                            .on(child.id)
                            .with_fix(WidgetFix::AssignSlot { node: child.id, slot: slots.first().cloned() })
                            .filter_fix(!slots.is_empty()),
                    ),
                    Some(s) if !slots.contains(s) => issues.push(
                        Issue::new(Severity::Warning, child.name.clone(), format!("The widget has no slot called `{s}`"))
                            .on(child.id)
                            .with_fix(WidgetFix::AssignSlot { node: child.id, slot: slots.first().cloned() })
                            .filter_fix(!slots.is_empty()),
                    ),
                    Some(_) => {}
                }
            }
        }
    }

    /// Carry out `fix`. Fixes that need the library do nothing here.
    pub fn apply_fix(&mut self, fix: &WidgetFix) {
        match fix {
            WidgetFix::RenameNode { node, name } => {
                if let Some(n) = self.node_mut(*node) {
                    n.name = name.clone();
                }
            }
            WidgetFix::RenameParameter { param, name } => {
                if let Some(p) = self.parameters.iter_mut().find(|p| p.id == *param) {
                    p.name = name.clone();
                }
            }
            WidgetFix::RemoveTarget { param, node, prop } => {
                if let Some(p) = self.parameters.iter_mut().find(|p| p.id == *param) {
                    p.targets.retain(|t| !(t.node == *node && &t.prop == prop));
                }
            }
            WidgetFix::RemoveOverride { node, param } => {
                if let Some(instance) = self.node_mut(*node).and_then(|n| n.instance.as_mut()) {
                    instance.overrides.remove(param);
                }
            }
            WidgetFix::ResetParameterDefault { param } => {
                if let Some(p) = self.parameters.iter_mut().find(|p| p.id == *param) {
                    p.default = p.class.default_value();
                }
            }
            WidgetFix::AssignSlot { node, slot } => {
                if let Some(n) = self.node_mut(*node) {
                    n.for_slot = slot.clone();
                }
            }
            WidgetFix::UpdateNameHint { node, name } => {
                if let Some(instance) = self.node_mut(*node).and_then(|n| n.instance.as_mut()) {
                    instance.name_hint = name.clone();
                }
            }
            WidgetFix::NewWidgetId => self.regenerate_id(),
        }
    }
}

impl<F> Issue<F> {
    /// Keep the fix only if `keep`.
    fn filter_fix(mut self, keep: bool) -> Self {
        if !keep {
            self.fix = None;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::schema::ValueClass;
    use serde_json::json;

    fn library(widgets: &[&WidgetAsset]) -> HashMap<WidgetId, Arc<WidgetAsset>> {
        widgets.iter().map(|w| (w.id.clone(), Arc::new((*w).clone()))).collect()
    }

    fn with_text(w: &mut WidgetAsset) -> NodeId {
        let node = w.new_node(Kind::Text);
        let id = node.id;
        let root = w.root.id;
        w.add(root, None, node).unwrap();
        // Give it a size so the canvas check is quiet.
        id
    }

    #[test]
    fn a_fresh_widget_is_clean() {
        assert!(WidgetAsset::new().validate(&NoWidgets).is_empty());
    }

    #[test]
    fn duplicate_names_offer_a_rename() {
        let mut w = WidgetAsset::new();
        let a = with_text(&mut w);
        let b = with_text(&mut w);
        w.rename(b, "Text").unwrap();
        let issues = w.validate(&NoWidgets);
        let dup = issues.iter().find(|i| i.message.contains("also called")).expect("reported");
        assert_eq!(dup.node, Some(b));
        w.apply_fix(dup.fix.as_ref().unwrap());
        assert!(w.validate(&NoWidgets).iter().all(|i| !i.message.contains("also called")));
        assert_ne!(w.node(a).unwrap().name, w.node(b).unwrap().name);
    }

    #[test]
    fn broken_parameter_links_are_reported_and_repairable() {
        let mut w = WidgetAsset::new();
        let t = with_text(&mut w);
        let p = w.add_parameter("Label", ValueClass::Text);
        w.link_parameter(p, t, "text").unwrap();
        assert!(w.validate(&NoWidgets).iter().all(|i| i.severity != Severity::Error));
        // Break it from outside the editing operations, as a hand-edit would.
        w.parameters[0].targets.push(crate::model::widget::Target { node: t, prop: "font_size".into() });
        let issues = w.validate(&NoWidgets);
        let broken = issues.iter().find(|i| i.severity == Severity::Error).expect("type mismatch reported");
        w.apply_fix(broken.fix.as_ref().unwrap());
        assert!(w.validate(&NoWidgets).iter().all(|i| i.severity != Severity::Error));
    }

    #[test]
    fn a_missing_widget_is_an_error_that_names_what_it_was() {
        let mut w = WidgetAsset::new();
        let inst = w.new_instance(WidgetId::new(), "ItemCard");
        let root = w.root.id;
        w.add(root, None, inst).unwrap();
        let issues = w.validate(&NoWidgets);
        let missing = issues.iter().find(|i| i.severity == Severity::Error).unwrap();
        assert!(missing.message.contains("ItemCard"), "{}", missing.message);
    }

    #[test]
    fn cycles_are_detected_directly_and_through_other_widgets() {
        let a = WidgetAsset::new();
        let mut b = WidgetAsset::new();
        let mut c = WidgetAsset::new();
        // b instances a; c instances b.
        let inst = b.new_instance(a.id.clone(), "A");
        let root = b.root.id;
        b.add(root, None, inst).unwrap();
        let inst = c.new_instance(b.id.clone(), "B");
        let root = c.root.id;
        c.add(root, None, inst).unwrap();
        let lib = library(&[&a, &b, &c]);

        assert!(would_cycle(&lib, &a.id, &a.id), "a widget can't hold itself");
        assert!(would_cycle(&lib, &a.id, &c.id), "a would hold c, which holds b, which holds a");
        assert!(!would_cycle(&lib, &c.id, &a.id), "c holding a is fine");

        // A file edited by hand to close the loop is reported.
        let mut a2 = a.clone();
        let inst = a2.new_instance(c.id.clone(), "C");
        let root = a2.root.id;
        a2.add(root, None, inst).unwrap();
        let lib = library(&[&a2, &b, &c]);
        assert!(a2.validate(&lib).iter().any(|i| i.message.contains("inside itself")));
    }

    #[test]
    fn overrides_are_checked_against_the_target_widgets_parameters() {
        let mut card = WidgetAsset::new();
        let title = card.add_parameter("Title", ValueClass::Text);
        let mut host = WidgetAsset::new();
        let inst = host.new_instance(card.id.clone(), "Card");
        let inst_id = inst.id;
        let root = host.root.id;
        host.add(root, None, inst).unwrap();
        let lib = library(&[&card, &host]);

        fn overrides(host: &mut WidgetAsset, id: NodeId) -> &mut std::collections::BTreeMap<ParamId, Arg> {
            &mut host.node_mut(id).unwrap().instance.as_mut().unwrap().overrides
        }
        overrides(&mut host, inst_id).insert(title, Arg::Value(json!("Hello")));
        assert!(host.validate(&lib).iter().all(|i| i.severity != Severity::Warning && i.severity != Severity::Error));

        overrides(&mut host, inst_id).insert(title, Arg::Value(json!(5)));
        let issues = host.validate(&lib);
        let wrong = issues.iter().find(|i| i.message.contains("doesn't match")).expect("type mismatch");
        host.apply_fix(wrong.fix.as_ref().unwrap());

        overrides(&mut host, inst_id).insert(ParamId(77), Arg::Value(json!("x")));
        let issues = host.validate(&lib);
        assert!(issues.iter().any(|i| i.message.contains("no longer has")));
    }

    #[test]
    fn slot_children_must_name_a_slot_the_widget_has() {
        let mut frame = WidgetAsset::new();
        let mut slot = frame.new_node(Kind::NamedSlot);
        slot.name = "Content".into();
        let root = frame.root.id;
        frame.add(root, None, slot).unwrap();

        let mut host = WidgetAsset::new();
        let inst = host.new_instance(frame.id.clone(), "Frame");
        let inst_id = inst.id;
        let root = host.root.id;
        host.add(root, None, inst).unwrap();
        let mut child = host.new_node(Kind::Text);
        child.for_slot = Some("Content".into());
        let child_id = child.id;
        host.add(inst_id, None, child).unwrap();
        let lib = library(&[&frame, &host]);
        assert!(host.validate(&lib).iter().all(|i| i.severity < Severity::Warning), "{:?}", host.validate(&lib));

        host.node_mut(child_id).unwrap().for_slot = Some("Header".into());
        let issues = host.validate(&lib);
        let bad = issues.iter().find(|i| i.message.contains("no slot")).expect("reported");
        host.apply_fix(bad.fix.as_ref().unwrap());
        assert_eq!(host.node(child_id).unwrap().for_slot.as_deref(), Some("Content"));
    }

    #[test]
    fn dependencies_list_each_instanced_widget_once() {
        let mut w = WidgetAsset::new();
        let other = WidgetId::new();
        for _ in 0..3 {
            let inst = w.new_instance(other.clone(), "X");
            let root = w.root.id;
            w.add(root, None, inst).unwrap();
        }
        assert_eq!(w.dependencies(), vec![other]);
    }
}
