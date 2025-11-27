use gpui::{*, prelude::FluentBuilder, actions};
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, divider::Divider, button::{Button, ButtonVariant, ButtonVariants}};
use ui_types_common::{AliasAsset, TypeAstNode, PRIMITIVES, CONSTRUCTORS};
use std::path::PathBuf;
use crate::type_block::TypeBlock;

actions!(alias_editor, [Save]);

#[derive(Clone, Debug, PartialEq)]
pub enum OldTypeBlock {
    Empty,
    Primitive(String),
    Path(String),
    AliasRef(String),
    Constructor {
        name: String,
        params: Vec<OldTypeBlock>,
    },
    Tuple {
        elements: Vec<OldTypeBlock>,
    },
}

impl OldTypeBlock {
    fn to_ast_node(&self) -> Option<TypeAstNode> {
        match self {
            OldTypeBlock::Empty => None,
            OldTypeBlock::Primitive(name) => Some(TypeAstNode::Primitive { name: name.clone() }),
            OldTypeBlock::Path(path) => Some(TypeAstNode::Path { path: path.clone() }),
            OldTypeBlock::AliasRef(alias) => Some(TypeAstNode::AliasRef { alias: alias.clone() }),
            OldTypeBlock::Constructor { name, params } => {
                let param_nodes: Vec<TypeAstNode> = params
                    .iter()
                    .filter_map(|p| p.to_ast_node())
                    .collect();
                Some(TypeAstNode::Constructor {
                    name: name.clone(),
                    params: param_nodes,
                    lifetimes: vec![],
                    const_generics: vec![],
                })
            }
            OldTypeBlock::Tuple { elements } => {
                let element_nodes: Vec<TypeAstNode> = elements
                    .iter()
                    .filter_map(|e| e.to_ast_node())
                    .collect();
                Some(TypeAstNode::Tuple { elements: element_nodes })
            }
        }
    }

    fn from_ast_node(node: &TypeAstNode) -> Self {
        match node {
            TypeAstNode::Primitive { name } => OldTypeBlock::Primitive(name.clone()),
            TypeAstNode::Path { path } => OldTypeBlock::Path(path.clone()),
            TypeAstNode::AliasRef { alias } => OldTypeBlock::AliasRef(alias.clone()),
            TypeAstNode::Constructor { name, params, .. } => OldTypeBlock::Constructor {
                name: name.clone(),
                params: params.iter().map(Self::from_ast_node).collect(),
            },
            TypeAstNode::Tuple { elements } => OldTypeBlock::Tuple {
                elements: elements.iter().map(Self::from_ast_node).collect(),
            },
            TypeAstNode::FnPointer { .. } => {
                // For now, represent as a path
                OldTypeBlock::Path("FnPointer".to_string())
            }
        }
    }

    fn display_name(&self) -> String {
        match self {
            OldTypeBlock::Empty => "Empty".to_string(),
            OldTypeBlock::Primitive(name) => name.clone(),
            OldTypeBlock::Path(path) => path.clone(),
            OldTypeBlock::AliasRef(alias) => alias.clone(),
            OldTypeBlock::Constructor { name, .. } => name.clone(),
            OldTypeBlock::Tuple { .. } => "Tuple".to_string(),
        }
    }

    fn color(&self) -> Hsla {
        match self {
            OldTypeBlock::Empty => hsla(0.0, 0.0, 0.5, 1.0),
            OldTypeBlock::Primitive(_) => hsla(0.55, 0.7, 0.5, 1.0), // Blue
            OldTypeBlock::Path(_) => hsla(0.5, 0.7, 0.5, 1.0), // Cyan
            OldTypeBlock::AliasRef(_) => hsla(0.75, 0.7, 0.5, 1.0), // Purple
            OldTypeBlock::Constructor { name, .. } => {
                match name.as_str() {
                    "Box" | "Arc" | "Rc" => hsla(0.33, 0.7, 0.45, 1.0), // Green
                    "Vec" | "HashMap" | "HashSet" => hsla(0.83, 0.7, 0.5, 1.0), // Magenta
                    "Option" | "Result" => hsla(0.0, 0.7, 0.5, 1.0), // Red/Pink
                    _ => hsla(0.15, 0.7, 0.5, 1.0), // Orange
                }
            }
            OldTypeBlock::Tuple { .. } => hsla(0.66, 0.7, 0.5, 1.0), // Blue-Purple
        }
    }
}

pub struct AliasEditor {
    file_path: Option<PathBuf>,
    name: String,
    display_name: String,
    description: String,
    root_block: OldTypeBlock,
    selected_path: Vec<usize>, // Path to selected block in tree
    show_palette: bool,
    error_message: Option<String>,
    focus_handle: FocusHandle,
}

impl AliasEditor {
    pub fn new_with_file(file_path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the alias data
        let (name, display_name, description, root_block, error_message) =
            match std::fs::read_to_string(&file_path) {
                Ok(json_content) => {
                    match serde_json::from_str::<AliasAsset>(&json_content) {
                        Ok(asset) => (
                            asset.name.clone(),
                            asset.display_name.clone(),
                            asset.description.unwrap_or_default(),
                            OldTypeBlock::from_ast_node(&asset.ast),
                            None,
                        ),
                        Err(e) => (
                            String::new(),
                            "New Alias".to_string(),
                            String::new(),
                            OldTypeBlock::Empty,
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
                        OldTypeBlock::Empty,
                        None,
                    )
                }
            };

        // No need to register action handler - we'll handle it in on_click

        Self {
            file_path: Some(file_path),
            name,
            display_name,
            description,
            root_block,
            selected_path: vec![],
            show_palette: false,
            error_message,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }

    fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file_path) = &self.file_path {
            if let Some(ast) = self.root_block.to_ast_node() {
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
                            eprintln!("Saved type alias to {:?}", file_path);
                            // TODO: Sync to project type index
                        }
                    }
                    Err(e) => {
                        self.error_message = Some(format!("Failed to serialize: {}", e));
                    }
                }
            } else {
                self.error_message = Some("Cannot save empty type".to_string());
            }
        }
        cx.notify();
    }

    fn render_block(&self, block: &OldTypeBlock, depth: usize, path: Vec<usize>, cx: &mut Context<Self>) -> Div {
        let is_selected = path == self.selected_path;
        let color = block.color();

        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .p_3()
                    .ml(px(depth as f32 * 20.0))
                    .bg(color.opacity(0.8))
                    .rounded(px(6.0))
                    .when(is_selected, |this| this.border_2().border_color(cx.theme().accent))
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                            .child(block.display_name())
                    )
            )
            .when(matches!(block, OldTypeBlock::Constructor { params, .. } if !params.is_empty()), |this| {
                if let OldTypeBlock::Constructor { params, .. } = block {
                    let mut result = this;
                    for (i, param) in params.iter().enumerate() {
                        let mut param_path = path.clone();
                        param_path.push(i);
                        result = result.child(self.render_block(param, depth + 1, param_path, cx));
                    }
                    result
                } else {
                    this
                }
            })
            .when(matches!(block, OldTypeBlock::Tuple { elements, .. } if !elements.is_empty()), |this| {
                if let OldTypeBlock::Tuple { elements } = block {
                    let mut result = this;
                    for (i, element) in elements.iter().enumerate() {
                        let mut elem_path = path.clone();
                        elem_path.push(i);
                        result = result.child(self.render_block(element, depth + 1, elem_path, cx));
                    }
                    result
                } else {
                    this
                }
            })
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_3()
            .p_4()
            .bg(cx.theme().secondary.opacity(0.9))
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.0))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("Primitives")
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(PRIMITIVES.iter().take(10).map(|&prim| {
                        Button::new(prim)
                            .with_variant(ButtonVariant::Ghost)
                            .child(prim)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.root_block = OldTypeBlock::Primitive(prim.to_string());
                                this.show_palette = false;
                                cx.notify();
                            }))
                    }))
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("Constructors")
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(CONSTRUCTORS.iter().map(|&cons| {
                        Button::new(cons)
                            .with_variant(ButtonVariant::Ghost)
                            .child(cons)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.root_block = OldTypeBlock::Constructor {
                                    name: cons.to_string(),
                                    params: vec![OldTypeBlock::Empty],
                                };
                                this.show_palette = false;
                                cx.notify();
                            }))
                    }))
            )
    }
}

impl Render for AliasEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Header with metadata inputs
                v_flex()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .bg(cx.theme().secondary.opacity(0.5))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().text_xl().child("🔗"))
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(self.display_name.clone())
                            )
                            .child(
                                Button::new("save")
                                    .with_variant(ButtonVariant::Primary)
                                    .child("Save")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save(&Save, window, cx);
                                    }))
                            )
                    )
                    .when(!self.name.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("name: {}", &self.name))
                        )
                    })
                    .when(!self.description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.description.clone())
                        )
                    })
            )
            .child(
                // Main editor area
                v_flex()
                    .flex_1()
                    .p_4()
                    .gap_4()
                    .when(self.error_message.is_some(), |this| {
                        let error = self.error_message.as_ref().unwrap();
                        this.child(
                            div()
                                .p_4()
                                .bg(hsla(0.0, 0.8, 0.5, 0.1))
                                .border_1()
                                .border_color(hsla(0.0, 0.8, 0.5, 1.0))
                                .rounded(px(6.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(hsla(0.0, 0.8, 0.5, 1.0))
                                        .child(error.clone())
                                )
                        )
                    })
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Type Definition")
                            )
                            .child(
                                Button::new("toggle_palette")
                                    .with_variant(ButtonVariant::Secondary)
                                    .child(if self.show_palette { "Hide Types" } else { "Add Type" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_palette = !this.show_palette;
                                        cx.notify();
                                    }))
                            )
                    )
                    .when(self.show_palette, |this| {
                        this.child(self.render_palette(cx))
                    })
                    .child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .bg(cx.theme().background.blend(cx.theme().secondary.opacity(0.1)))
                            .rounded(px(8.0))
                            .min_h(px(200.0))
                            .child(self.render_block(&self.root_block, 0, vec![], cx))
                    )
            )
    }
}

impl Focusable for AliasEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for AliasEditor {}

impl Panel for AliasEditor {
    fn panel_name(&self) -> &'static str {
        "Type Alias Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        if !self.display_name.is_empty() {
            self.display_name.clone()
        } else {
            "Alias".to_string()
        }
        .into_any_element()
    }
}
