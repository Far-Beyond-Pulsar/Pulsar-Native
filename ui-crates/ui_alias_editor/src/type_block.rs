use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, ActiveTheme, StyledExt, Colorize};
use ui_types_common::TypeAstNode;
use std::sync::Arc;

/// Unique identifier for a block instance
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub Arc<str>);

impl BlockId {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        BlockId(Arc::from(format!("block_{}", id)))
    }
}

/// Slot label for constructor parameters
#[derive(Clone, Debug)]
pub struct SlotLabel {
    pub text: String,
    pub index: usize,
}

/// Represents a visual block for a type node (like Scratch blocks)
#[derive(Clone, Debug)]
pub enum TypeBlock {
    /// Primitive type block (leaf node)
    Primitive {
        id: BlockId,
        name: String,
        color: BlockColor,
    },
    /// Path type block (leaf node)
    Path {
        id: BlockId,
        path: String,
        color: BlockColor,
    },
    /// Alias reference block (leaf node)
    AliasRef {
        id: BlockId,
        alias: String,
        color: BlockColor,
    },
    /// Constructor block with labeled slots for nested types
    Constructor {
        id: BlockId,
        name: String,
        color: BlockColor,
        slots: Vec<Option<Box<TypeBlock>>>,
        slot_labels: Vec<String>,  // Labels like "T", "E", "K", "V"
        expected_params: usize,
    },
    /// Tuple block with multiple element slots
    Tuple {
        id: BlockId,
        color: BlockColor,
        elements: Vec<Option<Box<TypeBlock>>>,
    },
}

#[derive(Clone, Debug, Copy)]
pub enum BlockColor {
    Primitive,   // Blue
    Path,        // Green
    Alias,       // Purple
    Constructor, // Orange
    Tuple,       // Yellow
}

impl BlockColor {
    pub fn to_hsla(&self) -> Hsla {
        match self {
            BlockColor::Primitive => hsla(0.6, 0.7, 0.5, 1.0),   // Blue
            BlockColor::Path => hsla(0.35, 0.7, 0.5, 1.0),       // Green
            BlockColor::Alias => hsla(0.75, 0.7, 0.5, 1.0),      // Purple
            BlockColor::Constructor => hsla(0.08, 0.8, 0.6, 1.0), // Orange
            BlockColor::Tuple => hsla(0.15, 0.8, 0.6, 1.0),      // Yellow
        }
    }
}

impl TypeBlock {
    /// Create a primitive type block
    pub fn primitive(name: impl Into<String>) -> Self {
        TypeBlock::Primitive {
            id: BlockId::new(),
            name: name.into(),
            color: BlockColor::Primitive,
        }
    }

    /// Create a path type block
    pub fn path(path: impl Into<String>) -> Self {
        TypeBlock::Path {
            id: BlockId::new(),
            path: path.into(),
            color: BlockColor::Path,
        }
    }

    /// Create an alias reference block
    pub fn alias(alias: impl Into<String>) -> Self {
        TypeBlock::AliasRef {
            id: BlockId::new(),
            alias: alias.into(),
            color: BlockColor::Alias,
        }
    }

    /// Create a constructor block (Box, Arc, Vec, etc.) with labeled slots
    pub fn constructor(name: impl Into<String>, param_count: usize) -> Self {
        let name = name.into();
        let slots = vec![None; param_count];
        
        // Generate default slot labels (T, E, K, V, etc.)
        let slot_labels = Self::generate_slot_labels(&name, param_count);

        TypeBlock::Constructor {
            id: BlockId::new(),
            name,
            color: BlockColor::Constructor,
            slots,
            slot_labels,
            expected_params: param_count,
        }
    }

    /// Create a tuple block
    pub fn tuple(element_count: usize) -> Self {
        TypeBlock::Tuple {
            id: BlockId::new(),
            color: BlockColor::Tuple,
            elements: vec![None; element_count],
        }
    }

    /// Generate meaningful slot labels based on constructor name
    fn generate_slot_labels(name: &str, param_count: usize) -> Vec<String> {
        match name {
            "Result" if param_count == 2 => vec!["T".to_string(), "E".to_string()],
            "HashMap" | "BTreeMap" if param_count == 2 => vec!["K".to_string(), "V".to_string()],
            _ => (0..param_count).map(|i| {
                if i == 0 { "T".to_string() }
                else { format!("T{}", i) }
            }).collect()
        }
    }

    /// Get the block's ID
    pub fn id(&self) -> &BlockId {
        match self {
            TypeBlock::Primitive { id, .. }
            | TypeBlock::Path { id, .. }
            | TypeBlock::AliasRef { id, .. }
            | TypeBlock::Constructor { id, .. }
            | TypeBlock::Tuple { id, .. } => id,
        }
    }

    /// Set a slot's content
    pub fn set_slot(&mut self, slot_index: usize, block: TypeBlock) -> bool {
        match self {
            TypeBlock::Constructor { slots, .. } => {
                if slot_index < slots.len() {
                    slots[slot_index] = Some(Box::new(block));
                    true
                } else {
                    false
                }
            }
            TypeBlock::Tuple { elements, .. } => {
                if slot_index < elements.len() {
                    elements[slot_index] = Some(Box::new(block));
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Get a slot's content
    pub fn get_slot(&self, slot_index: usize) -> Option<&TypeBlock> {
        match self {
            TypeBlock::Constructor { slots, .. } => {
                slots.get(slot_index).and_then(|s| s.as_ref()).map(|b| b.as_ref())
            }
            TypeBlock::Tuple { elements, .. } => {
                elements.get(slot_index).and_then(|e| e.as_ref()).map(|b| b.as_ref())
            }
            _ => None,
        }
    }

    /// Remove a block from a slot and return it
    pub fn take_slot(&mut self, slot_index: usize) -> Option<TypeBlock> {
        match self {
            TypeBlock::Constructor { slots, .. } => {
                slots.get_mut(slot_index).and_then(|s| s.take()).map(|b| *b)
            }
            TypeBlock::Tuple { elements, .. } => {
                elements.get_mut(slot_index).and_then(|e| e.take()).map(|b| *b)
            }
            _ => None,
        }
    }

    /// Get slot labels for constructor blocks
    pub fn slot_labels(&self) -> Option<&[String]> {
        match self {
            TypeBlock::Constructor { slot_labels, .. } => Some(slot_labels),
            _ => None,
        }
    }

    /// Convert to AST node for code generation
    pub fn to_ast(&self) -> Option<TypeAstNode> {
        match self {
            TypeBlock::Primitive { name, .. } => Some(TypeAstNode::Primitive {
                name: name.clone(),
            }),
            TypeBlock::Path { path, .. } => Some(TypeAstNode::Path {
                path: path.clone(),
            }),
            TypeBlock::AliasRef { alias, .. } => Some(TypeAstNode::AliasRef {
                alias: alias.clone(),
            }),
            TypeBlock::Constructor { name, slots, .. } => {
                let params: Vec<_> = slots
                    .iter()
                    .filter_map(|slot| slot.as_ref().and_then(|b| b.to_ast()))
                    .collect();

                // Check if all slots are filled
                if params.len() != slots.len() {
                    return None;
                }

                Some(TypeAstNode::Constructor {
                    name: name.clone(),
                    params,
                    lifetimes: vec![],
                    const_generics: vec![],
                })
            }
            TypeBlock::Tuple { elements, .. } => {
                let element_nodes: Vec<_> = elements
                    .iter()
                    .filter_map(|el| el.as_ref().and_then(|b| b.to_ast()))
                    .collect();

                if element_nodes.len() != elements.len() {
                    return None;
                }

                Some(TypeAstNode::Tuple {
                    elements: element_nodes,
                })
            }
        }
    }

    /// Create from AST node
    pub fn from_ast(node: &TypeAstNode) -> Self {
        match node {
            TypeAstNode::Primitive { name } => TypeBlock::primitive(name.clone()),
            TypeAstNode::Path { path } => TypeBlock::path(path.clone()),
            TypeAstNode::AliasRef { alias } => TypeBlock::alias(alias.clone()),
            TypeAstNode::Constructor { name, params, .. } => {
                let mut block = TypeBlock::constructor(name.clone(), params.len());
                if let TypeBlock::Constructor { slots, .. } = &mut block {
                    for (i, param) in params.iter().enumerate() {
                        match param {
                            TypeAstNode::None => {
                                // Leave slot empty for None
                            }
                            _ => {
                                if i < slots.len() {
                                    slots[i] = Some(Box::new(TypeBlock::from_ast(param)));
                                }
                            }
                        }
                    }
                }
                block
            }
            TypeAstNode::Tuple { elements } => {
                let mut block = TypeBlock::tuple(elements.len());
                if let TypeBlock::Tuple { elements: el, .. } = &mut block {
                    for (i, elem) in elements.iter().enumerate() {
                        match elem {
                            TypeAstNode::None => {
                                // Leave slot empty for None
                            }
                            _ => {
                                if i < el.len() {
                                    el[i] = Some(Box::new(TypeBlock::from_ast(elem)));
                                }
                            }
                        }
                    }
                }
                block
            }
            TypeAstNode::FnPointer { .. } => TypeBlock::primitive("FnPtr"),
            TypeAstNode::None => {
                // Do not fill slot, leave it empty
                TypeBlock::primitive("") // Or handle as needed
            }
        }
    }

    /// Get display name for the block
    pub fn display_name(&self) -> String {
        match self {
            TypeBlock::Primitive { name, .. } => name.clone(),
            TypeBlock::Path { path, .. } => path.clone(),
            TypeBlock::AliasRef { alias, .. } => alias.clone(),
            TypeBlock::Constructor { name, .. } => name.clone(),
            TypeBlock::Tuple { .. } => "Tuple".to_string(),
        }
    }

    /// Get the color for this block
    pub fn color(&self) -> BlockColor {
        match self {
            TypeBlock::Primitive { color, .. }
            | TypeBlock::Path { color, .. }
            | TypeBlock::AliasRef { color, .. }
            | TypeBlock::Constructor { color, .. }
            | TypeBlock::Tuple { color, .. } => *color,
        }
    }

    /// Check if this is a container block (has slots)
    pub fn is_container(&self) -> bool {
        matches!(self, TypeBlock::Constructor { .. } | TypeBlock::Tuple { .. })
    }

    /// Get number of slots (0 for leaf nodes)
    pub fn slot_count(&self) -> usize {
        match self {
            TypeBlock::Constructor { slots, .. } => slots.len(),
            TypeBlock::Tuple { elements, .. } => elements.len(),
            _ => 0,
        }
    }
    


    /// Check if a slot is filled
    pub fn is_slot_filled(&self, index: usize) -> bool {
        match self {
            TypeBlock::Constructor { slots, .. } => {
                slots.get(index).and_then(|s| s.as_ref()).is_some()
            }
            TypeBlock::Tuple { elements, .. } => {
                elements.get(index).and_then(|e| e.as_ref()).is_some()
            }
            _ => false,
        }
    }

    /// Find a block by ID in this tree
    pub fn find_block_mut(&mut self, target_id: &BlockId) -> Option<&mut TypeBlock> {
        if self.id() == target_id {
            return Some(self);
        }

        match self {
            TypeBlock::Constructor { slots, .. } => {
                for slot in slots {
                    if let Some(block) = slot {
                        if let Some(found) = block.find_block_mut(target_id) {
                            return Some(found);
                        }
                    }
                }
            }
            TypeBlock::Tuple { elements, .. } => {
                for element in elements {
                    if let Some(block) = element {
                        if let Some(found) = block.find_block_mut(target_id) {
                            return Some(found);
                        }
                    }
                }
            }
            _ => {}
        }

        None
    }
}

/// Visual representation of a type block
pub struct TypeBlockView {
    block: TypeBlock,
    id: ElementId,
    on_slot_click: Option<Arc<dyn Fn(BlockId, usize) + Send + Sync + 'static>>,
}

impl TypeBlockView {
    pub fn new(block: TypeBlock, id: impl Into<ElementId>) -> Self {
        Self {
            block,
            id: id.into(),
            on_slot_click: None,
        }
    }
    
    pub fn on_slot_click(mut self, handler: impl Fn(BlockId, usize) + Send + Sync + 'static) -> Self {
        self.on_slot_click = Some(Arc::new(handler));
        self
    }

    fn render_leaf_block(&self, cx: Option<&App>) -> Div {
        let color = self.block.color().to_hsla();

        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .bg(color)
            .rounded(px(6.0))
            .border_1()
            .border_color(color.lighten(0.1))
            .shadow_sm()
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(gpui::white())
                    .child(self.block.display_name())
            )
    }

    fn render_container_block(&self, cx: Option<&App>) -> Div {
        let color = self.block.color().to_hsla();

        match &self.block {
            TypeBlock::Constructor { name, slots, slot_labels, .. } => {
                v_flex()
                    .gap_0()
                    .min_w(px(200.0))
                    .child(
                        // Header - curved top, wraps around
                        h_flex()
                            .px_4()
                            .py_2()
                            .bg(color)
                            .rounded_t(px(8.0))
                            .border_2()
                            .border_color(color.lighten(0.15))
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_bold()
                                    .text_color(gpui::white())
                                    .child(name.clone())
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::white().opacity(0.7))
                                    .child("<")
                            )
                    )
                    .children(slots.iter().enumerate().map(|(i, slot)| {
                        // Each slot has a label and wraps with notches (Scratch-style)
                        let label = slot_labels.get(i).map(|s| s.as_str()).unwrap_or("T");
                        
                        v_flex()
                            .gap_0()
                            .child(
                                // Slot label and notch top
                                h_flex()
                                    .bg(color.opacity(0.3))
                                    .border_x_2()
                                    .border_color(color.lighten(0.15))
                                    .pl_4()
                                    .pr_2()
                                    .py_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(color.lighten(0.3))
                                            .child(format!("{}: ", label))
                                    )
                            )
                            .child(
                                // Slot content area with inset
                                h_flex()
                                    .bg(color.opacity(0.15))
                                    .border_x_2()
                                    .border_color(color.lighten(0.15))
                                    .px_3()
                                    .py_3()
                                    .child(self.render_slot(i, slot, cx))
                            )
                    }))
                    .child(
                        // Footer - curved bottom, closes the wrap
                        h_flex()
                            .px_4()
                            .py_2()
                            .bg(color)
                            .rounded_b(px(8.0))
                            .border_2()
                            .border_color(color.lighten(0.15))
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_bold()
                                    .text_color(gpui::white().opacity(0.7))
                                    .child(">")
                            )
                    )
            }
            TypeBlock::Tuple { elements, .. } => {
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .bg(color)
                            .rounded_l(px(6.0))
                            .border_1()
                            .border_color(color.lighten(0.1))
                            .text_sm()
                            .font_bold()
                            .text_color(gpui::white())
                            .child("(")
                    )
                    .children(
                        elements.iter().enumerate().map(|(i, el)| {
                            h_flex()
                                .gap_1()
                                .child(self.render_slot(i, el, cx))
                                .when(i < elements.len() - 1, |this| {
                                    this.child(
                                        div()
                                            .text_sm()
                                            .text_color(color)
                                            .child(",")
                                    )
                                })
                        })
                    )
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .bg(color)
                            .rounded_r(px(6.0))
                            .border_1()
                            .border_color(color.lighten(0.1))
                            .text_sm()
                            .font_bold()
                            .text_color(gpui::white())
                            .child(")")
                    )
            }
            _ => div().child(self.render_leaf_block(cx)),
        }
    }

    fn render_slot(&self, index: usize, slot: &Option<Box<TypeBlock>>, _cx: Option<&App>) -> Div {
        if let Some(block) = slot {
            let mut nested_view = TypeBlockView::new(
                *block.clone(),
                ("slot", index),
            );
            // Pass down the click handler to nested blocks
            if let Some(handler) = &self.on_slot_click {
                let handler = Arc::clone(handler);
                nested_view = nested_view.on_slot_click(move |id, idx| handler(id, idx));
            }
            div().child(nested_view)
        } else {
            // Explicitly render a visual indicator for None slots
            let parent_id = self.block.id();
            let slot_idx = index;
            let mut slot_div = div()
                .min_w(px(150.0))
                .px_4()
                .py_4()
                .bg(hsla(0.0, 0.0, 0.2, 0.2))
                .rounded(px(6.0))
                .border_2()
                .border_color(hsla(0.0, 0.0, 0.4, 0.6))
                .border_dashed()
                .items_center()
                .justify_center()
                .hover(|style| {
                    style
                        .bg(hsla(0.0, 0.0, 0.3, 0.3))
                        .border_color(hsla(0.6, 0.7, 0.6, 0.8))
                        .cursor_pointer()
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                        .child("Empty slot (None)")
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(hsla(0.0, 0.0, 0.5, 0.7))
                        .child("Click to select type")
                );
            // Add click handler if provided
            if let Some(handler) = &self.on_slot_click {
                let handler = Arc::clone(handler);
                let parent_id = parent_id.clone();
                slot_div = slot_div.on_mouse_down(gpui::MouseButton::Left, move |_event, _window, _cx| {
                    handler(parent_id.clone(), slot_idx);
                });
            }
            slot_div
        }
    }
}

impl IntoElement for TypeBlockView {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        let id = self.id.clone();
        let content = if self.block.is_container() {
            self.render_container_block(None)
        } else {
            self.render_leaf_block(None)
        };
        
        div().id(id).child(content)
    }
}

impl RenderOnce for TypeBlockView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let content = if self.block.is_container() {
            self.render_container_block(Some(cx))
        } else {
            self.render_leaf_block(Some(cx))
        };
        
        div().id(self.id).child(content)
    }
}
