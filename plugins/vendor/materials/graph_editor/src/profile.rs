//! The shader editor's profile: what it tells the shared Graphy canvas.
//!
//! [`ShaderProfile`] is the `GraphProfile` / `Presenter` / `GpuiProfile` of the
//! Shader editor. It supplies the node catalogue and node factory from the shader
//! definitions, the WGSL connection rules ([`crate::types`]), the look of nodes and
//! pins, and the preview rows. It owns no drawing, layout or hit-testing: a preview
//! is an [`ExtraRow`] with a height, and Graphy lays the node out around it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{AnyElement, App, Window};
use graphy::{DataType, NodeInstance, Pin, PinInstance, PinType, Position, TypeResolver};
use graphy_gpui::canvas::GpuiProfile;
use graphy_gpui::{CatalogEntry, GraphProfile};
use graphy_render::geom::rgba;
use graphy_render::{
    category_accent, Color, ExtraRow, NodePresentation, PinShape, PinSide, PinStyle, Presenter,
    RowPosition,
};
use serde_json::Value;

use crate::definitions::{NodeDefinition, NodeDefinitions};
use crate::previews::{PinPreviews, PREVIEW_MIN_WIDTH, PREVIEW_ROW_H};
use crate::types::{is_texture_previewable, pin_color, resolver};

/// `#RRGGBB` to a canvas colour.
fn hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(rgba(channel(0)?, channel(2)?, channel(4)?, 1.0))
}

/// Editor text as a property value: numbers and booleans are stored as such,
/// blank text is no value.
pub fn text_to_value(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        None
    } else {
        Some(default_value(text))
    }
}

/// A node default from a definition: numbers and booleans are stored as such.
fn default_value(text: &str) -> Value {
    let text = text.trim();
    if let Ok(number) = text.parse::<f64>() {
        serde_json::json!(number)
    } else if let Ok(boolean) = text.parse::<bool>() {
        serde_json::json!(boolean)
    } else {
        Value::String(text.to_string())
    }
}

/// The node a definition describes, with its pins and default properties.
pub fn node_from_definition(definition: &NodeDefinition, id: &str, pos: Position) -> NodeInstance {
    let mut node = NodeInstance::new(id, definition.id.as_str(), pos);
    for pin in &definition.inputs {
        node.inputs.push(PinInstance::new(
            pin.id.clone(),
            Pin::new(pin.id.clone(), pin.name.clone(), pin.data_type.clone(), PinType::Input),
        ));
    }
    for pin in &definition.outputs {
        node.outputs.push(PinInstance::new(
            pin.id.clone(),
            Pin::new(pin.id.clone(), pin.name.clone(), pin.data_type.clone(), PinType::Output),
        ));
    }
    for (key, value) in &definition.properties {
        node.properties.insert(key.clone(), default_value(value));
    }
    node
}

/// How shader nodes, pins and preview rows look.
pub struct ShaderPresenter {
    previews_enabled: Arc<AtomicBool>,
}

impl Presenter for ShaderPresenter {
    fn node(&self, node: &NodeInstance) -> NodePresentation {
        if node.is_reroute() {
            let mut p = NodePresentation::card("", rgba(0x8b, 0x8b, 0x8b, 1.0));
            p.shape = graphy_render::NodeShape::Reroute;
            return p;
        }
        let definitions = NodeDefinitions::load();
        let definition = definitions.get_node_definition(&node.node_type);
        let title = definition.map_or_else(
            || graphy_render::prettify(&node.node_type),
            |d| d.name.clone(),
        );
        let accent = definition
            .and_then(|d| d.color.as_deref())
            .and_then(hex_color)
            .unwrap_or_else(|| category_accent(&node.node_type));
        let mut presentation = NodePresentation::card(title, accent);
        presentation.subtitle = None;

        // Variable-height rows: each previewable output gets a row under its pin.
        if self.previews_enabled.load(Ordering::Relaxed) {
            for (index, pin) in node.outputs.iter().enumerate() {
                if is_texture_previewable(pin.data_type()) {
                    presentation.extra_rows.push(ExtraRow {
                        position: RowPosition::AfterRow(index),
                        slot: index as u32,
                        height: PREVIEW_ROW_H,
                        min_width: PREVIEW_MIN_WIDTH,
                    });
                }
            }
        }
        presentation
    }

    fn pin(&self, _node: &NodeInstance, pin: &PinInstance, _side: PinSide) -> PinStyle {
        let data_type = pin.data_type();
        PinStyle {
            shape: if data_type.is_execution() {
                PinShape::Arrow
            } else {
                PinShape::Circle
            },
            ringed: false,
            color: pin_color(data_type),
        }
    }
}

/// The profile of the shader editor.
pub struct ShaderProfile {
    presenter: ShaderPresenter,
    resolver: graphy::ChainResolver,
    previews: Arc<PinPreviews>,
}


impl ShaderProfile {
    pub fn new(previews: Arc<PinPreviews>) -> Self {
        Self {
            presenter: ShaderPresenter {
                previews_enabled: previews.enabled_flag(),
            },
            resolver: resolver(),
            previews,
        }
    }
}

impl GraphProfile for ShaderProfile {
    fn id(&self) -> &str {
        "shader"
    }

    fn presenter(&self) -> &dyn Presenter {
        &self.presenter
    }

    fn type_resolver(&self) -> &dyn TypeResolver {
        &self.resolver
    }

    fn catalog(&self) -> Vec<CatalogEntry> {
        let mut categories: Vec<_> = NodeDefinitions::load().categories.iter().collect();
        categories.sort_by(|a, b| a.name.cmp(&b.name));
        categories
            .into_iter()
            .flat_map(|category| {
                let mut nodes: Vec<_> = category.nodes.iter().collect();
                nodes.sort_by(|a, b| a.name.cmp(&b.name));
                nodes.into_iter().map(move |node| CatalogEntry {
                    definition: node.id.clone(),
                    title: node.name.clone(),
                    category: category.name.clone(),
                    keywords: Vec::new(),
                    description: node.description.clone(),
                    icon: node.icon.clone(),
                    color: graphy_render::geom::parse_hex(&category.color),
                    inputs: Some(node.inputs.iter().map(|p| p.data_type.clone()).collect()),
                })
            })
            .collect()
    }

    fn create_node(&self, definition: &str, id: &str, pos: Position) -> Option<NodeInstance> {
        if definition == "reroute" {
            return Some(self.create_reroute(id, pos, &DataType::typed("?")));
        }
        let definition = NodeDefinitions::load().get_node_definition(definition)?;
        Some(node_from_definition(definition, id, pos))
    }

    /// Shader reroutes name their pins `input` / `output`.
    fn create_reroute(&self, id: &str, pos: Position, data_type: &DataType) -> NodeInstance {
        let mut node = NodeInstance::new(id, "reroute", pos);
        node.add_input_pin("input", data_type.clone());
        node.add_output_pin("output", data_type.clone());
        node
    }
}

impl GpuiProfile for ShaderProfile {
    fn render_slot(
        &self,
        graph: &graphy::GraphDescription,
        node: &str,
        slot: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.previews.slot(graph, node, slot, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphy_render::{LayoutMetrics, NodeLayout, NodeSpec, RowContent};

    fn profile() -> ShaderProfile {
        ShaderProfile::new(Arc::new(PinPreviews::new()))
    }

    #[test]
    fn catalogue_creates_every_node_with_its_definition_pins() {
        let profile = profile();
        let catalog = profile.catalog();
        assert!(!catalog.is_empty());
        for entry in &catalog {
            let node = profile
                .create_node(&entry.definition, "n", Position::new(0.0, 0.0))
                .unwrap_or_else(|| panic!("{} creates no node", entry.definition));
            assert_eq!(node.node_type, entry.definition);
        }
    }

    #[test]
    fn constants_start_with_a_numeric_value() {
        let node = profile()
            .create_node("float_constant", "n", Position::new(0.0, 0.0))
            .unwrap();
        assert_eq!(node.properties.get("value"), Some(&serde_json::json!(0.0)));
    }

    #[test]
    fn reroutes_keep_the_shader_pin_names() {
        let node = profile()
            .create_node("reroute", "r", Position::new(0.0, 0.0))
            .unwrap();
        assert!(node.is_reroute());
        assert_eq!(node.inputs[0].id, "input");
        assert_eq!(node.outputs[0].id, "output");
    }

    /// Previews are off by default: a colour output is an ordinary pin row. Turned
    /// on, the presenter reserves a taller row under that output only, and the
    /// shared layout puts the following pins below it.
    #[test]
    fn preview_rows_are_variable_height_rows_in_the_shared_layout() {
        let previews = Arc::new(PinPreviews::new());
        let profile = ShaderProfile::new(previews.clone());
        let node = profile
            .create_node("fragment_output", "o", Position::new(0.0, 0.0))
            .unwrap();
        let mut sample = NodeInstance::new("s", "x", Position::new(0.0, 0.0));
        sample.add_output_pin("a", DataType::typed("f32"));
        sample.add_output_pin("color", DataType::typed("vec4<f32>"));
        sample.add_output_pin("b", DataType::typed("f32"));
        let _ = node;

        let layout_of = |profile: &ShaderProfile| {
            let presentation = profile.presenter().node(&sample);
            let m = LayoutMetrics::default();
            let (rows, min_width) = NodeSpec::build_rows(
                0,
                sample.outputs.len(),
                None,
                &presentation.extra_rows,
                |_| m.row_h,
            );
            let spec = NodeSpec {
                shape: presentation.shape,
                title_width: 40.0,
                has_icon: false,
                input_label_widths: vec![],
                output_label_widths: vec![30.0; 3],
                header_output: None,
                rows,
                min_width,
            };
            (presentation.extra_rows.len(), NodeLayout::compute(&spec, &m))
        };

        let (extra, off) = layout_of(&profile);
        assert_eq!(extra, 0);
        previews.set_enabled(true);
        let (extra, on) = layout_of(&profile);
        assert_eq!(extra, 1);

        let m = LayoutMetrics::default();
        // The preview row sits under the `color` pin (row 1) and is taller than a pin row.
        let custom: Vec<_> = on
            .rows
            .iter()
            .filter(|r| matches!(r.content, RowContent::Custom { .. }))
            .collect();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].rect.height(), PREVIEW_ROW_H);
        assert!(PREVIEW_ROW_H > m.row_h);
        assert!(custom[0].rect.min.y > on.outputs[1].center.y);
        // The pin after the preview moved down by the row's height; the ones before did not.
        assert_eq!(on.outputs[0].center.y, off.outputs[0].center.y);
        assert_eq!(on.outputs[1].center.y, off.outputs[1].center.y);
        assert_eq!(
            on.outputs[2].center.y - off.outputs[2].center.y,
            PREVIEW_ROW_H + m.row_gap
        );
        assert!(on.size.x >= PREVIEW_MIN_WIDTH);
        assert!(on.size.y > off.size.y);
    }

    /// The preview row is hit as a host row with its slot, the pins around it are
    /// still pins, and the row is placed where the layout says (what the preview
    /// element is positioned over).
    #[test]
    fn hit_testing_finds_the_preview_row_and_the_pins_around_it() {
        use graphy_render::{
            FixedAdvance, Hit, HitTolerance, NodePart, PinAddr, Rect, Scene, Vec2,
        };

        let previews = Arc::new(PinPreviews::new());
        previews.set_enabled(true);
        let profile = ShaderProfile::new(previews);
        let mut graph = graphy::GraphDescription::new("g");
        let mut node = NodeInstance::new("n", "x", Position::new(100.0, 40.0));
        node.add_output_pin("a", DataType::typed("f32"));
        node.add_output_pin("color", DataType::typed("vec4<f32>"));
        node.add_output_pin("b", DataType::typed("f32"));
        graph.add_node(node);

        let mut scene = Scene::new(LayoutMetrics::default());
        scene.rebuild(&graph, profile.presenter(), &FixedAdvance::default());
        let everywhere = Rect::from_pos_size(Vec2::new(-1.0e4, -1.0e4), Vec2::new(2.0e4, 2.0e4));
        let slots = scene.slot_rects(everywhere);
        assert_eq!(slots.len(), 1);
        assert_eq!((slots[0].node.as_str(), slots[0].slot), ("n", 1));
        assert!(slots[0].rect.height() > 0.0);

        let tol = HitTolerance::default();
        let centre = slots[0].rect.center();
        match scene.hit_test(centre, 1.0, tol) {
            Hit::Node { part, .. } => assert_eq!(part, NodePart::Custom(1)),
            other => panic!("expected the preview row, got {other:?}"),
        }
        // The pin of the row above, and the pin below the preview, are pins.
        let node = scene.node("n").unwrap();
        for (index, side_pin) in [0usize, 2].into_iter().enumerate() {
            let at = node.pin_pos(graphy_render::PinSide::Output, side_pin).unwrap();
            match scene.hit_test(at, 1.0, tol) {
                Hit::Pin(PinAddr { pin, .. }) => assert_eq!(pin, side_pin, "pin {index}"),
                other => panic!("expected pin {side_pin}, got {other:?}"),
            }
        }
        // The pin below sits under the preview row, not inside it.
        let below = node.pin_pos(graphy_render::PinSide::Output, 2).unwrap().y;
        assert!(below > slots[0].rect.max.y);
    }
}
