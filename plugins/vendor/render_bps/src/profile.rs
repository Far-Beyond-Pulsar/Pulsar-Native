//! What the shared Graphy canvas needs to know about render graphs: the node
//! catalogue and factory, how nodes and pins look, which pins connect, dynamic
//! pins, and which nodes open another graph. Pure Rust (no GPUI); the canvas
//! owns the document, layout, drawing and input.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use graphy::{
    Compatibility, DataType, GraphCommand, GraphDescription, NodeInstance, PinInstance, Position,
    StructuralResolver, TypeResolver,
};
use graphy_gpui::{Action, CatalogEntry, GraphProfile, MacroTarget, MenuItem, MenuTarget};
use graphy_render::geom::rgba;
use graphy_render::presenter::{
    default_pin_style, prettify, CommentPresentation, NodePresentation, PinStyle, Presenter,
};
use graphy_render::{NodeShape, PinSide};
use helio_graph_asset::{sections, NodeRegistry};

use crate::document::{ENTRY_TYPE, EXIT_TYPE};
use crate::model;

/// Context menu entries this editor adds to Graphy's.
pub const MENU_COLLAPSE: &str = "render.collapse";
pub const MENU_EXPAND: &str = "render.expand";

/// Section id -> display name, shared between the editor (which renames and adds
/// sections) and the presenter (which titles the nodes that place them).
pub type SectionNames = Arc<RwLock<HashMap<String, String>>>;

/// Two pins connect when their type strings are equal: kind, persistence and
/// Helio resource name all have to match, because Helio binds resources by name.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderResolver;

impl TypeResolver for RenderResolver {
    fn compatibility(&self, from: &DataType, to: &DataType) -> Compatibility {
        match StructuralResolver.compatibility(from, to) {
            Compatibility::Unknown => Compatibility::Incompatible(format!(
                "{} cannot connect to {}",
                model::type_name(from),
                model::type_name(to)
            )),
            other => other,
        }
    }
}

/// Presentation of render graph nodes and pins.
pub struct RenderPresenter {
    registry: Arc<NodeRegistry>,
    sections: SectionNames,
}

impl Presenter for RenderPresenter {
    fn node(&self, node: &NodeInstance) -> NodePresentation {
        let ty = node.node_type.as_str();
        if node.is_reroute() {
            let mut p = NodePresentation::card("", rgba(0x8b, 0x8b, 0x8b, 1.0));
            p.shape = NodeShape::Reroute;
            return p;
        }
        if matches!(ty, ENTRY_TYPE | EXIT_TYPE) {
            let (title, icon) = if ty == ENTRY_TYPE {
                ("Section Entry", '▶')
            } else {
                ("Section Exit", '◀')
            };
            let mut p = NodePresentation::card(title, rgba(0x7c, 0x3a, 0xed, 1.0));
            p.icon = Some(icon);
            return p;
        }
        if let Some(id) = sections::section_id(ty) {
            let names = self.sections.read().unwrap_or_else(|e| e.into_inner());
            let title = names.get(id).cloned().unwrap_or_else(|| prettify(id));
            let mut p = NodePresentation::card(title, rgba(0x9b, 0x59, 0xb6, 1.0));
            p.icon = Some('📦');
            return p;
        }
        match self.registry.get(ty) {
            Some(descriptor) => {
                let (accent, icon) = model::category_look(&descriptor.category);
                let title = model::buffer_title(ty, &model::effective_text(Some(descriptor), node))
                    .unwrap_or_else(|| descriptor.title.clone());
                let mut p = NodePresentation::card(title, accent);
                p.icon = Some(icon);
                p
            }
            None => NodePresentation::card(ty.to_string(), rgba(0x4a, 0x55, 0x68, 1.0)),
        }
    }

    fn pin(&self, _node: &NodeInstance, pin: &PinInstance, _side: PinSide) -> PinStyle {
        let type_name = model::type_name(pin.data_type());
        if type_name == "?" {
            default_pin_style(pin.data_type())
        } else {
            model::pin_style(&type_name)
        }
    }

    fn pin_label(&self, _node: &NodeInstance, pin: &PinInstance, _side: PinSide) -> String {
        pin.pin.name.clone()
    }

    fn comment(&self, _index: usize, comment: &graphy::core::GraphComment) -> CommentPresentation {
        CommentPresentation {
            accent: comment.color,
        }
    }
}

/// The render graph profile of one open document.
pub struct RenderProfile {
    registry: Arc<NodeRegistry>,
    presenter: RenderPresenter,
    resolver: RenderResolver,
}

impl RenderProfile {
    pub fn new(registry: Arc<NodeRegistry>, sections: SectionNames) -> Self {
        Self {
            presenter: RenderPresenter {
                registry: registry.clone(),
                sections,
            },
            registry,
            resolver: RenderResolver,
        }
    }

    pub fn registry(&self) -> &Arc<NodeRegistry> {
        &self.registry
    }
}

impl GraphProfile for RenderProfile {
    fn id(&self) -> &str {
        "render-graph"
    }

    fn presenter(&self) -> &dyn Presenter {
        &self.presenter
    }

    fn type_resolver(&self) -> &dyn TypeResolver {
        &self.resolver
    }

    fn catalog(&self) -> Vec<CatalogEntry> {
        model::catalog(&self.registry)
    }

    fn create_node(&self, definition: &str, id: &str, pos: Position) -> Option<NodeInstance> {
        model::create_node(&self.registry, definition, id, pos)
    }

    fn pin_commands(&self, graph: &GraphDescription, node: &str) -> Vec<GraphCommand> {
        model::refresh_pins(graph, node, &self.registry)
    }

    fn menu_items(&self, graph: &GraphDescription, target: &MenuTarget) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if matches!(target, MenuTarget::Node(_) | MenuTarget::Selection) {
            items.push(MenuItem::new(MENU_COLLAPSE, "Collapse to Section").separated());
        }
        if let MenuTarget::Node(id) = target {
            let section = graph
                .nodes
                .get(id)
                .is_some_and(|node| sections::section_id(&node.node_type).is_some());
            if section {
                items.push(MenuItem::new(MENU_EXPAND, "Expand Section"));
            }
        }
        items
    }

    fn run_menu_item(
        &self,
        _graph: &GraphDescription,
        target: &MenuTarget,
        item: &str,
    ) -> Vec<Action> {
        match item {
            MENU_COLLAPSE | MENU_EXPAND => vec![Action::Custom {
                id: item.to_string(),
                target: target.clone(),
            }],
            _ => Vec::new(),
        }
    }

    fn is_deletable(&self, node: &NodeInstance) -> bool {
        !matches!(node.node_type.as_str(), ENTRY_TYPE | EXIT_TYPE)
    }

    fn macro_target(&self, node: &NodeInstance) -> Option<MacroTarget> {
        sections::section_id(&node.node_type).map(|id| MacroTarget {
            graph_id: id.to_string(),
        })
    }
}

impl graphy_gpui::canvas::GpuiProfile for RenderProfile {}
