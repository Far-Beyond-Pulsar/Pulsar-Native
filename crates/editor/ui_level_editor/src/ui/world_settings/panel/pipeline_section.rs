//! The level's render pipeline: one of Helio's built-in pipelines or a
//! render-graph asset from the project, chosen first in World Settings.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::SearchableList,
    popover::Popover,
    v_flex, ActiveTheme, Sizable,
};

use super::{mark_world_settings_dirty, WorldSettingsPanelImpl};
use crate::WorldSettingsPanel;
use engine_backend::subsystems::render::helio_renderer::pipelines;

/// Label for a pipeline id; an id that is not registered shows as itself.
pub(super) fn pipeline_label(id: &String) -> String {
    pipelines::list_pipelines()
        .into_iter()
        .find(|pipeline| &pipeline.id == id)
        .map_or_else(|| id.clone(), |pipeline| pipeline.label)
}

impl WorldSettingsPanelImpl {
    /// Registered pipeline ids, built-ins first.
    pub(super) fn pipeline_ids() -> Vec<String> {
        pipelines::list_pipelines().into_iter().map(|p| p.id).collect()
    }

    /// Keeps the picker in step with what is registered: a project's
    /// render-graph assets register after the panel exists.
    fn refresh_pipeline_items(&mut self, cx: &mut Context<WorldSettingsPanel>) {
        let ids = Self::pipeline_ids();
        if ids != self.pipeline_ids {
            self.pipeline_picker
                .update(cx, |picker, cx| picker.set_items(ids.clone(), cx));
            self.pipeline_ids = ids;
        }
    }

    pub(super) fn render_pipeline_section(
        &mut self,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        self.refresh_pipeline_items(cx);
        let requested = self.state.read().scene.world_settings.render_pipeline.clone();
        let resolved = pipelines::resolve_pipeline(requested.as_deref());
        let infos = pipelines::list_pipelines();
        let info = resolved
            .as_deref()
            .and_then(|id| infos.iter().find(|p| p.id == id));
        let display = info.map_or_else(|| "None".to_string(), |p| p.label.clone());
        let description = info.map(|p| p.description.clone()).unwrap_or_default();
        // A saved pipeline that no longer exists runs the default; say so.
        let missing = requested
            .as_deref()
            .filter(|id| !pipelines::list_pipelines().iter().any(|p| p.id == *id))
            .map(|id| format!("Pipeline `{id}` was not found; using {display}."));
        let picker = self.pipeline_picker.clone();

        v_flex()
            .gap_2()
            .child(
                Popover::<SearchableList<String>>::new("render-pipeline")
                    .anchor(Corner::BottomRight)
                    .trigger(
                        Button::new("render-pipeline-trigger")
                            .label(display)
                            .small()
                            .ghost()
                            .dropdown_caret(true),
                    )
                    .content(move |_window, _cx| picker.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(missing.unwrap_or(description)),
            )
            .into_any_element()
    }

    /// Registers the project's `.rendergraph` assets as pipelines, now and
    /// whenever one changes. Scanning reads the project, so it runs off the UI
    /// thread; the picker follows on the next render.
    pub(super) fn watch_render_graphs(&self, cx: &mut Context<WorldSettingsPanel>) {
        let mut events = engine_fs::subscribe();
        cx.spawn(async move |this, cx| {
            let mut reload = true;
            loop {
                if reload {
                    if let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from)
                    {
                        let report = cx
                            .background_executor()
                            .spawn(async move { pipelines::refresh_project_pipelines(&root) })
                            .await;
                        for (path, error) in &report.failed {
                            tracing::warn!("Render graph {path} is not offered as a pipeline: {error}");
                        }
                        cx.update(|cx| {
                            let _ = this.update(cx, |_, cx| cx.notify());
                        });
                    }
                }
                let Ok(event) = events.recv().await else {
                    reload = false;
                    continue;
                };
                reload = event.path.extension().and_then(|e| e.to_str())
                    == Some(pipelines::RENDER_GRAPH_EXTENSION);
            }
        })
        .detach();
    }

    /// Stores the picked pipeline in the level. The default is saved as
    /// `None` so a level that never chose follows the engine default.
    pub(super) fn select_pipeline(&mut self, id: &str, cx: &mut Context<WorldSettingsPanel>) {
        let mut state = self.state.write();
        let is_default = pipelines::resolve_pipeline(None).as_deref() == Some(id);
        state.scene.world_settings.render_pipeline = (!is_default).then(|| id.to_string());
        mark_world_settings_dirty(&mut state);
        cx.notify();
    }
}
