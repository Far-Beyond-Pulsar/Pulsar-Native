//! The footer's one language server entry, "LSP Servers": the aggregate
//! state and progress of every server; a click lists each server (name, root,
//! state) and each opens a submenu of its actions (restart, stop, open log,
//! trace, the plugin's settings, and for a missing server its install). It is
//! a function of what `LanguageServers` knows: nothing here names a server.

use engine_state::plugin_settings::{self, SettingScope};
use engine_state::{global_config, ConfigValue};
use gpui::UpdateGlobal as _;
use gpui::{
    prelude::*, px, AnyElement, App, Context, Corner, Hsla, IntoElement,
};
use pulsar_language::TraceLevel;
use pulsar_lsp::{LanguageServer, LanguageServers, ServerId, ServerStatus};
use ui::{
    button::{Button, ButtonVariants as _},
    menu::PopupMenuItem,
    popup_menu::PopupMenuExt as _,
    ActiveTheme as _, Icon, IconName,
};

use super::PulsarApp;

/// Everything the footer shows of one server, read once per frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ServerEntry {
    pub id: ServerId,
    pub name: &'static str,
    /// The root's folder name, when several servers share the name.
    pub root_label: Option<String>,
    pub status: ServerStatus,
    pub detail: String,
    pub trace: TraceLevel,
    /// The server's managed install, when it has one: what the footer offers
    /// while the server is missing (the person approves it by choosing it).
    pub install: Option<InstallOffer>,
    /// The plugin settings group and page of this server.
    pub origin: &'static str,
    pub group: &'static str,
}

/// A managed install on offer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct InstallOffer {
    pub label: &'static str,
    /// What it will download and where it goes, shown before the person
    /// chooses it.
    pub description: String,
}

impl ServerEntry {
    /// What the footer prints for the state: `(text, detail)`.
    pub(super) fn summary(&self) -> (String, String) {
        match &self.status {
            ServerStatus::Idle => ("Idle".into(), String::new()),
            ServerStatus::Starting => ("Starting...".into(), self.detail.clone()),
            ServerStatus::Indexing { progress, message } => (
                if *progress > 0.0 && *progress < 1.0 {
                    format!("Indexing {:.0}%", progress * 100.0)
                } else {
                    "Indexing".to_string()
                },
                message.clone(),
            ),
            ServerStatus::Ready => ("Ready".into(), String::new()),
            ServerStatus::Error(error) => ("Error".into(), error.clone()),
            ServerStatus::Missing(message) => ("Not installed".into(), message.clone()),
            ServerStatus::Stopped => ("Stopped".into(), String::new()),
        }
    }

    /// The label on the entry's button: the name, with the root's folder when
    /// the project has several servers of one kind.
    pub(super) fn label(&self) -> String {
        match &self.root_label {
            Some(root) => format!("{} ({root})", self.name),
            None => self.name.to_string(),
        }
    }
}

/// The entries of every server, in creation order.
pub(super) fn entries(servers: &LanguageServers, cx: &App) -> Vec<ServerEntry> {
    let list: Vec<&gpui::Entity<LanguageServer>> = servers.servers().iter().collect();
    list.iter()
        .map(|server| {
            let server = server.read(cx);
            let shared = list
                .iter()
                .filter(|other| other.read(cx).id().server == server.id().server)
                .count()
                > 1;
            ServerEntry {
                id: server.id().clone(),
                name: server.spec().display_name,
                root_label: shared.then(|| {
                    server
                        .root()
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| server.root().display().to_string())
                }),
                status: server.status().clone(),
                detail: server.detail().to_string(),
                trace: server.settings().trace,
                install: server.installer().map(|installer| InstallOffer {
                    label: installer.label,
                    description: (installer.describe)(),
                }),
                origin: server.language().origin,
                group: server.spec().id,
            }
        })
        .collect()
}

/// The footer's thin progress line: the furthest-along server still working.
pub(super) fn progress_of(entries: &[ServerEntry]) -> f32 {
    entries
        .iter()
        .filter_map(|entry| match &entry.status {
            ServerStatus::Indexing { progress, .. } if *progress > 0.0 && *progress < 1.0 => {
                Some(*progress)
            }
            _ => None,
        })
        .fold(0.0, f32::max)
}

/// The one footer entry's label.
pub(super) const FOOTER_LABEL: &str = "LSP Servers";

/// How bad the worst server is, worst last (so `max` is the worst).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Health {
    Idle,
    Ready,
    Working,
    Missing,
    Error,
}

fn health_of(status: &ServerStatus) -> Health {
    match status {
        ServerStatus::Idle | ServerStatus::Stopped => Health::Idle,
        ServerStatus::Ready => Health::Ready,
        ServerStatus::Starting | ServerStatus::Indexing { .. } => Health::Working,
        ServerStatus::Missing(_) => Health::Missing,
        ServerStatus::Error(_) => Health::Error,
    }
}

/// What the single footer entry shows of every server together.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Aggregate {
    pub health: Health,
    /// The state after the label: the worst server's, with a count when
    /// several share it ("2 errors", "Indexing 45%", "Ready").
    pub text: String,
    pub progress: f32,
}

/// The aggregate of all servers; `None` while there are none (nothing to show).
pub(super) fn aggregate(entries: &[ServerEntry]) -> Option<Aggregate> {
    let health = entries.iter().map(|entry| health_of(&entry.status)).max()?;
    let count = entries
        .iter()
        .filter(|entry| health_of(&entry.status) == health)
        .count();
    let progress = progress_of(entries);
    let text = match health {
        Health::Error if count == 1 => "1 error".to_string(),
        Health::Error => format!("{count} errors"),
        Health::Missing => format!("{count} not installed"),
        Health::Working if progress > 0.0 => format!("Indexing {:.0}%", progress * 100.0),
        Health::Working => "Starting".to_string(),
        Health::Ready => "Ready".to_string(),
        Health::Idle => "Idle".to_string(),
    };
    Some(Aggregate { health, text, progress })
}

/// One server's row in the dropdown: name, root and state.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ServerRow {
    pub label: String,
    pub root: String,
    pub state: String,
}

pub(super) fn rows(entries: &[ServerEntry]) -> Vec<ServerRow> {
    entries
        .iter()
        .map(|entry| ServerRow {
            label: entry.label(),
            root: entry.id.root.display().to_string(),
            state: entry.summary().0,
        })
        .collect()
}

/// What choosing a server's menu item does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Action {
    Start,
    Restart,
    Stop,
    OpenLog,
    /// The spec's managed install (the person approves by choosing it).
    Install,
    SetTrace(TraceLevel),
    /// The plugin's settings page.
    OpenSettings,
    /// Pick the server's executable with a file dialog and use it.
    SetPath,
    /// Leave an approval step without doing anything.
    Dismiss,
}

/// One line of a server's submenu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuLine {
    Label(String),
    Separator,
    Item { label: String, action: Action, checked: Option<bool> },
    /// A nested menu (the install's approval step).
    Submenu { label: String, lines: Vec<MenuLine> },
}

/// The widest a server's menu gets: long texts wrap inside it instead of
/// running past the edge.
pub(super) const MENU_MAX_WIDTH: f32 = 400.0;

/// Characters per line that fit `MENU_MAX_WIDTH` at the menu's text size.
pub(super) const WRAP_COLUMNS: usize = 52;

/// `text` broken into lines of at most `columns` characters at spaces (a word
/// longer than that gets a line of its own).
pub(super) fn wrap_text(text: &str, columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

fn wrapped_labels(text: &str) -> impl Iterator<Item = MenuLine> {
    wrap_text(text, WRAP_COLUMNS).into_iter().map(MenuLine::Label)
}

/// The submenu of one server, from its state alone: a generic function of
/// what `LanguageServers` knows, so any plugin's server gets the same one.
pub(super) fn server_menu_lines(entry: &ServerEntry) -> Vec<MenuLine> {
    let item = |label: &str, action| MenuLine::Item {
        label: label.to_string(),
        action,
        checked: None,
    };
    let (state, detail) = entry.summary();
    let mut lines = vec![MenuLine::Label(state)];
    lines.extend(wrapped_labels(&detail));
    lines.extend(wrapped_labels(&entry.id.root.display().to_string()));
    lines.push(MenuLine::Separator);
    if matches!(entry.status, ServerStatus::Missing(_)) {
        // Say what to do about it: install (when the spec offers one), set a
        // path, or look again.
        if let Some(offer) = &entry.install {
            // Nothing is downloaded by one click: the item opens an approval
            // step that says what will happen, and only its button installs.
            let mut approval = vec![MenuLine::Label("Install?".to_string())];
            approval.extend(wrapped_labels(&offer.description));
            approval.push(MenuLine::Separator);
            approval.push(item("Download and install", Action::Install));
            approval.push(item("Cancel", Action::Dismiss));
            lines.push(MenuLine::Submenu {
                label: format!("{}...", offer.label),
                lines: approval,
            });
        }
        lines.push(item("Set executable path...", Action::SetPath));
        lines.push(item("Check again", Action::Restart));
        lines.push(MenuLine::Separator);
    } else if entry.status.is_running() {
        lines.push(item("Restart", Action::Restart));
        lines.push(item("Stop", Action::Stop));
    } else {
        lines.push(item("Start", Action::Start));
    }
    lines.push(item("Open Log", Action::OpenLog));
    lines.push(MenuLine::Separator);
    lines.push(MenuLine::Label("Trace".to_string()));
    for (label, level) in [
        ("Off", TraceLevel::Off),
        ("Messages", TraceLevel::Messages),
        ("Verbose", TraceLevel::Verbose),
    ] {
        lines.push(MenuLine::Item {
            label: label.to_string(),
            action: Action::SetTrace(level),
            checked: Some(entry.trace == level),
        });
    }
    lines.push(MenuLine::Separator);
    lines.push(item("Settings...", Action::OpenSettings));
    lines
}

fn status_style(health: Health, cx: &App) -> (Hsla, IconName) {
    match health {
        Health::Ready => (cx.theme().success, IconName::CheckCircle),
        Health::Working => (cx.theme().warning, IconName::Loader),
        Health::Error => (cx.theme().danger, IconName::TriangleAlert),
        Health::Missing => (cx.theme().warning, IconName::TriangleAlert),
        Health::Idle => (cx.theme().muted_foreground, IconName::Circle),
    }
}

/// Persist `trace` for a server as the person's own setting; the service
/// applies it to the running server when the setting changes.
fn set_trace(origin: &'static str, group: &str, trace: TraceLevel) {
    let owner = pulsar_settings::plugin::owner_of(origin, group);
    let value = ConfigValue::String(trace.as_str().to_string());
    match plugin_settings::set(&owner, "trace", SettingScope::User, value) {
        Ok(()) => {
            if let Err(error) = plugin_settings::save() {
                tracing::warn!("could not save the trace level: {error:#}");
            }
        }
        Err(error) => tracing::warn!("could not set the trace level: {error:#}"),
    }
}

/// Persist the executable the person picked as their own `path` setting.
fn set_path(origin: &'static str, group: &str, path: &std::path::Path) {
    let owner = pulsar_settings::plugin::owner_of(origin, group);
    let value = ConfigValue::String(path.to_string_lossy().into_owned());
    match plugin_settings::set(&owner, "path", SettingScope::User, value) {
        Ok(()) => {
            if let Err(error) = plugin_settings::save() {
                tracing::warn!("could not save the executable path: {error:#}");
            }
        }
        Err(error) => tracing::warn!("could not set the executable path: {error:#}"),
    }
}

/// Run `action` for the server `entry`.
fn perform(
    action: &Action,
    entry: &ServerEntry,
    servers: &gpui::Entity<LanguageServers>,
    project: Option<std::path::PathBuf>,
    cx: &mut App,
) {
    let id = entry.id.clone();
    match action {
        Action::Start => servers.update(cx, |servers, cx| servers.start(&id, cx)),
        Action::Restart => servers.update(cx, |servers, cx| servers.restart(&id, cx)),
        Action::Stop => servers.update(cx, |servers, cx| servers.stop(&id, cx)),
        Action::Install => servers.update(cx, |servers, cx| servers.install(&id, cx)),
        Action::OpenLog => {
            window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                registry.open("EditorTasksWindow", cx)
            });
        }
        Action::SetTrace(level) => set_trace(entry.origin, entry.group, *level),
        Action::Dismiss => {}
        Action::SetPath => {
            let (servers, origin, group) = (servers.clone(), entry.origin, entry.group);
            let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Select the language server executable".into()),
            });
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(paths))) = picked.await else {
                    return;
                };
                let Some(path) = paths.first() else {
                    return;
                };
                set_path(origin, group, path);
                let _ = cx.update(|cx| servers.update(cx, |servers, cx| servers.restart(&id, cx)));
            })
            .detach();
        }
        Action::OpenSettings => {
            let owner = pulsar_settings::plugin::page_key(entry.origin);
            let title = pulsar_settings::plugin::page_title(global_config(), &owner);
            ui_settings::SettingsWindow::open_page(project, format!("Plugin: {title}"), cx);
        }
    }
}

impl PulsarApp {
    /// The one footer entry, "LSP Servers": the aggregate state and progress;
    /// a click lists every server, each with a submenu of its actions. The
    /// list is whatever `LanguageServers` holds, so a plugin that contributes
    /// a server appears here with no UI of its own.
    pub(super) fn render_language_server_entries(
        &self,
        entries: &[ServerEntry],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(summary) = aggregate(entries) else {
            return Vec::new();
        };
        let (color, icon) = status_style(summary.health, cx);
        let servers = self.state.language_servers.clone();
        let project = self.state.project_path.clone();
        let tooltip = rows(entries)
            .iter()
            .map(|row| format!("{} - {} - {}", row.label, row.state, row.root))
            .collect::<Vec<_>>()
            .join("\n");
        let entries = entries.to_vec();

        vec![Button::new("language-servers")
            .ghost()
            .icon(Icon::new(icon).size(px(14.)).text_color(color))
            .label(format!("{FOOTER_LABEL}: {}", summary.text))
            .px_2()
            .py_1()
            .rounded(px(4.))
            .tooltip(tooltip)
            .popup_menu_with_anchor(Corner::TopLeft, move |menu, window, cx| {
                servers_menu(menu, &entries, &servers, project.clone(), window, cx)
            })
            .into_any_element()]
    }
}

/// Fill a menu from `lines`, within the menu's maximum width.
fn fill_submenu(
    menu: ui::popup_menu::PopupMenu,
    lines: &[MenuLine],
    entry: &ServerEntry,
    servers: &gpui::Entity<LanguageServers>,
    project: &Option<std::path::PathBuf>,
    window: &mut gpui::Window,
    cx: &mut Context<ui::popup_menu::PopupMenu>,
) -> ui::popup_menu::PopupMenu {
    let mut menu = menu.max_w(px(MENU_MAX_WIDTH));
    for line in lines {
        menu = match line {
            MenuLine::Label(text) => menu.label(text.clone()),
            MenuLine::Separator => menu.separator(),
            MenuLine::Item { label, action, checked } => {
                let (action, entry, servers, project) =
                    (action.clone(), entry.clone(), servers.clone(), project.clone());
                menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(checked.unwrap_or(false))
                        .on_click(move |_, _, cx| perform(&action, &entry, &servers, project.clone(), cx)),
                )
            }
            MenuLine::Submenu { label, lines } => {
                let (lines, entry, servers, project) =
                    (lines.clone(), entry.clone(), servers.clone(), project.clone());
                menu.submenu(label.clone(), window, cx, move |submenu, window, cx| {
                    fill_submenu(submenu, &lines, &entry, &servers, &project, window, cx)
                })
            }
        };
    }
    menu
}

/// The dropdown: every server, each opening its own submenu.
fn servers_menu(
    menu: ui::popup_menu::PopupMenu,
    entries: &[ServerEntry],
    servers: &gpui::Entity<LanguageServers>,
    project: Option<std::path::PathBuf>,
    window: &mut gpui::Window,
    cx: &mut Context<ui::popup_menu::PopupMenu>,
) -> ui::popup_menu::PopupMenu {
    let mut menu = menu.label(FOOTER_LABEL).separator();
    for (entry, row) in entries.iter().zip(rows(entries)) {
        let entry = entry.clone();
        let servers = servers.clone();
        let project = project.clone();
        let label = format!("{} - {}", row.label, row.state);
        menu = menu.submenu(label, window, cx, move |submenu, window, cx| {
            fill_submenu(submenu, &server_menu_lines(&entry), &entry, &servers, &project, window, cx)
        });
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(status: ServerStatus) -> ServerEntry {
        ServerEntry {
            id: ServerId { server: "rust-analyzer", root: PathBuf::from("/p") },
            name: "rust-analyzer",
            root_label: None,
            status,
            detail: "Installing rust-analyzer...".into(),
            trace: TraceLevel::Off,
            install: None,
            origin: "engine:pulsar_lsp",
            group: "rust-analyzer",
        }
    }

    #[test]
    fn each_state_has_its_footer_text() {
        assert_eq!(entry(ServerStatus::Idle).summary().0, "Idle");
        assert_eq!(
            entry(ServerStatus::Starting).summary(),
            ("Starting...".to_string(), "Installing rust-analyzer...".to_string())
        );
        assert_eq!(
            entry(ServerStatus::Indexing { progress: 0.5, message: "crate x".into() }).summary(),
            ("Indexing 50%".to_string(), "crate x".to_string())
        );
        assert_eq!(entry(ServerStatus::Ready).summary().0, "Ready");
        assert_eq!(
            entry(ServerStatus::Error("boom".into())).summary(),
            ("Error".to_string(), "boom".to_string())
        );
        // A missing server says what is missing; the footer offers its install.
        assert_eq!(
            entry(ServerStatus::Missing("Node.js was not found".into())).summary(),
            ("Not installed".to_string(), "Node.js was not found".to_string())
        );
    }

    #[test]
    fn servers_of_one_kind_are_told_apart_by_their_root() {
        let mut shared = entry(ServerStatus::Ready);
        shared.root_label = Some("game".into());
        assert_eq!(shared.label(), "rust-analyzer (game)");
        assert_eq!(entry(ServerStatus::Ready).label(), "rust-analyzer");
    }

    fn named(name: &'static str, status: ServerStatus) -> ServerEntry {
        let mut e = entry(status);
        e.id.server = name;
        e.name = name;
        e
    }

    #[test]
    fn the_one_entry_shows_the_worst_state_of_all_servers() {
        assert_eq!(aggregate(&[]), None, "no servers, no entry");
        let ready = named("a", ServerStatus::Ready);
        let working = named("b", ServerStatus::Indexing { progress: 0.45, message: String::new() });
        let missing = named("c", ServerStatus::Missing("no node".into()));
        let broken = named("d", ServerStatus::Error("boom".into()));
        let both_broken = named("e", ServerStatus::Error("bang".into()));

        let all_ready = aggregate(&[ready.clone()]).unwrap();
        assert_eq!((all_ready.health, all_ready.text.as_str()), (Health::Ready, "Ready"));

        let busy = aggregate(&[ready.clone(), working.clone()]).unwrap();
        assert_eq!((busy.health, busy.text.as_str()), (Health::Working, "Indexing 45%"));
        assert_eq!(busy.progress, 0.45);

        let not_installed = aggregate(&[ready.clone(), working, missing.clone()]).unwrap();
        assert_eq!(
            (not_installed.health, not_installed.text.as_str()),
            (Health::Missing, "1 not installed")
        );

        let errors = aggregate(&[ready, missing, broken.clone(), both_broken]).unwrap();
        assert_eq!((errors.health, errors.text.as_str()), (Health::Error, "2 errors"));
        assert_eq!(aggregate(&[broken]).unwrap().text, "1 error");
        assert_eq!(FOOTER_LABEL, "LSP Servers");
    }

    #[test]
    fn the_dropdown_lists_every_server_with_name_root_and_state() {
        let mut shared = named("typescript-language-server", ServerStatus::Missing("x".into()));
        shared.root_label = Some("classes".into());
        let listed = rows(&[named("rust-analyzer", ServerStatus::Ready), shared]);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].label, "rust-analyzer");
        assert_eq!(listed[0].state, "Ready");
        assert_eq!(listed[1].label, "typescript-language-server (classes)");
        assert_eq!(listed[1].state, "Not installed");
        assert_eq!(listed[1].root, "/p");
    }

    fn actions(lines: &[MenuLine]) -> Vec<Action> {
        lines
            .iter()
            .filter_map(|line| match line {
                MenuLine::Item { action, .. } => Some(action.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_running_servers_submenu_restarts_stops_logs_traces_and_links_to_settings() {
        let lines = server_menu_lines(&named("a", ServerStatus::Ready));
        let actions = actions(&lines);
        assert!(actions.contains(&Action::Restart) && actions.contains(&Action::Stop));
        assert!(!actions.contains(&Action::Start) && !actions.contains(&Action::Install));
        assert!(actions.contains(&Action::OpenLog) && actions.contains(&Action::OpenSettings));
        let traces: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                MenuLine::Item { action: Action::SetTrace(level), checked, .. } => {
                    Some((*level, *checked))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            traces,
            vec![
                (TraceLevel::Off, Some(true)),
                (TraceLevel::Messages, Some(false)),
                (TraceLevel::Verbose, Some(false)),
            ]
        );
    }

    #[test]
    fn a_stopped_server_can_be_started() {
        let actions = actions(&server_menu_lines(&named("a", ServerStatus::Stopped)));
        assert!(actions.contains(&Action::Start) && !actions.contains(&Action::Stop));
    }

    #[test]
    fn a_missing_server_says_why_and_offers_install_and_a_path() {
        let mut missing =
            named("typescript-language-server", ServerStatus::Missing("Node.js was not found".into()));
        missing.install = Some(InstallOffer {
            label: "Install typescript-language-server",
            description: "Downloads typescript-language-server 4.4.0".into(),
        });
        let lines = server_menu_lines(&missing);
        assert!(
            lines.contains(&MenuLine::Label("Node.js was not found".into())),
            "the message is reachable here"
        );
        // One click never installs: the offer is an approval step with its own button.
        assert!(!actions(&lines).contains(&Action::Install), "not directly in the menu");
        let approval = lines
            .iter()
            .find_map(|line| match line {
                MenuLine::Submenu { label, lines } => Some((label, lines)),
                _ => None,
            })
            .expect("an approval step");
        assert_eq!(approval.0, "Install typescript-language-server...");
        assert!(actions(approval.1).contains(&Action::Install) && actions(approval.1).contains(&Action::Dismiss));
        let said: String = approval
            .1
            .iter()
            .filter_map(|l| if let MenuLine::Label(t) = l { Some(t.as_str()) } else { None })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(said.contains("Downloads typescript-language-server 4.4.0"), "{said}");
        let found = actions(&lines);
        assert!(found.contains(&Action::SetPath), "a file dialog picks the executable");
        // Without an install on offer the path remains.
        let bare = actions(&server_menu_lines(&named("b", ServerStatus::Missing("m".into()))));
        assert!(!bare.contains(&Action::Install) && bare.contains(&Action::SetPath));
    }

    #[test]
    fn long_texts_wrap_inside_the_menus_width() {
        let long = "The TypeScript language server (typescript-language-server) was not found: it is not in the project's node_modules and not on PATH. Use Install on this entry to download a pinned, checksum-verified copy, or run `npm i -g typescript typescript-language-server`, or set its path in Settings.";
        let wrapped = wrap_text(long, WRAP_COLUMNS);
        assert!(wrapped.len() > 3);
        assert!(wrapped.iter().all(|l| l.chars().count() <= WRAP_COLUMNS), "{wrapped:?}");
        assert_eq!(wrapped.join(" "), long, "nothing is lost");
        // A word longer than a line gets its own line rather than being cut.
        assert_eq!(wrap_text("aaaa bbbbbbbbbbbb cc", 6), ["aaaa", "bbbbbbbbbbbb", "cc"]);

        // Every label of a server's menu fits, whatever the message.
        let mut missing = named("tsls", ServerStatus::Missing(long.into()));
        missing.install = Some(InstallOffer {
            label: "Install it",
            description: "Downloads typescript-language-server 4.4.0 and typescript 5.9.3 from the npm registry (registry.npmjs.org), checks each against a digest pinned in this plugin, and unpacks them into a folder.".into(),
        });
        fn all_labels<'a>(lines: &'a [MenuLine], out: &mut Vec<&'a str>) {
            for line in lines {
                match line {
                    MenuLine::Label(t) => out.push(t),
                    MenuLine::Submenu { lines, .. } => all_labels(lines, out),
                    _ => {}
                }
            }
        }
        let mut labels = Vec::new();
        let menu = server_menu_lines(&missing);
        all_labels(&menu, &mut labels);
        assert!(labels.iter().all(|l| l.chars().count() <= WRAP_COLUMNS), "{labels:?}");
        assert!(MENU_MAX_WIDTH <= 420.0);
    }
}
