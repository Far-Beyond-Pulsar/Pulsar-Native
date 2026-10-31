//! Flat, filterable model behind the virtualized properties panels.
//!
//! An inspector with thousands of properties cannot build an element per row:
//! it is laid out and prepainted whether or not it is on screen.  Instead a
//! panel describes its content as [`CardDesc`]s (cheap metadata, no elements),
//! [`flatten`] turns those into one list of [`InspectorItem`]s honouring the
//! search query and the collapse state, and the panel renders only the items
//! that scroll into view (`gpui::list`).
//!
//! This module knows nothing about widgets or GPUI; everything here is plain
//! data so the search and collapse rules are unit-tested.

use std::collections::{HashMap, HashSet};

/// One property row as the model sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PropDesc {
    /// Reflected field name (also the editor-cache key).
    pub name: String,
    pub display_name: String,
    pub category: Option<String>,
    pub category_color: Option<String>,
    pub category_default_collapsed: bool,
    pub category_order: Option<usize>,
    /// Rows this property offers below its own (see
    /// [`pulsar_reflection::VirtualRows`]); they follow it in the list.
    pub extra_rows: usize,
}

/// One component card.
#[derive(Clone, Debug, PartialEq)]
pub struct CardDesc {
    /// Unique per component INSTANCE (`"{class}#{index}"`).
    pub key: String,
    pub title: String,
    pub props: Vec<PropDesc>,
    /// A card whose body is a single custom element (no per-property rows).
    pub custom: bool,
    /// A warning line under the card title (for example an unfinished class).
    pub note: Option<String>,
}

/// One entry of the flattened list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InspectorItem {
    /// A card header (the collapse toggle).
    Card { card: usize },
    /// A category header inside a card.
    Category { card: usize, name: String },
    /// A property row; `prop` indexes `CardDesc::props`.
    Property { card: usize, prop: usize },
    /// Row `ix` of the rows property `prop` offers below its own.
    Child { card: usize, prop: usize, ix: usize },
    /// The whole body of a `custom` card.
    Body { card: usize },
}

/// Which cards and categories the user collapsed or expanded.
#[derive(Default, Clone, Debug)]
pub struct CollapseState {
    collapsed_cards: HashSet<String>,
    collapsed_categories: HashSet<(String, String)>,
    expanded_categories: HashSet<(String, String)>,
}

impl CollapseState {
    pub fn is_card_collapsed(&self, card_key: &str) -> bool {
        self.collapsed_cards.contains(card_key)
    }

    pub fn toggle_card(&mut self, card_key: &str) {
        if !self.collapsed_cards.remove(card_key) {
            self.collapsed_cards.insert(card_key.to_string());
        }
    }

    /// Collapse or expand every card in `keys`.
    pub fn set_all_cards(&mut self, keys: impl IntoIterator<Item = String>, collapsed: bool) {
        if collapsed {
            self.collapsed_cards.extend(keys);
        } else {
            self.collapsed_cards.clear();
        }
    }

    pub fn is_category_collapsed(
        &self,
        card_key: &str,
        category: &str,
        default_collapsed: bool,
    ) -> bool {
        let key = (card_key.to_string(), category.to_string());
        if self.collapsed_categories.contains(&key) {
            true
        } else if self.expanded_categories.contains(&key) {
            false
        } else {
            default_collapsed
        }
    }

    pub fn toggle_category(&mut self, card_key: &str, category: &str, default_collapsed: bool) {
        let was_collapsed = self.is_category_collapsed(card_key, category, default_collapsed);
        let key = (card_key.to_string(), category.to_string());
        if was_collapsed {
            self.collapsed_categories.remove(&key);
            self.expanded_categories.insert(key);
        } else {
            self.expanded_categories.remove(&key);
            self.collapsed_categories.insert(key);
        }
    }
}

/// The search query split into lowercase tokens.  A row matches when every
/// token appears in its haystack, so `"light int"` finds `Light > Intensity`.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Query {
    tokens: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        Self {
            tokens: text.split_whitespace().map(str::to_lowercase).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    fn matches(&self, haystack: &str) -> bool {
        let haystack = haystack.to_lowercase();
        self.tokens.iter().all(|t| haystack.contains(t.as_str()))
    }
}

/// Flatten `cards` into the list of items to show.
///
/// * No query: cards honour their collapse state, categories theirs.
/// * With a query: collapse state is ignored (matches must be visible), cards
///   with no match are dropped, and a card whose title alone matches shows
///   every property.
pub fn flatten(cards: &[CardDesc], query: &Query, collapse: &CollapseState) -> Vec<InspectorItem> {
    let mut items = Vec::new();
    for (card_ix, card) in cards.iter().enumerate() {
        if card.custom {
            if !query.is_empty() && !query.matches(&card.title) {
                continue;
            }
            items.push(InspectorItem::Body { card: card_ix });
            continue;
        }

        let title_matches = query.is_empty() || query.matches(&card.title);
        let visible: Vec<usize> = card
            .props
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                title_matches || {
                    query.matches(&format!(
                        "{} {} {} {}",
                        card.title,
                        p.category.as_deref().unwrap_or(""),
                        p.display_name,
                        p.name
                    ))
                }
            })
            .map(|(ix, _)| ix)
            .collect();
        if visible.is_empty() {
            continue;
        }

        items.push(InspectorItem::Card { card: card_ix });
        if query.is_empty() && collapse.is_card_collapsed(&card.key) {
            continue;
        }

        // Uncategorised rows first, then categories by `category_order`
        // (first appearance breaks ties), as the eager panels always did.
        // A property that offers rows of its own (a long array) is followed
        // by them, so one list virtualizes the lot.
        let push_property = |items: &mut Vec<InspectorItem>, prop_ix: usize| {
            items.push(InspectorItem::Property {
                card: card_ix,
                prop: prop_ix,
            });
            items.extend((0..card.props[prop_ix].extra_rows).map(|ix| InspectorItem::Child {
                card: card_ix,
                prop: prop_ix,
                ix,
            }));
        };
        let mut categories: Vec<(String, Vec<usize>, bool, usize)> = Vec::new();
        let mut index_of: HashMap<&str, usize> = HashMap::new();
        for &prop_ix in &visible {
            let prop = &card.props[prop_ix];
            match prop.category.as_deref().filter(|c| !c.trim().is_empty()) {
                None => push_property(&mut items, prop_ix),
                Some(cat) => {
                    let order = prop.category_order.unwrap_or(usize::MAX);
                    let at = *index_of.entry(cat).or_insert_with(|| {
                        categories.push((cat.to_string(), Vec::new(), false, usize::MAX));
                        categories.len() - 1
                    });
                    let entry = &mut categories[at];
                    entry.1.push(prop_ix);
                    entry.2 |= prop.category_default_collapsed;
                    entry.3 = entry.3.min(order);
                }
            }
        }
        categories.sort_by_key(|c| c.3);
        for (name, props, default_collapsed, _) in categories {
            let collapsed = query.is_empty()
                && collapse.is_category_collapsed(&card.key, &name, default_collapsed);
            items.push(InspectorItem::Category {
                card: card_ix,
                name,
            });
            if !collapsed {
                for prop in props {
                    push_property(&mut items, prop);
                }
            }
        }
    }
    items
}

/// A category's default-collapsed flag, as [`flatten`] computed it (any
/// property of the category asking for it).
pub fn category_default_collapsed(card: &CardDesc, category: &str) -> bool {
    card.props
        .iter()
        .filter(|p| p.category.as_deref() == Some(category))
        .any(|p| p.category_default_collapsed)
}

/// A category's accent colour: the first property that declares one.
pub fn category_color<'a>(card: &'a CardDesc, category: &str) -> Option<&'a str> {
    card.props
        .iter()
        .filter(|p| p.category.as_deref() == Some(category))
        .find_map(|p| p.category_color.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(name: &str, category: Option<&str>) -> PropDesc {
        PropDesc {
            name: name.to_string(),
            display_name: name.to_string(),
            category: category.map(str::to_string),
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            extra_rows: 0,
        }
    }

    fn cards() -> Vec<CardDesc> {
        vec![
            CardDesc {
                key: "Light#0".into(),
                title: "Light".into(),
                props: vec![
                    prop("intensity", None),
                    prop("shadow_bias", Some("Shadows")),
                    prop("shadow_size", Some("Shadows")),
                ],
                custom: false,
                note: None,
            },
            CardDesc {
                key: "Mesh#1".into(),
                title: "Mesh".into(),
                props: vec![prop("path", None)],
                custom: false,
                note: None,
            },
        ]
    }

    #[test]
    fn no_query_lists_everything() {
        let items = flatten(&cards(), &Query::default(), &CollapseState::default());
        // Light header, intensity, Shadows header + 2 rows, Mesh header, path.
        assert_eq!(items.len(), 7);
        assert_eq!(items[0], InspectorItem::Card { card: 0 });
        assert_eq!(items[1], InspectorItem::Property { card: 0, prop: 0 });
    }

    #[test]
    fn collapsed_card_keeps_only_its_header() {
        let mut c = CollapseState::default();
        c.toggle_card("Light#0");
        let items = flatten(&cards(), &Query::default(), &c);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], InspectorItem::Card { card: 0 });
        assert_eq!(items[1], InspectorItem::Card { card: 1 });
    }

    #[test]
    fn collapsed_category_keeps_its_header() {
        let mut c = CollapseState::default();
        c.toggle_category("Light#0", "Shadows", false);
        let items = flatten(&cards(), &Query::default(), &c);
        assert_eq!(items.len(), 5);
        c.toggle_category("Light#0", "Shadows", false);
        assert_eq!(flatten(&cards(), &Query::default(), &c).len(), 7);
    }

    #[test]
    fn query_filters_rows_and_ignores_collapse() {
        let mut c = CollapseState::default();
        c.toggle_card("Light#0");
        let items = flatten(&cards(), &Query::new("shadow size"), &c);
        assert_eq!(
            items,
            vec![
                InspectorItem::Card { card: 0 },
                InspectorItem::Category {
                    card: 0,
                    name: "Shadows".into()
                },
                InspectorItem::Property { card: 0, prop: 2 },
            ]
        );
    }

    #[test]
    fn title_match_shows_whole_card_and_drops_others() {
        let items = flatten(&cards(), &Query::new("mesh"), &CollapseState::default());
        assert_eq!(
            items,
            vec![
                InspectorItem::Card { card: 1 },
                InspectorItem::Property { card: 1, prop: 0 },
            ]
        );
    }

    #[test]
    fn tokens_may_span_card_and_property() {
        let items = flatten(&cards(), &Query::new("light intens"), &CollapseState::default());
        assert!(items.contains(&InspectorItem::Property { card: 0, prop: 0 }));
    }

    #[test]
    fn no_match_is_empty() {
        assert!(flatten(&cards(), &Query::new("zzz"), &CollapseState::default()).is_empty());
    }

    #[test]
    fn default_collapsed_category_starts_collapsed_until_expanded() {
        let mut cs = cards();
        cs[0].props[1].category_default_collapsed = true;
        let mut c = CollapseState::default();
        assert_eq!(flatten(&cs, &Query::default(), &c).len(), 5);
        c.toggle_category("Light#0", "Shadows", true);
        assert_eq!(flatten(&cs, &Query::default(), &c).len(), 7);
    }
}

#[cfg(test)]
mod extra_rows_tests {
    use super::*;

    #[test]
    fn a_property_is_followed_by_the_rows_it_offers() {
        let mut card = CardDesc {
            key: "A#0".into(),
            title: "A".into(),
            props: vec![
                PropDesc {
                    name: "list".into(),
                    display_name: "List".into(),
                    category: None,
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: None,
                    extra_rows: 3,
                },
                PropDesc {
                    name: "after".into(),
                    display_name: "After".into(),
                    category: None,
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: None,
                    extra_rows: 0,
                },
            ],
            custom: false,
            note: None,
        };
        let items = flatten(&[card.clone()], &Query::default(), &CollapseState::default());
        assert_eq!(items.len(), 1 + 1 + 3 + 1);
        assert_eq!(items[2], InspectorItem::Child { card: 0, prop: 0, ix: 0 });
        assert_eq!(items[5], InspectorItem::Property { card: 0, prop: 1 });
        card.props[0].extra_rows = 100_000;
        let items = flatten(&[card], &Query::default(), &CollapseState::default());
        assert_eq!(items.len(), 1 + 1 + 100_000 + 1);
    }
}
