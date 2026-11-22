//! What the panel lists, as plain data: every code diagnostic and every
//! published finding as one [`Item`], then the rows of the current tab (group
//! headings and items) after the severity filter, the search, the dismissed
//! set and the collapsed groups.
//!
//! Nothing here touches GPUI, so the arrangement is tested on its own.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pulsar_findings::{Category, FindingAction, Location, Published, Severity};

use super::filter::display_path;
use super::types::{Diagnostic, DiagnosticSeverity};

/// One line of the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `<source>|<key>`: stable across republishes (what a dismissal, the
    /// selection and a collapsed group remember).
    pub id: String,
    pub source: String,
    pub category: Category,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// The group heading it is listed under.
    pub group: String,
    pub location: Location,
    pub metric: Option<String>,
    pub actions: Vec<FindingAction>,
    /// A code diagnostic: its source and index in that source's slot (its
    /// quick fixes and file preview are read from there).
    pub code: Option<(String, usize)>,
}

impl Item {
    /// Code diagnostics are fixed, not dismissed.
    pub fn dismissible(&self) -> bool {
        self.category != Category::Code
    }
}

pub fn severity_of(severity: &DiagnosticSeverity) -> Severity {
    match severity {
        DiagnosticSeverity::Error => Severity::Error,
        DiagnosticSeverity::Warning => Severity::Warning,
        DiagnosticSeverity::Information | DiagnosticSeverity::Hint => Severity::Info,
    }
}

/// Every item: the code diagnostics of each source (`code`: source name to
/// its current set), then every published finding.
pub fn build_items(
    code: &BTreeMap<String, Vec<Diagnostic>>,
    findings: &Published,
    project_root: Option<&Path>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for (source, diagnostics) in code {
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            let mut lines = diagnostic.message.lines();
            let title = lines.next().unwrap_or_default().trim().to_string();
            let detail = lines.collect::<Vec<_>>().join("\n").trim().to_string();
            items.push(Item {
                id: format!(
                    "{source}|{}:{}:{}:{title}",
                    diagnostic.file_path, diagnostic.line, diagnostic.column
                ),
                source: source.clone(),
                category: Category::Code,
                severity: severity_of(&diagnostic.severity),
                title,
                detail,
                group: display_path(project_root, &diagnostic.file_path),
                location: Location::File {
                    path: PathBuf::from(&diagnostic.file_path),
                    line: diagnostic.line,
                    column: diagnostic.column,
                },
                metric: (diagnostic.line > 0)
                    .then(|| format!("{}:{}", diagnostic.line, diagnostic.column)),
                actions: Vec::new(),
                code: Some((source.clone(), index)),
            });
        }
    }
    // A source's label is its name before `#` (a registration's id, a
    // renderer's instance): ids are by label, so a dismissal outlives a
    // restart, and a finding two instances share (two viewports) is listed once.
    let mut seen = std::collections::HashSet::new();
    for (source, set) in findings.iter() {
        let label = source.split('#').next().unwrap_or(source);
        for finding in set.iter() {
            let id = format!("{label}|{}", finding.key);
            if !seen.insert(id.clone()) {
                continue;
            }
            items.push(Item {
                id,
                source: label.to_string(),
                category: finding.category,
                severity: finding.severity,
                title: finding.title.clone(),
                detail: finding.detail.clone(),
                group: finding.subject.clone().unwrap_or_else(|| label.to_string()),
                location: finding.location.clone(),
                metric: finding.metric.clone(),
                actions: finding.actions.clone(),
                code: None,
            });
        }
    }
    items
}

/// What the list shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub severity: Option<Severity>,
    /// Lowercased search text.
    pub query: String,
    /// List only the dismissed items (to restore them).
    pub dismissed_only: bool,
    /// Only this source's items.
    pub source: Option<String>,
}

fn matches(item: &Item, view: &View, dismissed: &BTreeSet<String>) -> bool {
    let is_dismissed = item.dismissible() && dismissed.contains(&item.id);
    if is_dismissed != view.dismissed_only {
        return false;
    }
    if view.severity.is_some_and(|s| s != item.severity) {
        return false;
    }
    if view.source.as_ref().is_some_and(|source| *source != item.source) {
        return false;
    }
    let query = view.query.as_str();
    query.is_empty()
        || item.title.to_lowercase().contains(query)
        || item.detail.to_lowercase().contains(query)
        || item.group.to_lowercase().contains(query)
        || item.source.to_lowercase().contains(query)
}

/// A tab's badge: how many items it shows and the worst of them (dismissed
/// items not counted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabCount {
    pub count: usize,
    pub worst: Option<Severity>,
}

pub fn tab_counts(items: &[Item], dismissed: &BTreeSet<String>) -> BTreeMap<Category, TabCount> {
    let mut counts: BTreeMap<Category, TabCount> = Category::ALL
        .into_iter()
        .map(|category| (category, TabCount::default()))
        .collect();
    for item in items {
        if item.dismissible() && dismissed.contains(&item.id) {
            continue;
        }
        let tab = counts.entry(item.category).or_default();
        tab.count += 1;
        tab.worst = Some(tab.worst.map_or(item.severity, |w| w.min(item.severity)));
    }
    counts
}

/// How many items `category` has dismissed.
pub fn dismissed_count(items: &[Item], category: Category, dismissed: &BTreeSet<String>) -> usize {
    items
        .iter()
        .filter(|item| item.category == category && item.dismissible() && dismissed.contains(&item.id))
        .count()
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Group {
        /// `<category>/<group>`: what collapsing remembers.
        key: String,
        title: String,
        count: usize,
        worst: Severity,
        collapsed: bool,
    },
    /// An index into the items.
    Item(usize),
}

/// The rows of `category`: groups ordered worst first (then by name), each
/// group's items worst first (then by title); a collapsed group lists only
/// its heading.
pub fn rows(
    items: &[Item],
    category: Category,
    view: &View,
    dismissed: &BTreeSet<String>,
    collapsed: &BTreeSet<String>,
) -> Vec<Row> {
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (ix, item) in items.iter().enumerate() {
        if item.category == category && matches(item, view, dismissed) {
            groups.entry(item.group.as_str()).or_default().push(ix);
        }
    }
    let mut groups: Vec<(&str, Vec<usize>, Severity)> = groups
        .into_iter()
        .map(|(title, mut members)| {
            members.sort_by(|&a, &b| {
                (items[a].severity, &items[a].title).cmp(&(items[b].severity, &items[b].title))
            });
            let worst = members.iter().map(|&ix| items[ix].severity).min().unwrap_or(Severity::Info);
            (title, members, worst)
        })
        .collect();
    groups.sort_by(|a, b| (a.2, a.0).cmp(&(b.2, b.0)));

    let mut rows = Vec::new();
    for (title, members, worst) in groups {
        let key = format!("{}/{title}", category.label());
        // Searching opens every group.
        let is_collapsed = view.query.is_empty() && collapsed.contains(&key);
        rows.push(Row::Group {
            key,
            title: title.to_string(),
            count: members.len(),
            worst,
            collapsed: is_collapsed,
        });
        if !is_collapsed {
            rows.extend(members.into_iter().map(Row::Item));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_findings::Finding;
    use std::sync::Arc;

    fn diagnostic(file: &str, severity: DiagnosticSeverity, message: &str) -> Diagnostic {
        Diagnostic {
            file_path: file.into(),
            line: 3,
            column: 5,
            end_line: None,
            end_column: None,
            severity,
            message: message.into(),
            source: None,
            hints: Vec::new(),
            subitems: Vec::new(),
            loading_actions: false,
        }
    }

    fn sample() -> Vec<Item> {
        let mut code = BTreeMap::new();
        code.insert(
            "server".to_string(),
            vec![
                diagnostic("/p/src/a.rs", DiagnosticSeverity::Warning, "unused\nsecond line"),
                diagnostic("/p/src/b.rs", DiagnosticSeverity::Error, "mismatched"),
                diagnostic("/p/src/a.rs", DiagnosticSeverity::Error, "broken"),
            ],
        );
        let mut findings = BTreeMap::new();
        findings.insert(
            "Budgets#check".to_string(),
            Arc::new(vec![
                Finding::new("pool", Category::Performance, Severity::Warning, "Pool full")
                    .subject("Pool")
                    .metric("92%"),
                Finding::new("spike", Category::Performance, Severity::Error, "Frame spike"),
            ]),
        );
        build_items(&code, &Arc::new(findings), Some(Path::new("/p")))
    }

    #[test]
    fn code_is_grouped_by_project_relative_file_worst_first() {
        let items = sample();
        let rows = rows(&items, Category::Code, &View::default(), &BTreeSet::new(), &BTreeSet::new());
        let titles: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Group { title, .. } => format!("# {title}"),
                Row::Item(ix) => items[*ix].title.clone(),
            })
            .collect();
        assert_eq!(titles, ["# p/src/a.rs", "broken", "unused", "# p/src/b.rs", "mismatched"]);
        let unused = items.iter().find(|i| i.title == "unused").unwrap();
        assert_eq!(unused.detail, "second line", "the message's first line is the title");
    }

    #[test]
    fn findings_without_a_subject_group_under_their_source_label() {
        let items = sample();
        let rows = rows(&items, Category::Performance, &View::default(), &BTreeSet::new(), &BTreeSet::new());
        let groups: Vec<&str> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Group { title, .. } => Some(title.as_str()),
                Row::Item(_) => None,
            })
            .collect();
        assert_eq!(groups, ["Budgets", "Pool"], "the group with an error comes first");
    }

    #[test]
    fn dismissed_findings_leave_the_list_and_the_count_until_restored() {
        let items = sample();
        let pool = items.iter().find(|i| i.title == "Pool full").unwrap().id.clone();
        let dismissed: BTreeSet<String> = [pool].into();
        let counts = tab_counts(&items, &dismissed);
        assert_eq!(counts[&Category::Performance].count, 1);
        assert_eq!(counts[&Category::Code].count, 3);
        assert_eq!(counts[&Category::Code].worst, Some(Severity::Error));
        let shown = rows(&items, Category::Performance, &View::default(), &dismissed, &BTreeSet::new());
        assert_eq!(shown.len(), 2, "one group, one item");
        let restore_view = View {
            dismissed_only: true,
            ..View::default()
        };
        let restore = rows(&items, Category::Performance, &restore_view, &dismissed, &BTreeSet::new());
        assert_eq!(restore.len(), 2);
        assert_eq!(dismissed_count(&items, Category::Performance, &dismissed), 1);
    }

    #[test]
    fn a_collapsed_group_shows_only_its_heading_unless_searching() {
        let items = sample();
        let key = "Code/p/src/a.rs".to_string();
        let collapsed: BTreeSet<String> = [key].into();
        let closed = rows(&items, Category::Code, &View::default(), &BTreeSet::new(), &collapsed);
        assert_eq!(closed.len(), 3, "a.rs heading, b.rs heading and its item");
        let searching = View {
            query: "unused".into(),
            ..View::default()
        };
        let open = rows(&items, Category::Code, &searching, &BTreeSet::new(), &collapsed);
        assert_eq!(open.len(), 2);
    }

    #[test]
    fn severity_filter_and_search_combine() {
        let items = sample();
        let view = View {
            severity: Some(Severity::Error),
            query: "broken".into(),
            ..View::default()
        };
        let rows = rows(&items, Category::Code, &view, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(rows.len(), 2);
    }
}
