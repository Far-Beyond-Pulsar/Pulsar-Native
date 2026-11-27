//! The browser's search: space-separated terms, all of which must match.
//!
//! - `word`: in the entity's name, stable id or `index v generation`;
//! - `has:word` (or `c:word`): the entity has a component whose type or class
//!   name contains `word`;
//! - `-word`, `-has:word`: the same, negated.

use super::index::{EntityRow, SceneIndex, TypeRow};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    negated: bool,
    component: bool,
    /// Lowercase.
    text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    terms: Vec<Term>,
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let terms = input
            .split_whitespace()
            .filter_map(|word| {
                let (negated, word) = match word.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, word),
                };
                let lower = word.to_lowercase();
                let (component, text) = match lower
                    .strip_prefix("has:")
                    .or_else(|| lower.strip_prefix("c:"))
                {
                    Some(rest) => (true, rest.to_string()),
                    None => (false, lower),
                };
                (!text.is_empty()).then_some(Term {
                    negated,
                    component,
                    text,
                })
            })
            .collect();
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The entities that match, as a mask over [`SceneIndex::entities`].
    pub fn matches(&self, index: &SceneIndex) -> Vec<bool> {
        // A component term is decided once per archetype, not per entity.
        let per_archetype: Vec<Vec<bool>> = self
            .terms
            .iter()
            .map(|term| {
                if !term.component {
                    return Vec::new();
                }
                let types: Vec<bool> = index
                    .types
                    .iter()
                    .map(|row| type_contains(row, &term.text))
                    .collect();
                index
                    .archetypes
                    .iter()
                    .map(|archetype| archetype.types.iter().any(|&t| types[t as usize]))
                    .collect()
            })
            .collect();
        index
            .entities
            .iter()
            .map(|row| self.entity(row, &per_archetype))
            .collect()
    }

    fn entity(&self, row: &EntityRow, per_archetype: &[Vec<bool>]) -> bool {
        self.terms
            .iter()
            .zip(per_archetype)
            .all(|(term, by_archetype)| {
                let found = if term.component {
                    by_archetype[row.archetype as usize]
                } else {
                    row.haystack.contains(&term.text)
                };
                found != term.negated
            })
    }

    /// Whether a component type passes the plain and component terms, for
    /// the Components list.
    pub fn matches_type(&self, row: &TypeRow) -> bool {
        self.terms
            .iter()
            .all(|term| type_contains(row, &term.text) != term.negated)
    }
}

fn type_contains(row: &TypeRow, text: &str) -> bool {
    row.name.to_lowercase().contains(text)
        || row
            .class
            .is_some_and(|class| class.to_lowercase().contains(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::index::scan_world;
    use pulsar_scene_model::{Name, StableId};
    use pulsar_scenedb::World;

    struct Physics;

    fn names(index: &SceneIndex, query: &str) -> Vec<String> {
        let mask = Query::parse(query).matches(index);
        let mut names: Vec<String> = index
            .entities
            .iter()
            .zip(mask)
            .filter(|(_, hit)| *hit)
            .map(|(row, _)| row.label.to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn terms_match_text_components_and_their_negations() {
        let mut world = World::new();
        for (id, name, physics) in [("1", "Crate", true), ("2", "Crane", false)] {
            let entity = world.spawn();
            world.insert(entity, StableId(id.into()));
            world.insert(entity, Name(name.into()));
            if physics {
                world.insert(entity, Physics);
            }
        }
        let index = scan_world(&world);
        assert_eq!(names(&index, ""), ["Crane", "Crate"]);
        assert_eq!(names(&index, "cra"), ["Crane", "Crate"]);
        assert_eq!(names(&index, "has:phys"), ["Crate"]);
        assert_eq!(names(&index, "cra -c:phys"), ["Crane"]);
        assert_eq!(names(&index, "cra -crate"), ["Crane"]);
        assert!(names(&index, "zzz").is_empty());
        assert!(Query::parse("  -  has: ").is_empty());
    }
}
