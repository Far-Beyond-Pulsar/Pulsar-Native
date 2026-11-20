use gpui::*;

actions!(
    problems_drawer,
    [FilterAll, FilterErrors, FilterWarnings, FilterInfo, ClearSourceFilter]
);

/// Show only the diagnostics one source published (a language server, a
/// compiler).
#[derive(Clone, PartialEq, Eq, serde::Deserialize, gpui::Action)]
#[action(namespace = problems_drawer, no_json)]
pub struct FilterSource {
    pub source: String,
}
