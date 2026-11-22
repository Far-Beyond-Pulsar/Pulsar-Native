//! What the settings screen does: rebuild its pages, search, navigate the
//! tree, jump to a section, follow the scroll, reset a setting, save.

use engine_state::{GlobalSettings, ProjectSettings};
use gpui::{px, Context, ListOffset, Window};
use ui::{notification::Notification, ContextModal as _};

use crate::components::SettingsScreen;
use crate::utils::model::{self, SettingId, Target, WindowKind, PLUGINS_FOLDER};
use crate::utils::store;

/// Rebuild every page from the settings database (the plugin set changed, or
/// the window opened), keeping the selection when its page still exists.
pub fn rebuild(screen: &mut SettingsScreen) {
    screen.plugin_generation = pulsar_plugins::generation();
    screen.all_pages = screen.build_pages();
    screen.fields.clear();
    refilter(screen);
}

/// Apply the search to the pages and refresh the tree and the content.
fn refilter(screen: &mut SettingsScreen) {
    screen.pages = model::filter_pages(&screen.all_pages, &screen.query);
    let selected_shown = screen
        .selected_page
        .as_deref()
        .is_some_and(|id| screen.pages.iter().any(|page| page.id == id));
    if !selected_shown {
        screen.selected_page = screen.pages.first().map(|page| page.id.clone());
        screen.selected_section = None;
    }
    refresh_tree(screen);
    refresh_content(screen);
}

/// The tree rows for the current pages and open rows.
pub fn refresh_tree(screen: &mut SettingsScreen) {
    screen.tree = model::tree_rows(&screen.pages, &screen.expanded, !screen.query.is_empty());
}

/// The content rows of the selected page, scrolled to its top.
fn refresh_content(screen: &mut SettingsScreen) {
    let (rows, headings) = match screen.current_page() {
        Some((_, page)) => model::content_rows(page),
        None => (Vec::new(), Vec::new()),
    };
    screen.rows = rows;
    screen.headings = headings;
    screen.content.reset(screen.rows.len());
}

/// The search text changed (`query` is lowercased and trimmed).
pub fn set_query(screen: &mut SettingsScreen, query: String) {
    if screen.query == query {
        return;
    }
    screen.query = query;
    refilter(screen);
}

/// Open or close a tree row.
pub fn toggle(screen: &mut SettingsScreen, id: &str) {
    if !screen.expanded.remove(id) {
        screen.expanded.insert(id.to_string());
    }
    refresh_tree(screen);
}

/// A tree row was clicked.
pub fn activate(screen: &mut SettingsScreen, target: Target, id: &str) {
    match target {
        Target::Folder => toggle(screen, id),
        Target::Page(ix) => {
            select_page(screen, ix);
            screen.content.scroll_to(ListOffset::default());
        }
        Target::Section(ix, section) => jump_to_section(screen, ix, section),
    }
}

/// Show page `ix` of the shown pages (from its top).
fn select_page(screen: &mut SettingsScreen, ix: usize) {
    let Some(page) = screen.pages.get(ix) else {
        return;
    };
    if screen.selected_page.as_deref() == Some(page.id.as_str()) {
        screen.selected_section = None;
        return;
    }
    screen.selected_page = Some(page.id.clone());
    screen.selected_section = None;
    screen.pending_jump = None;
    refresh_content(screen);
}

/// Show page `ix` scrolled so section `section`'s heading is at the top. On
/// the page already shown it scrolls now; on another page it waits for the
/// page's first layout, so the heading's position is measured, not guessed.
fn jump_to_section(screen: &mut SettingsScreen, ix: usize, section: usize) {
    let same_page = screen
        .pages
        .get(ix)
        .is_some_and(|page| screen.selected_page.as_deref() == Some(page.id.as_str()));
    select_page(screen, ix);
    screen.selected_section = Some(section);
    let Some(&row) = screen.headings.get(section) else {
        return;
    };
    if same_page {
        screen.content.scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(0.0),
        });
    } else {
        screen.pending_jump = Some(row);
    }
}

/// Select the page titled `title` (the footer's "settings" link names a
/// plugin), opening the Plugins folder when it is a plugin's.
pub fn select_page_titled(screen: &mut SettingsScreen, title: &str) {
    let title = title.strip_prefix("Plugin: ").unwrap_or(title);
    let Some(ix) = screen
        .pages
        .iter()
        .position(|page| page.title.eq_ignore_ascii_case(title))
    else {
        return;
    };
    if screen.pages[ix].plugin {
        screen.expanded.insert(PLUGINS_FOLDER.to_string());
    }
    screen.expanded.insert(screen.pages[ix].id.clone());
    select_page(screen, ix);
    refresh_tree(screen);
}

/// The content scrolled so `first` is its first visible row: highlight the
/// section that row is in. Returns whether the highlight moved.
pub fn follow_scroll(screen: &mut SettingsScreen, first: usize) -> bool {
    let section = screen.headings.iter().rposition(|&row| row <= first);
    if section != screen.selected_section {
        screen.selected_section = section;
        return true;
    }
    false
}

/// A setting took a new value: settings the editor shows itself take effect
/// now (the appearance, applied after the current event, so every window
/// can be refreshed).
pub fn applied(id: &SettingId, cx: &mut gpui::App) {
    use pulsar_settings::editor::appearance;
    if id.namespace == appearance::NS && id.owner == appearance::OWNER {
        cx.defer(ui_common::appearance::apply);
    }
}

/// A setting was written: the window has something to save.
pub fn mark_changed(screen: &mut SettingsScreen, requires_restart: bool) {
    screen.has_pending_changes = true;
    screen.restart_required |= requires_restart;
}

/// Put one setting back to its default and drop its field state (the field
/// is rebuilt showing the default).
pub fn reset_setting(
    screen: &mut SettingsScreen,
    id: &SettingId,
    requires_restart: bool,
    cx: &mut gpui::App,
) {
    match store::reset(id) {
        Ok(()) => {
            mark_changed(screen, requires_restart);
            applied(id, cx);
        }
        Err(error) => tracing::error!("reset {}: {error}", id.key_string()),
    }
    screen.fields.remove(id);
}

/// Write the window's settings to disk: the editor's, or the project's and
/// its plugins'.
pub fn save_pending_changes(
    screen: &mut SettingsScreen,
    window: &mut Window,
    cx: &mut Context<SettingsScreen>,
) {
    let mut errors = Vec::new();
    match screen.kind {
        WindowKind::Editor => {
            if let Err(error) = GlobalSettings::new().save_all() {
                tracing::error!("Error saving editor settings: {error:?}");
                errors.push(format!("Editor settings: {error}"));
            }
        }
        WindowKind::Project => match screen
            .project_path
            .as_deref()
            .and_then(ProjectSettings::new)
        {
            Some(project) => {
                if let Err(error) = project.save_all() {
                    tracing::error!("Error saving project settings: {error:?}");
                    errors.push(format!("Project settings: {error}"));
                }
                if let Err(error) = engine_state::plugin_settings::save() {
                    tracing::error!("Error saving plugin settings: {error:?}");
                    errors.push(format!("Plugin settings: {error:#}"));
                }
            }
            None => errors.push("No project is open: project settings cannot be saved.".into()),
        },
    }
    screen.has_pending_changes = !errors.is_empty();
    if errors.is_empty() && screen.restart_required {
        screen.restart_required = false;
        window.push_notification(
            Notification::info("Restart the editor to apply the settings you changed.")
                .title("Restart required"),
            cx,
        );
    }
    if !errors.is_empty() {
        window.push_notification(
            Notification::error(errors.join("\n")).title("Settings were not saved"),
            cx,
        );
    }
    cx.notify();
}
