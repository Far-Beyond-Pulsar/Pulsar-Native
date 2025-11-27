use ui::IconName;
use ui_common::command_palette::{PaletteDelegate, PaletteItem};
use crate::{TypeBlock, BlockId};

#[derive(Clone)]
pub enum TypeItem {
    Primitive(String),
    Constructor { name: String, params_count: usize, description: String },
}

impl PaletteItem for TypeItem {
    fn name(&self) -> &str {
        match self {
            TypeItem::Primitive(name) => name,
            TypeItem::Constructor { name, .. } => name,
        }
    }

    fn description(&self) -> &str {
        match self {
            TypeItem::Primitive(_) => "Primitive type",
            TypeItem::Constructor { description, .. } => description,
        }
    }

    fn icon(&self) -> IconName {
        match self {
            TypeItem::Primitive(_) => IconName::Code,
            TypeItem::Constructor { .. } => IconName::Box,
        }
    }

    fn keywords(&self) -> Vec<&str> {
        vec![]
    }

    fn documentation(&self) -> Option<String> {
        None
    }
}

pub struct TypeLibraryPalette {
    categories: Vec<(String, Vec<TypeItem>)>,
    selected_item: Option<TypeItem>,
    target_slot: Option<(BlockId, usize)>,
}

impl TypeLibraryPalette {
    pub fn new(target_slot: Option<(BlockId, usize)>) -> Self {
        use pulsar_std::get_all_type_constructors;
        use ui_types_common::PRIMITIVES;
        use std::collections::HashMap;

        let mut categories: Vec<(String, Vec<TypeItem>)> = Vec::new();

        // Add primitives category
        let primitives: Vec<TypeItem> = PRIMITIVES
            .iter()
            .map(|&name| TypeItem::Primitive(name.to_string()))
            .collect();
        categories.push(("Primitives".to_string(), primitives));

        // Group constructors by category
        let constructors = get_all_type_constructors();
        let mut by_category: HashMap<&str, Vec<TypeItem>> = HashMap::new();
        for ctor in constructors {
            by_category
                .entry(ctor.category)
                .or_insert_with(Vec::new)
                .push(TypeItem::Constructor {
                    name: ctor.name.to_string(),
                    params_count: ctor.params_count,
                    description: ctor.description.to_string(),
                });
        }

        // Sort categories for stable order
        let mut category_list: Vec<_> = by_category.into_iter().collect();
        category_list.sort_by_key(|(name, _)| *name);

        for (category_name, items) in category_list {
            categories.push((category_name.to_string(), items));
        }

        Self {
            categories,
            selected_item: None,
            target_slot,
        }
    }

    pub fn take_selected_item(&mut self) -> Option<TypeItem> {
        self.selected_item.take()
    }

    pub fn target_slot(&self) -> Option<(BlockId, usize)> {
        self.target_slot.clone()
    }
}

impl PaletteDelegate for TypeLibraryPalette {
    type Item = TypeItem;

    fn placeholder(&self) -> &str {
        "Search for types..."
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        self.categories.clone()
    }

    fn confirm(&mut self, item: &Self::Item) {
        self.selected_item = Some(item.clone());
    }

    fn categories_collapsed_by_default(&self) -> bool {
        true
    }

    fn supports_docs(&self) -> bool {
        false
    }
}

impl TypeItem {
    pub fn to_block(&self) -> TypeBlock {
        match self {
            TypeItem::Primitive(name) => TypeBlock::primitive(name),
            TypeItem::Constructor { name, params_count, .. } => {
                TypeBlock::constructor(name, *params_count)
            }
        }
    }
}
