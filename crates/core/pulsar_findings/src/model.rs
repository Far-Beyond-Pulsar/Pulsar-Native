//! What a finding says.

use std::path::PathBuf;

/// Which tab of the Problems panel a finding belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Code and scripts: compile errors, script runtime errors, class rules.
    Code,
    /// Performance: budgets that are full, frames that are slow, work the
    /// renderer refuses or drops.
    Performance,
    /// The project's health: plugins, imports, levels, pipelines, shaders.
    Project,
    /// The editor itself: stalled tasks, an unresponsive UI thread.
    Editor,
}

impl Category {
    pub const ALL: [Category; 4] = [Self::Code, Self::Performance, Self::Project, Self::Editor];

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Performance => "Performance",
            Self::Project => "Project",
            Self::Editor => "Editor",
        }
    }
}

/// How much a finding matters. For a performance finding this is its
/// impact: `Error` costs the frame visibly or drops content, `Warning`
/// costs quality or headroom, `Info` is advice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

/// Which settings window a setting is edited in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsWindow {
    Project,
    Editor,
}

/// Where a finding is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Location {
    /// Nowhere in particular (the frame, the project as a whole).
    #[default]
    None,
    /// A source file, at a 1-based line and column (0: the whole file).
    File {
        path: PathBuf,
        line: usize,
        column: usize,
    },
    /// An asset file of the project.
    Asset(PathBuf),
    /// A scene object, by its id.
    Object { id: String },
    /// A setting.
    Setting {
        window: SettingsWindow,
        namespace: String,
        owner: String,
        key: String,
    },
}

/// What a finding's button does. The panel carries it out.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Open a file at a line and column.
    OpenFile {
        path: PathBuf,
        line: usize,
        column: usize,
    },
    /// Open a settings window on the page of `owner` (`page`: a page title,
    /// such as a plugin's name, when the owner alone does not name it).
    OpenSettings {
        window: SettingsWindow,
        page: Option<String>,
    },
    /// Open an editor window by its registered name (`"TasksWindow"`).
    OpenWindow { name: &'static str },
}

/// A labelled [`Action`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FindingAction {
    pub label: String,
    pub action: Action,
}

impl FindingAction {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            action,
        }
    }
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Finding {
    /// Stable within its source across republishes (what a dismissal and
    /// the selection remember): derive it from what the finding is about,
    /// never from a count or a time.
    pub key: String,
    pub category: Category,
    pub severity: Severity,
    /// One line: what is wrong.
    pub title: String,
    /// Why it matters and what to do, shown when the row is expanded.
    pub detail: String,
    /// What the finding is about, as the panel groups by it (a file, an
    /// asset, a plugin, a render pass). `None`: grouped under its source.
    pub subject: Option<String>,
    pub location: Location,
    /// A short measured value shown at the row's end (`"2.1 ms"`,
    /// `"312 / 256"`).
    pub metric: Option<String>,
    pub actions: Vec<FindingAction>,
}

impl Finding {
    /// A finding with a key, category, severity and title; the rest empty.
    pub fn new(
        key: impl Into<String>,
        category: Category,
        severity: Severity,
        title: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            category,
            severity,
            title: title.into(),
            detail: String::new(),
            subject: None,
            location: Location::None,
            metric: None,
            actions: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn location(mut self, location: Location) -> Self {
        self.location = location;
        self
    }

    pub fn metric(mut self, metric: impl Into<String>) -> Self {
        self.metric = Some(metric.into());
        self
    }

    pub fn action(mut self, label: impl Into<String>, action: Action) -> Self {
        self.actions.push(FindingAction::new(label, action));
        self
    }
}
