use ui::IconName;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocCategory {
    Engine,
    Project,
    Manual,
}

impl DocCategory {
    pub const ALL: [DocCategory; 3] = [DocCategory::Engine, DocCategory::Project, DocCategory::Manual];

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
