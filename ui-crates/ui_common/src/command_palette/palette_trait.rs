use ui::IconName;

/// Trait for items that can be displayed in a palette
/// Provides display information only - no rendering logic
pub trait PaletteItem: Clone + 'static {
    /// Display name of the item
    fn name(&self) -> &str;

    /// Description/subtitle
    fn description(&self) -> &str;

    /// Icon to display
    fn icon(&self) -> IconName;

    /// Keywords for searching (default: empty)
    fn keywords(&self) -> Vec<&str> {
        vec![]
    }

    /// Optional documentation text (default: None)
    fn documentation(&self) -> Option<String> {
        None
    }
}

/// Trait for palette delegates that provide items and handle selection
/// Delegates provide data only - CommandPalette handles all rendering
pub trait PaletteDelegate: 'static {
    /// The type of items this palette provides
    type Item: PaletteItem;

    /// Placeholder text for the search input
    fn placeholder(&self) -> &str;

    /// Get all items grouped by categories
    /// Returns (category_name, items) tuples. Use empty string for uncategorized items.
    fn categories(&self) -> Vec<(String, Vec<Self::Item>)>;

    /// Filter items based on search query
    /// Default implementation filters by name, description, and keywords
    fn filter(&self, query: &str) -> Vec<(String, Vec<Self::Item>)> {
        if query.is_empty() {
            return self.categories();
        }

        let query_lower = query.to_lowercase();

        self.categories()
            .into_iter()
            .map(|(category, items)| {
                let filtered: Vec<Self::Item> = items
                    .into_iter()
                    .filter(|item| {
                        item.name().to_lowercase().contains(&query_lower)
                            || item.description().to_lowercase().contains(&query_lower)
                            || item.keywords().iter().any(|kw| {
                                kw.to_lowercase().contains(&query_lower)
                            })
                    })
                    .collect();
                (category, filtered)
            })
            .filter(|(_, items)| !items.is_empty())
            .collect()
    }

    /// Handle item selection/confirmation
    /// The delegate should emit appropriate events through the parent context
    fn confirm(&mut self, item: &Self::Item);

    /// Whether categories should start collapsed (default: false)
    fn categories_collapsed_by_default(&self) -> bool {
        false
    }

    /// Whether this palette supports documentation panel (default: based on items)
    fn supports_docs(&self) -> bool {
        // Check if any category has items with documentation
        self.categories().iter().any(|(_, items)| {
            items.iter().any(|item| item.documentation().is_some())
        })
    }
}
