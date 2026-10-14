//! Harnesses: declarative agent definitions a parent run can delegate to.
//!
//! A harness is a markdown file with a small frontmatter block and the system
//! prompt as its body:
//!
//! ```text
//! ---
//! name: explorer
//! description: Read-only code search. Returns findings with file:line refs.
//! tools: [group:read, group:search]
//! model: cheap            # cheap | same | <model id>
//! context_budget: 12000   # tokens
//! max_steps: 24
//! max_depth: 0            # how many levels of sub-agents this one may start
//! permissions: read-only  # read-only | edit-with-checkpoint | engine-scene-write
//! verify: [refs_exist]    # optional: refs_exist | schema | nonempty | default
//! result: [summary, findings, refs, open_questions]
//! ---
//! Prompt body. Placeholders: {workspace} {task} {pinned}
//! ```
//!
//! Built-ins ship in the binary; `.pulsar/agents/*.md` in a project and in the
//! user-global folder add to or replace them (project beats global beats
//! built-in, matched by `name`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::taint::{ToolClass, ToolClassifier};
use crate::trust::TrustStore;

/// Which model a child run uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelPolicy {
    /// A specific model id.
    Explicit(String),
    /// The parent's compaction model (cheaper, same provider).
    Cheap,
    /// The parent's model.
    Same,
}

impl ModelPolicy {
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "" | "same" => ModelPolicy::Same,
            "cheap" => ModelPolicy::Cheap,
            other => ModelPolicy::Explicit(other.to_string()),
        }
    }
}

/// What the child may change. Enforced by trimming the tool list: a
/// `ReadOnly` harness never receives edit tools whatever its allow-list says.
/// Ordered by power: `ReadOnly < EditWithCheckpoint < EngineSceneWrite`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HarnessPermissions {
    ReadOnly,
    /// Edit tools are allowed; Phase 5 snapshots the workspace first.
    EditWithCheckpoint,
    /// Edit tools plus the engine scene tools.
    EngineSceneWrite,
}

impl HarnessPermissions {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "read-only" | "read_only" | "readonly" => Some(Self::ReadOnly),
            "edit-with-checkpoint" | "edit_with_checkpoint" | "edit" => {
                Some(Self::EditWithCheckpoint)
            }
            "engine-scene-write" | "engine_scene_write" | "scene-write" => {
                Some(Self::EngineSceneWrite)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarnessOrigin {
    Builtin,
    /// Registered by a plugin; below global and project files.
    Plugin,
    Global,
    Project,
}

/// The section names a report is parsed into.
pub const DEFAULT_RESULT_FIELDS: [&str; 4] = ["summary", "findings", "refs", "open_questions"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harness {
    pub name: String,
    /// What the parent model reads when choosing a harness.
    pub description: String,
    /// Template with `{workspace}`, `{task}` and `{pinned}`.
    pub system_prompt: String,
    /// Tool names, `group:<name>` entries, or `*`.
    pub tools: Vec<String>,
    pub model_policy: ModelPolicy,
    /// Tokens; `None` inherits the parent's budget.
    pub context_budget_tokens: Option<u32>,
    pub max_steps: u32,
    /// Levels of sub-agents this harness's runs may start (0 = leaf).
    pub max_depth: u32,
    /// Best-of-N for this harness's runs (final report and invalid tool
    /// calls); below 2 is off.
    pub best_of: u32,
    /// Sections the final answer is parsed into.
    pub result_schema: Vec<String>,
    pub permissions: HarnessPermissions,
    /// The run edits a private fork of the project state (the scene) and
    /// returns its changes as a diff for review instead of applying them.
    /// Only meaningful with `permissions: engine-scene-write`.
    pub sandbox: bool,
    /// Named checks run on the final report before it is returned (see
    /// [`crate::report_verify`]): `refs_exist`, `schema`, `nonempty`. A
    /// failure is fed back to the child once; if it fails again the report is
    /// returned flagged `unverified`.
    pub verify: Vec<String>,
    /// Sandbox mode chosen by the harness (`sandbox: overlay|copy|off`), kept
    /// as written for the scene tools to interpret. `None` (`sandbox: true`
    /// or unset) uses the project's setting.
    pub sandbox_mode: Option<String>,
    pub origin: HarnessOrigin,
}

/// Tools no child receives: they drive the editor UI.
pub const CHILD_EXCLUDED_TOOLS: [&str; 5] = [
    "open_file_in_default_editor",
    "activate_open_editor",
    "query_open_editors",
    // Plan mode and project memory are the chat's own.
    "exit_plan_mode",
    "remember",
];

/// Tools that change files or editor state.
const EDIT_TOOLS: [&str; 5] = [
    "call_plugin_tool",
    "execute_plugin_tool",
    "open_file_in_default_editor",
    "activate_open_editor",
    "task_list_update",
];

fn group_members(group: &str) -> &'static [&'static str] {
    match group {
        "read" => &[
            "cd",
            "list_files",
            "tree",
            "query_available_file_types",
            "query_file_editors",
            "query_plugin_tools",
            "query_tools_for_plugin",
            "task_list_read",
        ],
        "search" => &["list_files", "tree"],
        "web" => &["web_search", "fetch_url"],
        "task" => &["task_list_read", "task_list_update"],
        "edit" => &["call_plugin_tool", "execute_plugin_tool"],
        _ => &[],
    }
}

impl Harness {
    /// Fill the placeholders. Unknown ones are left as written.
    pub fn render_prompt(&self, workspace: &str, task: &str, pinned: &str) -> String {
        self.system_prompt
            .replace("{workspace}", workspace)
            .replace("{task}", task)
            .replace("{pinned}", pinned)
    }

    /// Which of `available` this harness may call, judged with the default
    /// tool classification.
    pub fn resolve_tools(&self, available: &[String]) -> Vec<String> {
        self.resolve_tools_with(available, &ToolClassifier::default(), self.permissions)
    }

    /// Like [`Harness::resolve_tools`], with the run's own classifier and a
    /// permission `ceiling` (a child never gets more than the run that
    /// started it). A read-only result holds no tool the classifier calls
    /// `Write`; the safety layer refuses such calls again at run time.
    pub fn resolve_tools_with(
        &self,
        available: &[String],
        classifier: &ToolClassifier,
        ceiling: HarnessPermissions,
    ) -> Vec<String> {
        let permissions = self.permissions.min(ceiling);
        let mut allowed: Vec<String> = Vec::new();
        for entry in &self.tools {
            if entry == "*" {
                allowed.extend(available.iter().cloned());
            } else if let Some(group) = entry.strip_prefix("group:") {
                if group == "scene" {
                    allowed.extend(available.iter().filter(|n| n.starts_with("scene_")).cloned());
                } else {
                    allowed.extend(
                        group_members(group)
                            .iter()
                            .filter(|name| available.iter().any(|a| a == **name))
                            .map(|name| name.to_string()),
                    );
                }
            } else if available.iter().any(|a| a == entry) {
                allowed.push(entry.clone());
            }
        }
        allowed.retain(|name| {
            if CHILD_EXCLUDED_TOOLS.contains(&name.as_str()) {
                return false;
            }
            match permissions {
                HarnessPermissions::ReadOnly => {
                    !EDIT_TOOLS.contains(&name.as_str())
                        && !name.starts_with("scene_write")
                        && classifier.classify(name) != ToolClass::Write
                }
                HarnessPermissions::EditWithCheckpoint => !name.starts_with("scene_write"),
                HarnessPermissions::EngineSceneWrite => true,
            }
        });
        allowed.sort();
        allowed.dedup();
        allowed
    }

    /// Parse one harness file. `name` falls back to `fallback_name`
    /// (usually the file stem).
    pub fn parse(
        source: &str,
        fallback_name: &str,
        origin: HarnessOrigin,
    ) -> Result<Harness, String> {
        let source = source.trim_start_matches('\u{feff}');
        let (front, body) = split_frontmatter(source)?;
        let mut harness = Harness {
            name: fallback_name.to_string(),
            description: String::new(),
            system_prompt: body.trim().to_string(),
            tools: Vec::new(),
            model_policy: ModelPolicy::Same,
            context_budget_tokens: None,
            max_steps: 24,
            max_depth: 0,
            best_of: 0,
            result_schema: DEFAULT_RESULT_FIELDS.iter().map(|s| s.to_string()).collect(),
            permissions: HarnessPermissions::ReadOnly,
            sandbox: false,
            verify: Vec::new(),
            sandbox_mode: None,
            origin,
        };
        let mut tools_set = false;

        for (key, value) in parse_frontmatter(front) {
            match key.as_str() {
                "name" => {
                    let name = value.scalar();
                    if !name.is_empty() {
                        harness.name = name;
                    }
                }
                "description" => harness.description = value.scalar(),
                "tools" => {
                    harness.tools = value.list();
                    tools_set = true;
                }
                "model" | "model_policy" => harness.model_policy = ModelPolicy::parse(&value.scalar()),
                "context_budget" => {
                    harness.context_budget_tokens = value.scalar().replace('_', "").parse().ok()
                }
                "max_steps" => {
                    harness.max_steps = value
                        .scalar()
                        .parse()
                        .map_err(|_| format!("max_steps must be a number, got `{}`", value.scalar()))?
                }
                "max_depth" => {
                    harness.max_depth = value
                        .scalar()
                        .parse()
                        .map_err(|_| format!("max_depth must be a number, got `{}`", value.scalar()))?
                }
                "best_of" => {
                    harness.best_of = value
                        .scalar()
                        .parse()
                        .map_err(|_| format!("best_of must be a number, got `{}`", value.scalar()))?
                }
                "result" | "result_schema" => {
                    let fields = value.list();
                    if !fields.is_empty() {
                        harness.result_schema = fields.iter().map(|f| normalize_field(f)).collect();
                    }
                }
                "permissions" => {
                    harness.permissions = HarnessPermissions::parse(&value.scalar())
                        .ok_or_else(|| format!("unknown permissions `{}`", value.scalar()))?
                }
                "verify" => {
                    let names = value.list();
                    if let Some(unknown) =
                        names.iter().find(|name| !crate::report_verify::is_known(name))
                    {
                        return Err(format!(
                            "unknown verifier `{unknown}` (known: {})",
                            crate::report_verify::KNOWN.join(", ")
                        ));
                    }
                    harness.verify = names;
                }
                "sandbox" => {
                    let text = value.scalar().to_lowercase();
                    match text.as_str() {
                        // A named mode implies a sandbox request; `off` makes
                        // the request run live.
                        "overlay" | "copy" | "off" => {
                            harness.sandbox = true;
                            harness.sandbox_mode = Some(text);
                        }
                        other => {
                            harness.sandbox = matches!(other, "true" | "yes" | "on" | "1");
                            harness.sandbox_mode = None;
                        }
                    }
                }
                _ => {}
            }
        }

        if harness.name.trim().is_empty() {
            return Err("harness has no name".to_string());
        }
        if harness.system_prompt.is_empty() {
            return Err(format!("harness `{}` has an empty prompt", harness.name));
        }
        if harness.max_steps == 0 {
            return Err(format!("harness `{}` has max_steps 0", harness.name));
        }
        if !tools_set {
            harness.tools = vec!["group:read".to_string()];
        }
        Ok(harness)
    }
}

/// Lowercase, snake_case section name.
pub(crate) fn normalize_field(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| matches!(c, '#' | '*' | ':' | '_' | '`') || c.is_whitespace())
        .to_lowercase()
        .replace([' ', '-'], "_")
}

fn split_frontmatter(source: &str) -> Result<(&str, &str), String> {
    let trimmed = source.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return Ok(("", source));
    };
    let rest = rest.trim_start_matches(['\r', ' ']);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Ok((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err("frontmatter is not closed with `---`".to_string())
}

enum FrontValue {
    Scalar(String),
    List(Vec<String>),
}

impl FrontValue {
    fn scalar(&self) -> String {
        match self {
            FrontValue::Scalar(s) => s.clone(),
            FrontValue::List(items) => items.join(", "),
        }
    }

    fn list(&self) -> Vec<String> {
        match self {
            FrontValue::Scalar(s) => s
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect(),
            FrontValue::List(items) => items.clone(),
        }
    }
}

fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    for quote in ['"', '\''] {
        if raw.len() >= 2 && raw.starts_with(quote) && raw.ends_with(quote) {
            return raw[1..raw.len() - 1].to_string();
        }
    }
    raw.to_string()
}

/// `key: value`, `key: [a, b]` and `key:` followed by `- item` lines.
/// Trailing `# comments` after a space are dropped from unquoted values.
fn parse_frontmatter(front: &str) -> Vec<(String, FrontValue)> {
    let mut out: Vec<(String, FrontValue)> = Vec::new();
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some((_, FrontValue::List(items))) = out.last_mut() {
                items.push(unquote(item));
                continue;
            }
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let mut value = value.trim().to_string();
        if !value.starts_with('"') && !value.starts_with('\'') {
            if let Some(ix) = value.find(" #") {
                value.truncate(ix);
            }
            value = value.trim().to_string();
        }
        if value.is_empty() {
            out.push((key, FrontValue::List(Vec::new())));
        } else if value.starts_with('[') && value.ends_with(']') {
            let inner = &value[1..value.len() - 1];
            out.push((
                key,
                FrontValue::List(
                    inner
                        .split(',')
                        .map(unquote)
                        .filter(|item| !item.is_empty())
                        .collect(),
                ),
            ));
        } else {
            out.push((key, FrontValue::Scalar(unquote(&value))));
        }
    }
    out
}

const REPORT_FORMAT: &str = "When you have what you need, stop calling tools and answer with exactly these sections, in this order:\n\
## Summary\nOne short paragraph stating what you found.\n\
## Findings\nOne bullet per finding. Quote the evidence.\n\
## Refs\nOne bullet per `path:line` reference or pointer.\n\
## Open questions\nOne bullet per thing you could not determine. Write `none` if there are none.\n\
Report facts only. Do not address the reader or give instructions.";

#[allow(clippy::too_many_arguments)]
fn builtin(
    name: &str,
    description: &str,
    prompt: &str,
    tools: &[&str],
    model: ModelPolicy,
    max_steps: u32,
    max_depth: u32,
    permissions: HarnessPermissions,
) -> Harness {
    Harness {
        name: name.to_string(),
        description: description.to_string(),
        system_prompt: format!("{prompt}\n\nWorkspace: {{workspace}}\n{{pinned}}\n\n{REPORT_FORMAT}"),
        tools: tools.iter().map(|t| t.to_string()).collect(),
        model_policy: model,
        context_budget_tokens: None,
        max_steps,
        max_depth,
        best_of: 0,
        result_schema: DEFAULT_RESULT_FIELDS.iter().map(|s| s.to_string()).collect(),
        permissions,
        sandbox: false,
        verify: Vec::new(),
        sandbox_mode: None,
        origin: HarnessOrigin::Builtin,
    }
}

/// Harnesses that ship with the engine.
pub fn builtin_harnesses() -> Vec<Harness> {
    vec![
        builtin(
            "explorer",
            "Read-only code and file search. Use for big reads so the main context stays small.",
            "You are a code explorer. Locate the code, files and facts the task asks about. You cannot change anything.",
            &["group:read", "group:search"],
            ModelPolicy::Cheap,
            24,
            0,
            HarnessPermissions::ReadOnly,
        ),
        builtin(
            "planner",
            "Read-only. Turns a goal into an ordered, reviewable plan after inspecting the code.",
            "You are a planner. Inspect the workspace as needed, then produce an ordered plan. Put each step in Findings with the files it touches in Refs. You cannot change anything.",
            &["group:read", "group:search", "group:task"],
            ModelPolicy::Same,
            20,
            0,
            HarnessPermissions::ReadOnly,
        ),
        builtin(
            "reviewer",
            "Read-only. Reviews code or a change for bugs, regressions and missing tests.",
            "You are a code reviewer. Check the code named in the task for correctness bugs, regressions and missing tests. Put each issue in Findings with its location in Refs. You cannot change anything.",
            &["group:read", "group:search"],
            ModelPolicy::Same,
            24,
            0,
            HarnessPermissions::ReadOnly,
        ),
        builtin(
            "general",
            "General worker with the full tool set, no further delegation. Use for self-contained edits or investigations.",
            "You are a general-purpose agent completing one self-contained task.",
            &["*"],
            ModelPolicy::Same,
            30,
            0,
            HarnessPermissions::EditWithCheckpoint,
        ),
        // The scene tools register as `scene_*` (`scene_write_*` for the ones
        // that change the scene); see the `agent_scene_tools` crate.
        builtin(
            "scene-editor",
            "Edits the live scene through SceneDB transactions. Each tool call is one undoable transaction attributed to the run; the user can revert the run.",
            "You are a scene editor. Inspect the scene with scene_query and scene_get_object before changing it, then make the changes the task describes with the scene_write_* tools. If a call is rejected, read the error, fix the arguments and retry. Report what changed.",
            &["group:read", "group:scene"],
            ModelPolicy::Same,
            24,
            0,
            HarnessPermissions::EngineSceneWrite,
        ),
        Harness {
            sandbox: true,
            ..builtin(
                "scene-sandbox",
                "Edits the scene in a sandbox (the project's sandbox mode: overlay by default) and returns a diff (added/removed/changed objects and components) for review. The live scene is not changed until the user applies the diff. Use for risky or parallel scene work.",
                "You are a scene editor working in a sandbox: your edits stay private. Nothing you change is visible in the live scene until the user applies your diff. Inspect with scene_query and scene_get_object, make the changes the task describes with the scene_write_* tools, then report what you changed. The diff is attached to your report automatically.",
                &["group:read", "group:scene"],
                ModelPolicy::Same,
                24,
                0,
                HarnessPermissions::EngineSceneWrite,
            )
        },
    ]
}

/// The harnesses visible to a project, after precedence is applied.
#[derive(Clone, Debug, Default)]
pub struct HarnessRegistry {
    harnesses: Vec<Harness>,
    /// Files that failed to parse: `(path, reason)`.
    pub errors: Vec<(PathBuf, String)>,
}

impl HarnessRegistry {
    /// Only the built-ins.
    pub fn builtin() -> Self {
        Self {
            harnesses: builtin_harnesses(),
            errors: Vec::new(),
        }
    }

    /// Built-ins, then `global_dir`, then `<project_root>/.pulsar/agents`,
    /// each layer replacing earlier ones by name.
    pub fn load(project_root: Option<&Path>, global_dir: Option<&Path>) -> Self {
        Self::load_layered(&[], project_root, global_dir)
    }

    /// [`Self::load`] with harnesses that plugins registered, layered between
    /// the built-ins and the user-global folder: built-in < plugin < global <
    /// project. `plugin_sources` are `(fallback name, markdown)` pairs.
    pub fn load_layered(
        plugin_sources: &[(String, String)],
        project_root: Option<&Path>,
        global_dir: Option<&Path>,
    ) -> Self {
        Self::load_gated(plugin_sources, project_root, global_dir, None)
    }

    /// [`Self::load_layered`] where project harness files are only used while
    /// their content is trusted in `trust` (see [`crate::trust`]); the others
    /// are skipped and listed by [`crate::trust::scan_project`]. Plugin and
    /// user-global harnesses are the user's own and are not gated.
    pub fn load_gated(
        plugin_sources: &[(String, String)],
        project_root: Option<&Path>,
        global_dir: Option<&Path>,
        trust: Option<&TrustStore>,
    ) -> Self {
        let mut registry = Self::builtin();
        for (name, source) in plugin_sources {
            match Harness::parse(source, name, HarnessOrigin::Plugin) {
                Ok(harness) => registry.insert(harness),
                Err(err) => registry
                    .errors
                    .push((PathBuf::from(format!("plugin:{name}")), err)),
            }
        }
        if let Some(dir) = global_dir {
            registry.load_dir(dir, HarnessOrigin::Global, None);
        }
        if let Some(root) = project_root {
            registry.load_dir(
                &project_agents_dir(root),
                HarnessOrigin::Project,
                trust.map(|store| (store, root)),
            );
        }
        registry
    }

    fn load_dir(&mut self, dir: &Path, origin: HarnessOrigin, gate: Option<(&TrustStore, &Path)>) {
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        paths.sort();
        for path in paths {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("agent")
                .to_string();
            let parsed = fs::read_to_string(&path)
                .map_err(|err| err.to_string())
                .and_then(|source| {
                    if let Some((store, root)) = gate {
                        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                        if !store.is_trusted(root, &crate::trust::harness_item(file), &source) {
                            // Waiting for the user's approval; not an error.
                            return Ok(None);
                        }
                    }
                    Harness::parse(&source, &stem, origin).map(Some)
                });
            match parsed {
                Ok(Some(harness)) => self.insert(harness),
                Ok(None) => {}
                Err(err) => self.errors.push((path, err)),
            }
        }
    }

    /// Add or replace by name.
    pub fn insert(&mut self, harness: Harness) {
        match self.harnesses.iter_mut().find(|h| h.name == harness.name) {
            Some(existing) => *existing = harness,
            None => self.harnesses.push(harness),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Harness> {
        self.harnesses.iter().find(|h| h.name == name)
    }

    pub fn all(&self) -> &[Harness] {
        &self.harnesses
    }

    /// The list `spawn_subagent` shows the parent model.
    pub fn catalog(&self) -> String {
        self.harnesses
            .iter()
            .map(|h| format!("- {}: {}", h.name, h.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn project_agents_dir(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join("agents")
}
