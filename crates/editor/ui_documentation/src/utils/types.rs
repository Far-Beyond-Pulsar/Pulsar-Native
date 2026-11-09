use ui::IconName;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocCategory {
    Engine,
    Project,
    Manual,
}

impl DocCategory {
    pub const ALL: [DocCategory; 3] = [
        DocCategory::Engine,
        DocCategory::Project,
        DocCategory::Manual,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DocCategory::Engine => "Engine",
            DocCategory::Project => "Project",
            DocCategory::Manual => "Guides",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            DocCategory::Engine => IconName::Code,
            DocCategory::Project => IconName::Package,
            DocCategory::Manual => IconName::BookOpen,
        }
    }

    pub fn search_placeholder(self) -> &'static str {
        match self {
            DocCategory::Engine => "Search the engine API",
            DocCategory::Project => "Search your project",
            DocCategory::Manual => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_each_category_once() {
        assert_eq!(DocCategory::ALL.len(), 3);
        for (i, a) in DocCategory::ALL.iter().enumerate() {
            for b in &DocCategory::ALL[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn labels_are_stable() {
        let labels: Vec<_> = DocCategory::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels, vec!["Engine", "Project", "Guides"]);
    }

    #[test]
    fn manual_category_has_no_search_box_text() {
        assert!(DocCategory::Manual.search_placeholder().is_empty());
        assert!(!DocCategory::Engine.search_placeholder().is_empty());
        assert!(!DocCategory::Project.search_placeholder().is_empty());
    }
}
