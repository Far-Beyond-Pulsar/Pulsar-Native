//! The level's render pipeline: an asset picked in World Settings, first
//! section. The picker is the editor's shared [`AssetPicker`]: the project's
//! `.rendergraph` assets, plus the pipelines providers offer (Helio's shipped
//! ones) listed by id. "None" is a valid choice, and the default of a new
//! level: nothing runs, and the viewport says so.

use gpui::{prelude::*, *};
use helio::PipelineOrigin;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    v_flex, ActiveTheme, Sizable,
};
use ui_common::{AssetPickedEvent, AssetPicker, AssetQuery};

use super::{mark_world_settings_dirty, WorldSettingsPanelImpl};
use crate::WorldSettingsPanel;
use engine_backend::subsystems::render::helio_renderer::pipelines;

/// The picker and what it was made from; it is made again when either changes,
/// since an [`AssetPicker`] reads its items when it is created.
pub(super) struct PipelinePicker {
    picker: Entity<AssetPicker>,
    /// Ids providers offer, listed beside the project's render graphs.
    offered: Vec<String>,
    /// [`WorldSettingsPanelImpl::render_graph_epoch`] it was made at.
    epoch: u64,
    _subscription: Subscription,
}

/// Where a pipeline came from, as the panel says it.
fn origin_text(origin: PipelineOrigin) -> &'static str {
    match origin {
        PipelineOrigin::BuiltIn => "Offered by Helio",
        PipelineOrigin::Project => "Project render graph",
    }
}

impl WorldSettingsPanelImpl {
    /// The pipelines providers offer (not project assets: the picker finds
    /// those in the project), by id.
    fn offered_pipeline_ids() -> Vec<String> {
        pipelines::list_pipelines()
            .into_iter()
            .filter(|pipeline| pipeline.origin == PipelineOrigin::BuiltIn)
            .map(|pipeline| pipeline.id)
            .collect()
    }

    /// Keeps the picker in step with what is offered and what the project holds.
    fn ensure_pipeline_picker(
        &mut self,
        selected: &str,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> Entity<AssetPicker> {
        let offered = Self::offered_pipeline_ids();
        if let Some(existing) = &self.pipeline_picker {
            if existing.offered == offered && existing.epoch == self.render_graph_epoch {
                return existing.picker.clone();
            }
        }
        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let picker = cx.new(|cx| {
            AssetPicker::new(
                selected,
                offered.clone(),
                project_root,
                vec![AssetQuery::extension(pipelines::RENDER_GRAPH_EXTENSION)],
                window,
                cx,
            )
        });
        let subscription = cx.subscribe(&picker, |panel, picker, _: &AssetPickedEvent, cx| {
            let id = picker.read(cx).selected_path().to_owned();
            panel.world_settings.select_pipeline(Some(id), cx);
        });
        self.pipeline_picker = Some(PipelinePicker {
            picker: picker.clone(),
            offered,
            epoch: self.render_graph_epoch,
            _subscription: subscription,
        });
        picker
    }

    pub(super) fn render_pipeline_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let requested = self
            .state
            .read()
            .scene
            .world_settings
            .render_pipeline
            .clone();
        let picker = self.ensure_pipeline_picker(requested.as_deref().unwrap_or(""), window, cx);
        // What the engine runs for it: the pipeline, or why none does.
        let state = pipelines::pipeline_state(requested.as_deref());
        let info = state.id().and_then(|id| {
            pipelines::list_pipelines()
                .into_iter()
                .find(|pipeline| pipeline.id == id)
        });
        let display = match (&requested, &info) {
            (None, _) => "None".to_string(),
            (_, Some(info)) => info.label.clone(),
            (Some(id), None) => id.clone(),
        };
        let muted = cx.theme().muted_foreground;
        let problem = state.reason();
        let detail = match (&problem, &info) {
            (Some(reason), _) => reason.clone(),
            (None, Some(info)) => {
                format!("{}. {}", origin_text(info.origin), info.description)
            }
            (None, None) => String::new(),
        };
        let detail_color = if problem.is_some() {
            cx.theme().warning
        } else {
            muted
        };
        let entity = cx.entity();

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().text_color(muted).child("Pipeline"))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Popover::<AssetPicker>::new("render-pipeline")
                                    .anchor(Corner::BottomRight)
                                    .trigger(
                                        Button::new("render-pipeline-trigger")
                                            .label(display)
                                            .small()
                                            .ghost()
                                            .dropdown_caret(true),
                                    )
                                    .content({
                                        let picker = picker.clone();
                                        move |_window, _cx| picker.clone()
                                    }),
                            )
                            .when(requested.is_some(), |row| {
                                row.child(
                                    Button::new("render-pipeline-clear")
                                        .label("None")
                                        .small()
                                        .ghost()
                                        .tooltip("Run no render pipeline")
                                        .on_click(move |_, _window, cx| {
                                            picker.update(cx, |picker, _| {
                                                picker.set_selected_path(String::new())
                                            });
                                            entity.update(cx, |panel, cx| {
                                                panel.world_settings.select_pipeline(None, cx)
                                            });
                                        }),
                                )
                            }),
                    ),
            )
            .child(div().text_xs().text_color(detail_color).child(detail))
            .into_any_element()
    }

    /// Compiles the project's `.rendergraph` assets into pipelines, now and
    /// whenever one changes, so a saved graph reaches every viewport running it
    /// at the next frame boundary. Scanning reads the project through the VFS,
    /// off the UI thread; the picker follows on the next render.
    pub(super) fn watch_render_graphs(&self, cx: &mut Context<WorldSettingsPanel>) {
        let mut events = engine_fs::subscribe();
        cx.spawn(async move |this, cx| {
            let mut reload = true;
            loop {
                if reload {
                    if let Some(root) =
                        engine_state::get_project_path().map(std::path::PathBuf::from)
                    {
                        let report = cx
                            .background_executor()
                            .spawn(async move { pipelines::refresh_project_pipelines(&root) })
                            .await;
                        for (path, error) in &report.failed {
                            tracing::warn!(
                                "Render graph {path} is not offered as a pipeline: {error}"
                            );
                        }
                        cx.update(|cx| {
                            let _ = this.update(cx, |panel, cx| {
                                panel.world_settings.render_graph_epoch += 1;
                                cx.notify();
                            });
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

    /// Stores the picked pipeline in the level; `None` is no pipeline.
    pub(super) fn select_pipeline(
        &mut self,
        id: Option<String>,
        cx: &mut Context<WorldSettingsPanel>,
    ) {
        let mut state = self.state.write();
        let settings = &mut state.scene.world_settings;
        settings.render_pipeline = id.filter(|id| !id.is_empty());
        settings.pipeline_version = pipelines::PIPELINE_VERSION;
        mark_world_settings_dirty(&mut state);
        cx.notify();
    }
}
