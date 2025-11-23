use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};
use crate::type_block::{TypeBlock, BlockId};
use std::collections::HashMap;
use std::sync::Arc;

/// Drag state for blocks
#[derive(Clone, Debug)]
pub struct DragState {
    pub dragging_block: Option<TypeBlock>,
    pub drag_start_pos: Point<Pixels>,
    pub current_pos: Point<Pixels>,
    pub hover_target: Option<DropTarget>,
}

/// Where a block can be dropped
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Drop as root (replaces current root)
    Root,
    /// Drop into a slot of a specific block
    Slot {
        parent_block_id: BlockId,
        slot_index: usize,
    },
}

/// Canvas for visually composing type blocks with drag-and-drop
pub struct BlockCanvas {
    /// The root block being edited (the main type expression)
    root_block: Option<TypeBlock>,
    
    /// Current drag state
    drag_state: Option<DragState>,
    
    /// Hover highlight state
    hover_slot: Option<(BlockId, usize)>,
    
    /// Canvas bounds for coordinate conversion
    canvas_bounds: Option<Bounds<Pixels>>,
    
    /// Selected block for keyboard operations
    selected_block: Option<BlockId>,
}

impl BlockCanvas {
    pub fn new() -> Self {
        Self {
            root_block: None,
            drag_state: None,
            hover_slot: None,
            canvas_bounds: None,
            selected_block: None,
        }
    }

    pub fn with_root(root_block: TypeBlock) -> Self {
        Self {
            root_block: Some(root_block),
            drag_state: None,
            hover_slot: None,
            canvas_bounds: None,
            selected_block: None,
        }
    }

    pub fn root_block(&self) -> Option<&TypeBlock> {
        self.root_block.as_ref()
    }

    pub fn set_root_block(&mut self, block: Option<TypeBlock>) {
        self.root_block = block;
    }
    
    /// Fill a slot in a block with a new child block
    pub fn fill_slot(&mut self, parent_id: BlockId, slot_idx: usize, child: TypeBlock) -> bool {
        if let Some(root) = &mut self.root_block {
            Self::fill_slot_recursive(root, &parent_id, slot_idx, child)
        } else {
            false
        }
    }
    
    fn fill_slot_recursive(block: &mut TypeBlock, parent_id: &BlockId, slot_idx: usize, child: TypeBlock) -> bool {
        // Check if this is the parent block
        if block.id() == parent_id {
            return block.set_slot(slot_idx, child);
        }
        
        // Recursively search in child slots
        match block {
            TypeBlock::Constructor { slots, .. } => {
                for slot in slots.iter_mut() {
                    if let Some(nested) = slot {
                        if Self::fill_slot_recursive(nested, parent_id, slot_idx, child.clone()) {
                            return true;
                        }
                    }
                }
            }
            TypeBlock::Tuple { elements, .. } => {
                for element in elements.iter_mut() {
                    if let Some(nested) = element {
                        if Self::fill_slot_recursive(nested, parent_id, slot_idx, child.clone()) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
        
        false
    }

    /// Start dragging a block from the palette
    pub fn start_drag_from_palette(&mut self, block: TypeBlock, position: Point<Pixels>) {
        self.drag_state = Some(DragState {
            dragging_block: Some(block),
            drag_start_pos: position,
            current_pos: position,
            hover_target: None,
        });
    }

    /// Start dragging an existing block from the canvas
    pub fn start_drag_from_canvas(&mut self, block_id: &BlockId, position: Point<Pixels>) {
        // Extract the block from its current position
        if let Some(root) = &mut self.root_block {
            if root.id() == block_id {
                // Dragging root itself
                let taken = self.root_block.take();
                if let Some(block) = taken {
                    self.drag_state = Some(DragState {
                        dragging_block: Some(block),
                        drag_start_pos: position,
                        current_pos: position,
                        hover_target: None,
                    });
                }
                return;
            }

            // TODO: Find and extract from nested slots
        }
    }

    /// Update drag position
    pub fn update_drag(&mut self, position: Point<Pixels>) {
        let target = self.find_drop_target(position);
        
        if let Some(drag) = &mut self.drag_state {
            drag.current_pos = position;
            drag.hover_target = target;
        }
    }

    /// Complete the drag operation
    pub fn end_drag(&mut self) -> bool {
        if let Some(drag) = self.drag_state.take() {
            if let (Some(block), Some(target)) = (drag.dragging_block, drag.hover_target) {
                return self.drop_block(block, target);
            }
        }
        false
    }

    /// Cancel the drag operation
    pub fn cancel_drag(&mut self) {
        self.drag_state = None;
    }

    /// Find where a block would be dropped at the given position
    fn find_drop_target(&self, _position: Point<Pixels>) -> Option<DropTarget> {
        // If hover_slot is set, use that
        if let Some((block_id, slot_index)) = &self.hover_slot {
            return Some(DropTarget::Slot {
                parent_block_id: block_id.clone(),
                slot_index: *slot_index,
            });
        }

        // If no root, can drop as root
        if self.root_block.is_none() {
            return Some(DropTarget::Root);
        }

        None
    }

    /// Actually drop a block into the target
    fn drop_block(&mut self, block: TypeBlock, target: DropTarget) -> bool {
        match target {
            DropTarget::Root => {
                self.root_block = Some(block);
                true
            }
            DropTarget::Slot { parent_block_id, slot_index } => {
                if let Some(root) = &mut self.root_block {
                    if let Some(parent) = root.find_block_mut(&parent_block_id) {
                        return parent.set_slot(slot_index, block);
                    }
                }
                false
            }
        }
    }

    /// Set hover target for a slot (called when mouse enters a slot)
    pub fn set_hover_slot(&mut self, block_id: BlockId, slot_index: usize) {
        self.hover_slot = Some((block_id, slot_index));
    }

    /// Clear hover target (called when mouse leaves a slot)
    pub fn clear_hover_slot(&mut self) {
        self.hover_slot = None;
    }

    /// Check if a slot is currently hovered
    pub fn is_slot_hovered(&self, block_id: &BlockId, slot_index: usize) -> bool {
        if let Some((hover_id, hover_idx)) = &self.hover_slot {
            hover_id == block_id && *hover_idx == slot_index
        } else {
            false
        }
    }

    /// Render the canvas
    pub fn render_with_handlers(
        &self, 
        cx: &App, 
        on_slot_click: Option<Arc<dyn Fn(BlockId, usize) + Send + Sync + 'static>>,
        on_empty_click: Option<Arc<dyn Fn() + Send + Sync + 'static>>
    ) -> impl IntoElement {
        let theme = cx.theme();
        
        v_flex()
            .flex_1()
            .w_full()
            .h_full()
            .bg(theme.muted.opacity(0.05))
            .rounded(px(8.0))
            .border_2()
            .border_color(theme.border)
            .p_6()
            .child(
                if let Some(root) = &self.root_block {
                    self.render_block_tree(root, cx, on_slot_click)
                } else {
                    self.render_empty_state(cx, on_empty_click)
                }
            )
            .when(self.drag_state.is_some(), |this| {
                this.child(self.render_drag_preview(cx))
            })
    }
    
    /// Render the canvas (convenience method without empty click handler)
    pub fn render(&self, cx: &App, on_slot_click: Option<Arc<dyn Fn(BlockId, usize) + Send + Sync + 'static>>) -> impl IntoElement {
        self.render_with_handlers(cx, on_slot_click, None)
    }

    fn render_empty_state(&self, cx: &App, on_empty_click: Option<Arc<dyn Fn() + Send + Sync + 'static>>) -> Div {
        let is_drag_over = self.drag_state.is_some();
        
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                // Large placeholder slot that looks clickable
                div()
                    .w(px(500.0))
                    .min_h(px(250.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .bg(cx.theme().secondary.opacity(0.3))
                    .rounded(px(16.0))
                    .border_3()
                    .border_color(if is_drag_over {
                        cx.theme().accent.opacity(0.6)
                    } else {
                        cx.theme().muted_foreground.opacity(0.3)
                    })
                    .border_dashed()
                    .p_8()
                    .when(on_empty_click.is_some(), |this| {
                        this.cursor_pointer()
                    })
                    .hover(|this| {
                        this.bg(cx.theme().secondary.opacity(0.5))
                            .border_color(cx.theme().muted_foreground.opacity(0.5))
                    })
                    .when_some(on_empty_click, |this, handler| {
                        this.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            handler();
                            cx.stop_propagation();
                        })
                    })
                    .child(
                        div()
                            .text_3xl()
                            .text_color(cx.theme().muted_foreground.opacity(0.4))
                            .child("🎯")
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child(if is_drag_over {
                                "Drop to create type"
                            } else {
                                "Click to add a type"
                            })
                    )
                    .child(
                        div()
                            .text_base()
                            .text_color(cx.theme().muted_foreground)
                            .child(if is_drag_over {
                                "Release to place the type here"
                            } else {
                                "Click here or use the Add Type button"
                            })
                    )
            )
    }

    fn render_block_tree(&self, block: &TypeBlock, _cx: &App, on_slot_click: Option<Arc<dyn Fn(BlockId, usize) + Send + Sync + 'static>>) -> Div {
        use crate::type_block::TypeBlockView;
        
        let mut view = TypeBlockView::new(
            block.clone(),
            "canvas-root"
        );
        
        if let Some(handler) = on_slot_click {
            view = view.on_slot_click(move |id, idx| handler(id, idx));
        }
        
        v_flex()
            .h_full()
            .items_start()
            .justify_start()
            .p_4()
            .child(view)
    }

    fn render_drag_preview(&self, _cx: &App) -> Div {
        if let Some(drag) = &self.drag_state {
            if let Some(block) = &drag.dragging_block {
                use crate::type_block::TypeBlockView;
                
                return div()
                    .absolute()
                    .left(drag.current_pos.x - px(50.0))
                    .top(drag.current_pos.y - px(20.0))
                    .opacity(0.7)
                    .shadow_lg()
                    .child(
                        TypeBlockView::new(
                            block.clone(),
                            "drag-preview"
                        )
                    );
            }
        }
        
        div()
    }
}
