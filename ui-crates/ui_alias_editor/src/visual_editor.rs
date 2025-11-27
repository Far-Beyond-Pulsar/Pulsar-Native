use gpui::{*, prelude::FluentBuilder, actions};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, Colorize, 
    dock::{Panel, PanelEvent}, 
    button::{Button, ButtonVariant, ButtonVariants}, 
    divider::Divider,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::{InputState, TextInput},
};
use ui_types_common::{AliasAsset, TypeAstNode};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crate::{TypeBlock, BlockId, BlockCanvas, ConstructorPalette};

actions!(visual_alias_editor, [Save, TogglePalette]);

#[derive(Clone)]
pub struct ShowTypePickerRequest {
    pub target_slot: Option<(BlockId, usize)>,
}

/// Visual block-based type alias editor with Scratch-style interface
pub struct VisualAliasEditor {
    file_path: Option<PathBuf>,
    name: String,
    display_name: String,
    description: String,
    
    /// Canvas for composing type blocks
    canvas: BlockCanvas,
    
    /// Code preview input state
    preview_input: Entity<InputState>,
    
    /// Resizable state for canvas/preview split
    horizontal_resizable_state: Entity<ResizableState>,
    
    /// Flag to update preview on next render
    preview_needs_update: bool,
    
    /// Error message to display
    error_message: Option<String>,
    
    /// Code preview panel visible
    show_preview: bool,
    
    focus_handle: FocusHandle,
    
    /// Currently selected slot to fill (parent_block_id, slot_index)
    selected_slot: Option<(BlockId, usize)>,
    
    /// Block pending placement (from palette)
    pending_block: Option<TypeBlock>,
    
    /// Pending slot selection (shared state for click handler)
    pending_slot_selection: Arc<Mutex<Option<(BlockId, usize)>>>,
}

impl VisualAliasEditor {
    pub fn new_with_file(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the alias data
        let (name, display_name, description, root_block, error_message) =
            match std::fs::read_to_string(&file_path) {
                Ok(json_content) => {
                    match serde_json::from_str::<AliasAsset>(&json_content) {
                        Ok(asset) => (
                            asset.name.clone(),
                            asset.display_name.clone(),
                            asset.description.unwrap_or_default(),
                            Some(TypeBlock::from_ast(&asset.ast)),
                            None,
                        ),
                        Err(e) => (
                            String::new(),
                            "New Alias".to_string(),
                            String::new(),
                            None,
                            Some(format!("Failed to parse: {}", e)),
                        ),
                    }
                }
                Err(_) => {
                    // New file
                    (
                        String::new(),
                        "New Alias".to_string(),
                        String::new(),
                        None,
                        None,
                    )
                }
            };

        let canvas = if let Some(block) = root_block {
            BlockCanvas::with_root(block)
        } else {
            BlockCanvas::new()
        };
        
        let horizontal_resizable_state = ResizableState::new(cx);
        
        // Create preview input with code editor setup (same as script editor)
        let preview_input = cx.new(|cx| {
            use ui::input::TabSize;
            InputState::new(window, cx)
                .code_editor("rust")
                .line_number(true)
                .minimap(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
        });
        
        let mut editor = Self {
            file_path: Some(file_path),
            name,
            display_name,
            description,
            canvas,
            preview_input,
            horizontal_resizable_state,
            preview_needs_update: true,
            error_message,
            show_preview: true,
            focus_handle: cx.focus_handle(),
            selected_slot: None,
            pending_block: None,
            pending_slot_selection: Arc::new(Mutex::new(None)),
        };
        
        // Initialize preview input with current content
        editor.update_preview(window, cx);
        editor.preview_needs_update = false;
        
        editor
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }

    fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file_path) = &self.file_path {
            if let Some(root_block) = self.canvas.root_block() {
                if let Some(ast) = root_block.to_ast() {
                    let asset = AliasAsset {
                        schema_version: 1,
                        type_kind: ui_types_common::TypeKind::Alias,
                        name: self.name.clone(),
                        display_name: self.display_name.clone(),
                        description: if self.description.is_empty() {
                            None
                        } else {
                            Some(self.description.clone())
                        },
                        ast,
                        meta: serde_json::Value::Object(serde_json::Map::new()),
                    };

                    match serde_json::to_string_pretty(&asset) {
                        Ok(json) => {
                            if let Err(e) = std::fs::write(file_path, json) {
                                self.error_message = Some(format!("Failed to save: {}", e));
                            } else {
                                self.error_message = None;
                                // TODO: Generate Rust code and update type index
                                eprintln!("✅ Saved type alias to {:?}", file_path);
                            }
                        }
                        Err(e) => {
                            self.error_message = Some(format!("Failed to serialize: {}", e));
                        }
                    }
                } else {
                    self.error_message = Some("Type has empty slots - fill all slots before saving".to_string());
                }
            } else {
                self.error_message = Some("Cannot save empty type".to_string());
            }
        }
        cx.notify();
    }

    fn toggle_palette(&mut self, _: &TogglePalette, _window: &mut Window, cx: &mut Context<Self>) {
        // Open the centered type picker with no target slot
        cx.emit(ShowTypePickerRequest {
            target_slot: self.selected_slot.clone(),
        });
    }



    /// Add a block to the canvas
    fn add_block_to_canvas(&mut self, block: TypeBlock, cx: &mut Context<Self>) {
        if self.canvas.root_block().is_none() {
            // No root block yet - place as root
            self.canvas.set_root_block(Some(block));
            self.error_message = None;
            self.pending_block = None;
            self.selected_slot = None;
        } else if let Some((parent_id, slot_idx)) = &self.selected_slot {
            // Slot is selected - fill it
            if self.canvas.fill_slot(parent_id.clone(), *slot_idx, block) {
                self.error_message = None;
                self.selected_slot = None;
                self.pending_block = None;
            } else {
                self.error_message = Some("Failed to fill slot".to_string());
            }
        } else {
            // Has root but no slot selected - store as pending and prompt user
            self.pending_block = Some(block);
            self.error_message = Some("Click on an empty slot to place this type".to_string());
        }
        self.preview_needs_update = true;
        cx.notify();
    }
    
    /// Select a slot to fill - opens the type picker
    fn select_slot(&mut self, parent_id: BlockId, slot_idx: usize, cx: &mut Context<Self>) {
        self.selected_slot = Some((parent_id.clone(), slot_idx));
        
        // If we have a pending block, fill the slot immediately
        if let Some(block) = self.pending_block.take() {
            self.add_block_to_canvas(block, cx);
        } else {
            // Open the centered type picker for this slot
            cx.emit(ShowTypePickerRequest {
                target_slot: Some((parent_id, slot_idx)),
            });
        }
    }
    
    /// Add a block from the type picker
    pub fn add_type_from_picker(&mut self, type_item: &crate::TypeItem, target_slot: Option<(BlockId, usize)>, cx: &mut Context<Self>) {
        let block = type_item.to_block();
        
        if let Some((parent_id, slot_idx)) = target_slot {
            // Fill the specific slot
            if self.canvas.fill_slot(parent_id, slot_idx, block) {
                self.error_message = None;
                self.selected_slot = None;
            } else {
                self.error_message = Some("Failed to fill slot".to_string());
            }
        } else {
            // No slot specified - add to canvas
            self.add_block_to_canvas(block, cx);
        }
        self.preview_needs_update = true;
        cx.notify();
    }
    
    /// Update the preview input with current code
    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = if let Some(root) = self.canvas.root_block() {
            if let Some(ast) = root.to_ast() {
                self.generate_preview_code(&ast)
            } else {
                "// Fill all slots to see generated code".to_string()
            }
        } else {
            "// Click to add a type or use the Add Type button".to_string()
        };
        
        self.preview_input.update(cx, |input, cx| {
            input.set_value(&code, window, cx);
        });
    }



    fn generate_preview_code(&self, ast: &TypeAstNode) -> String {
        let type_str = self.ast_to_rust_string(ast);
        
        format!(
            "// Auto-generated Rust type alias\n\
             pub type {} = {};\n\n\
             // Usage example:\n\
             // let value: {} = ...;",
            self.display_name,
            type_str,
            self.display_name
        )
    }

    fn ast_to_rust_string(&self, ast: &TypeAstNode) -> String {
        match ast {
            TypeAstNode::Primitive { name } => name.clone(),
            TypeAstNode::Path { path } => path.clone(),
            TypeAstNode::AliasRef { alias } => alias.clone(),
            TypeAstNode::Constructor { name, params, .. } => {
                let params_str = params
                    .iter()
                    .map(|p| self.ast_to_rust_string(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", name, params_str)
            }
            TypeAstNode::Tuple { elements } => {
                let elements_str = elements
                    .iter()
                    .map(|e| self.ast_to_rust_string(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", elements_str)
            }
            TypeAstNode::FnPointer { params, return_type } => {
                let params_str = params
                    .iter()
                    .map(|p| self.ast_to_rust_string(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({}) -> {}", params_str, self.ast_to_rust_string(return_type))
            }
        }
    }
}

impl Render for VisualAliasEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update preview if needed
        if self.preview_needs_update {
            self.update_preview(window, cx);
            self.preview_needs_update = false;
        }
        
        // Check for pending slot selection from click handler
        let pending_selection = if let Ok(mut guard) = self.pending_slot_selection.lock() {
            guard.take()
        } else {
            None
        };
        
        if let Some((block_id, slot_idx)) = pending_selection {
            // Special case: empty BlockId indicates empty state click (add root)
            if block_id.0.is_empty() {
                // Open type picker for root (no target slot)
                cx.emit(ShowTypePickerRequest {
                    target_slot: None,
                });
            } else {
                self.select_slot(block_id, slot_idx, cx);
            }
        }
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Top toolbar
                h_flex()
                    .w_full()
                    .px_4()
                    .py_3()
                    .gap_4()
                    .bg(cx.theme().secondary.opacity(0.5))
                    .border_b_2()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        // Icon and title
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().text_xl().child("🔗"))
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(if !self.display_name.is_empty() {
                                        self.display_name.clone()
                                    } else {
                                        "New Type Alias".to_string()
                                    })
                            )
                    )
                    .child(
                        // Spacer
                        div().flex_1()
                    )
                    .child(
                        // Action buttons
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("add_type_btn")
                                    .with_variant(ButtonVariant::Secondary)
                                    .child("🎨 Add Type")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_palette(&TogglePalette, window, cx);
                                    }))
                            )
                            .child(
                                Button::new("toggle_preview_btn")
                                    .with_variant(if self.show_preview {
                                        ButtonVariant::Secondary
                                    } else {
                                        ButtonVariant::Ghost
                                    })
                                    .child(if self.show_preview { "📋 Hide Preview" } else { "📋 Show Preview" })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.show_preview = !this.show_preview;
                                        cx.notify();
                                    }))
                            )
                            .child(Divider::vertical().h(px(24.0)))
                            .child(
                                Button::new("save_btn")
                                    .with_variant(ButtonVariant::Primary)
                                    .child("💾 Save")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save(&Save, window, cx);
                                    }))
                            )
                    )
            )
            .child(
                // Main content area - resizable canvas and preview
                h_resizable("alias-editor-horizontal", self.horizontal_resizable_state.clone())
                    .child(
                        resizable_panel()
                            .child(
                                // Canvas container
                                v_flex()
                                    .size_full()
                                    .p_4()
                                    .gap_4()
                                    .when(self.error_message.is_some(), |this| {
                                        let error = self.error_message.as_ref().unwrap();
                                        this.child(
                                            div()
                                                .w_full()
                                                .p_4()
                                                .bg(hsla(0.0, 0.8, 0.5, 0.1))
                                                .border_2()
                                                .border_color(hsla(0.0, 0.8, 0.6, 1.0))
                                                .rounded(px(8.0))
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_base()
                                                                .child("⚠️")
                                                        )
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(hsla(0.0, 0.8, 0.5, 1.0))
                                                                .child(error.clone())
                                                        )
                                                )
                                        )
                                    })
                                    .child({
                                        // Canvas - fills remaining space
                                        // Create a handler that stores slot clicks in shared state
                                        let pending = self.pending_slot_selection.clone();
                                        let slot_handler = Arc::new(move |block_id: BlockId, slot_idx: usize| {
                                            if let Ok(mut guard) = pending.lock() {
                                                *guard = Some((block_id, slot_idx));
                                            }
                                        });
                                        
                                        // Create handler for empty state click - opens type picker for root
                                        let pending_empty = self.pending_slot_selection.clone();
                                        let empty_handler = Arc::new(move || {
                                            // Signal that we want to add a root type (no specific slot)
                                            // Use empty BlockId as sentinel value
                                            if let Ok(mut guard) = pending_empty.lock() {
                                                *guard = Some((BlockId(Arc::from("")), 0));
                                            }
                                        });
                                        
                                        self.canvas.render_with_handlers(cx, Some(slot_handler), Some(empty_handler))
                                    })
                            )
                    )
                    .when(self.show_preview, |this| {
                        this.child(
                            resizable_panel()
                                .size(px(500.))
                                .size_range(px(300.)..px(800.))
                                .child(
                                    v_flex()
                                        .size_full()
                                        .bg(cx.theme().sidebar)
                                        .border_l_2()
                                        .border_color(cx.theme().border)
                                        .child(
                                            // Header
                                            h_flex()
                                                .w_full()
                                                .px_4()
                                                .py_3()
                                                .bg(cx.theme().secondary)
                                                .border_b_2()
                                                .border_color(cx.theme().border)
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_bold()
                                                        .text_color(cx.theme().foreground)
                                                        .child("📋 Code Preview")
                                                )
                                        )
                                        .child(
                                            // Code input - fills remaining space
                                            div()
                                                .flex_1()
                                                .w_full()
                                                .p_2()
                                                .child(
                                                    TextInput::new(&self.preview_input)
                                                        .h_full()
                                                        .w_full()
                                                        .appearance(false)
                                                        .font_family("monospace")
                                                        .font(gpui::Font {
                                                            family: "Jetbrains Mono".to_string().into(),
                                                            weight: gpui::FontWeight::NORMAL,
                                                            style: gpui::FontStyle::Normal,
                                                            features: gpui::FontFeatures::default(),
                                                            fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                                                        })
                                                        .text_size(px(14.0))
                                                )
                                        )
                                )
                        )
                    })
            )
            .when(!self.name.is_empty() && !self.description.is_empty(), |this| {
                // Bottom info bar
                this.child(
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_2()
                        .gap_4()
                        .bg(cx.theme().secondary.opacity(0.3))
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("name: {}", &self.name))
                        )
                        .child(Divider::vertical().h(px(12.0)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.description.clone())
                        )
                )
            })
    }
}

impl Focusable for VisualAliasEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for VisualAliasEditor {}
impl EventEmitter<ShowTypePickerRequest> for VisualAliasEditor {}

impl Panel for VisualAliasEditor {
    fn panel_name(&self) -> &'static str {
        "Visual Type Alias Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        if !self.display_name.is_empty() {
            format!("🔗 {}", self.display_name)
        } else {
            "🔗 New Type Alias".to_string()
        }
        .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}
