use ui_common::command_palette::{PaletteDelegate, PaletteItem, CommandDelegate, CommandOrFile};
use ui_editor::tabs::blueprint_editor::node_palette::NodePalette;
use ui_editor::tabs::blueprint_editor::NodeDefinition;
use ui_alias_editor::{TypeLibraryPalette, TypeItem, BlockId};
use ui::IconName;
use gpui::Point;

/// Unified item type that can be any palette item
#[derive(Clone)]
pub enum AnyPaletteItem {
    CommandOrFile(CommandOrFile),
    Node(NodeDefinition),
    Type(TypeItem),
}

impl PaletteItem for AnyPaletteItem {
    fn name(&self) -> &str {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.name(),
            AnyPaletteItem::Node(item) => item.name(),
            AnyPaletteItem::Type(item) => item.name(),
        }
    }

    fn description(&self) -> &str {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.description(),
            AnyPaletteItem::Node(item) => item.description(),
            AnyPaletteItem::Type(item) => item.description(),
        }
    }

    fn icon(&self) -> IconName {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.icon(),
            AnyPaletteItem::Node(item) => item.icon(),
            AnyPaletteItem::Type(item) => item.icon(),
        }
    }

    fn keywords(&self) -> Vec<&str> {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.keywords(),
            AnyPaletteItem::Node(item) => item.keywords(),
            AnyPaletteItem::Type(item) => item.keywords(),
        }
    }

    fn documentation(&self) -> Option<String> {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.documentation(),
            AnyPaletteItem::Node(item) => item.documentation(),
            AnyPaletteItem::Type(item) => item.documentation(),
        }
    }
}

/// Unified delegate type that can be any palette delegate
pub enum AnyPaletteDelegate {
    Command(CommandDelegate),
    Node(NodePalette),
    TypeLibrary(TypeLibraryPalette),
}

impl AnyPaletteDelegate {
    pub fn command(project_root: Option<std::path::PathBuf>) -> Self {
        AnyPaletteDelegate::Command(CommandDelegate::new(project_root))
    }

    pub fn node(graph_position: Point<f32>) -> Self {
        AnyPaletteDelegate::Node(NodePalette::new(graph_position))
    }

    pub fn type_library(target_slot: Option<(BlockId, usize)>) -> Self {
        AnyPaletteDelegate::TypeLibrary(TypeLibraryPalette::new(target_slot))
    }

    /// Get the selected command/file if this is a command delegate
    pub fn take_selected_command(&mut self) -> Option<CommandOrFile> {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.take_selected_item(),
            _ => None,
        }
    }

    /// Get the selected node if this is a node delegate
    pub fn take_selected_node(&mut self) -> Option<(NodeDefinition, Point<f32>)> {
        match self {
            AnyPaletteDelegate::Node(delegate) => delegate.take_selected_node(),
            _ => None,
        }
    }

    /// Get the selected type if this is a type library delegate
    pub fn take_selected_type(&mut self) -> Option<(TypeItem, Option<(BlockId, usize)>)> {
        match self {
            AnyPaletteDelegate::TypeLibrary(delegate) => {
                delegate.take_selected_item().map(|item| (item, delegate.target_slot()))
            },
            _ => None,
        }
    }
}

impl PaletteDelegate for AnyPaletteDelegate {
    type Item = AnyPaletteItem;

    fn placeholder(&self) -> &str {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.placeholder(),
            AnyPaletteDelegate::Node(delegate) => delegate.placeholder(),
            AnyPaletteDelegate::TypeLibrary(delegate) => delegate.placeholder(),
        }
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate
                .categories()
                .into_iter()
                .map(|(cat, items)| {
                    (
                        cat,
                        items.into_iter().map(AnyPaletteItem::CommandOrFile).collect(),
                    )
                })
                .collect(),
            AnyPaletteDelegate::Node(delegate) => delegate
                .categories()
                .into_iter()
                .map(|(cat, items)| {
                    (
                        cat,
                        items.into_iter().map(AnyPaletteItem::Node).collect(),
                    )
                })
                .collect(),
            AnyPaletteDelegate::TypeLibrary(delegate) => delegate
                .categories()
                .into_iter()
                .map(|(cat, items)| {
                    (
                        cat,
                        items.into_iter().map(AnyPaletteItem::Type).collect(),
                    )
                })
                .collect(),
        }
    }

    fn confirm(&mut self, item: &Self::Item) {
        match (self, item) {
            (AnyPaletteDelegate::Command(delegate), AnyPaletteItem::CommandOrFile(item)) => {
                delegate.confirm(item);
            }
            (AnyPaletteDelegate::Node(delegate), AnyPaletteItem::Node(item)) => {
                delegate.confirm(item);
            }
            (AnyPaletteDelegate::TypeLibrary(delegate), AnyPaletteItem::Type(item)) => {
                delegate.confirm(item);
            }
            _ => {
                // Mismatch - this shouldn't happen
                eprintln!("Warning: Delegate/item type mismatch in confirm");
            }
        }
    }

    fn categories_collapsed_by_default(&self) -> bool {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.categories_collapsed_by_default(),
            AnyPaletteDelegate::Node(delegate) => delegate.categories_collapsed_by_default(),
            AnyPaletteDelegate::TypeLibrary(delegate) => delegate.categories_collapsed_by_default(),
        }
    }

    fn supports_docs(&self) -> bool {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.supports_docs(),
            AnyPaletteDelegate::Node(delegate) => delegate.supports_docs(),
            AnyPaletteDelegate::TypeLibrary(delegate) => delegate.supports_docs(),
        }
    }
}
