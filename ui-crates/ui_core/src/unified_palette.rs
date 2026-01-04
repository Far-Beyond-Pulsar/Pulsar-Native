use ui_common::command_palette::{PaletteDelegate, PaletteItem, CommandDelegate, CommandOrFile};
// use ui_alias_editor::{TypeLibraryPalette, TypeItem, BlockId}; // Migrated to plugins
use ui::IconName;

// NOTE: Type library palette functionality has been migrated to plugins
// Keeping only command palette for now

/// Unified item type that can be any palette item
#[derive(Clone)]
pub enum AnyPaletteItem {
    CommandOrFile(CommandOrFile),
    // Type(TypeItem), // Migrated to plugins
}

impl PaletteItem for AnyPaletteItem {
    fn name(&self) -> &str {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.name(),
        }
    }

    fn description(&self) -> &str {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.description(),
        }
    }

    fn icon(&self) -> IconName {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.icon(),
        }
    }

    fn keywords(&self) -> Vec<&str> {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.keywords(),
        }
    }

    fn documentation(&self) -> Option<String> {
        match self {
            AnyPaletteItem::CommandOrFile(item) => item.documentation(),
        }
    }
}

/// Unified delegate type that can be any palette delegate
pub enum AnyPaletteDelegate {
    Command(CommandDelegate),
    // TypeLibrary(TypeLibraryPalette), // Migrated to plugins
}

impl AnyPaletteDelegate {
    pub fn command(project_root: Option<std::path::PathBuf>) -> Self {
        AnyPaletteDelegate::Command(CommandDelegate::new(project_root))
    }

    // pub fn type_library(target_slot: Option<(BlockId, usize)>) -> Self {
    //     AnyPaletteDelegate::TypeLibrary(TypeLibraryPalette::new(target_slot))
    // }

    /// Get the selected command/file if this is a command delegate
    pub fn take_selected_command(&mut self) -> Option<CommandOrFile> {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.take_selected_item(),
        }
    }

    // Get the selected type if this is a type library delegate - migrated to plugins
    // pub fn take_selected_type(&mut self) -> Option<(TypeItem, Option<(BlockId, usize)>)> {
    //     match self {
    //         AnyPaletteDelegate::TypeLibrary(delegate) => {
    //             delegate.take_selected_item().map(|item| (item, delegate.target_slot()))
    //         },
    //         _ => None,
    //     }
    // }
}

impl PaletteDelegate for AnyPaletteDelegate {
    type Item = AnyPaletteItem;

    fn placeholder(&self) -> &str {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.placeholder(),
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
        }
    }

    fn confirm(&mut self, item: &Self::Item) {
        match (self, item) {
            (AnyPaletteDelegate::Command(delegate), AnyPaletteItem::CommandOrFile(item)) => {
                delegate.confirm(item);
            }
        }
    }

    fn categories_collapsed_by_default(&self) -> bool {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.categories_collapsed_by_default(),
        }
    }

    fn supports_docs(&self) -> bool {
        match self {
            AnyPaletteDelegate::Command(delegate) => delegate.supports_docs(),
        }
    }
}
