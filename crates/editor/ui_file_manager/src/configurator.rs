//! Import configurator modal (issue #391).
//!
//! Shown when model files are dropped into the content drawer. Renders the
//! format's import-options schema using the engine's reflection-based property
//! editors; on confirm it converts each source to an engine-native `.mesh`
//! asset with the chosen options (the source file is not copied into the project).

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use pulsar_reflection::{Reflectable, RUNTIME_TYPE_REGISTRY};
use serde_json::Value as JsonValue;
use ui::notification::Notification;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, ContextModal as _, Sizable as _,
};
use ui_common::reflected_properties_panel::PropertyStateManager;
use ui_common::render_property_row_runtime;
use window_manager::{default_window_options, PulsarWindow};

use helio_component::mesh_cache::{self, ImportField};

/// Parameters for opening the import configurator as its own window: one
/// window per source format, covering every source of that format.
pub struct ImportConfiguratorParams {
    pub project_root: PathBuf,
    /// Source files already inside the project.
    pub sources: Vec<PathBuf>,
    /// Lower-case extension all `sources` share (e.g. `fbx`).
    pub ext: String,
    pub schema: mesh_cache::OptionsSchema,
}

thread_local! {
    /// Configurators waiting for the open one to close, so formats are
    /// configured one after another rather than in a pile of windows.
    static PENDING: std::cell::RefCell<std::collections::VecDeque<ImportConfiguratorParams>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    static OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Offer to import `sources` (project files), grouped by extension: one
/// configurator per format, shown in turn. Formats whose importer advertises
/// no options are linked straight away with defaults.
pub fn offer_import(
    project_root: PathBuf,
    by_ext: std::collections::BTreeMap<String, Vec<PathBuf>>,
    cx: &mut App,
) {
    for (ext, sources) in by_ext {
        let Some(schema) = asset_import::importer_for(&ext).and_then(|i| i.options_schema(&ext))
        else {
            asset_import::submit_import(
                project_root.clone(),
                sources,
                asset_import::ImportMode::Link,
                Arc::new(parking_lot::Mutex::new(HashMap::new())),
            );
            continue;
        };
        PENDING.with(|queue| {
            queue.borrow_mut().push_back(ImportConfiguratorParams {
                project_root: project_root.clone(),
                sources,
                ext,
                schema,
            })
        });
    }
    open_next(cx);
}

fn open_next(cx: &mut App) {
    use ui_common::PulsarWindowExt as _;
    if OPEN.with(|open| open.get()) {
        return;
    }
    if let Some(params) = PENDING.with(|queue| queue.borrow_mut().pop_front()) {
        OPEN.with(|open| open.set(true));
        ImportConfigurator::open(params, cx);
    }
}

pub struct ImportConfigurator {
    project_root: PathBuf,
    sources: Vec<PathBuf>,
    ext: String,
    fields: Vec<ImportField>,
    values_shared: Arc<parking_lot::Mutex<HashMap<String, Box<dyn Any + Send>>>>,
    /// Caches the current value of each field as JSON so that
    /// `render_field` can produce a `&dyn Any` that reflects the
    /// user's edits (via JSON deserialisation) rather than always
    /// passing the default — which would reset every editor each
    /// frame.
    field_json: Arc<Mutex<HashMap<String, JsonValue>>>,
    property_state: PropertyStateManager,
    focus_handle: FocusHandle,
}

impl ImportConfigurator {
    pub fn new(params: ImportConfiguratorParams, cx: &mut Context<Self>) -> Self {
        let values_shared = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let field_json = Arc::new(Mutex::new(HashMap::new()));

        // Whatever closes this window (a button, the title bar), the next
        // format's configurator opens.
        cx.on_release(|_this, cx| {
            OPEN.with(|open| open.set(false));
            open_next(cx);
        })
        .detach();

        Self {
            project_root: params.project_root,
            sources: params.sources,
            ext: params.ext,
            fields: params.schema.fields,
            values_shared,
            field_json,
            property_state: PropertyStateManager::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Queue one import task per source (they run in the Tasks window) and
    /// close. The user can follow progress and errors there.
    fn run_import(
        &mut self,
        mode: asset_import::ImportMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.sources.len();
        // The shared options map moves into the tasks; this window is done
        // with it.
        let options = std::mem::take(&mut *self.values_shared.lock());
        asset_import::submit_import(
            self.project_root.clone(),
            self.sources.clone(),
            mode,
            Arc::new(parking_lot::Mutex::new(options)),
        );
        window.push_notification(
            Notification::info(format!("Importing {count} file(s) — see Tasks")),
            cx,
        );
        window.remove_window();
    }

    /// Leave the sources alone and stop offering them.
    fn skip_forever(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if let Err(error) = asset_import::ignore_sources(&self.project_root, &self.sources) {
            tracing::warn!(%error, "could not record ignored import sources");
        }
        window.remove_window();
    }

    fn render_field(
        &mut self,
        field: &ImportField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vs = self.values_shared.clone();
        let fj = self.field_json.clone();
        let k = field.key.clone();
        let type_info = field.type_info;

        // Use the user-edited value (cached as JSON) when available, falling
        // back to the schema default.  Without this the editor's set_value
        // receives the default on every render, overwriting user edits.
        let current_value: Box<dyn Any> = fj
            .lock()
            .ok()
            .and_then(|guard| guard.get(&k).cloned())
            .and_then(|json| {
                RUNTIME_TYPE_REGISTRY
                    .deserialize_json_for_type(type_info, json)
                    .ok()
            })
            .unwrap_or_else(|| {
                // Clone the default through the JSON codec (the default is
                // Box<dyn Any + Send> and we need an un-send box for the
                // &dyn Any reference; the simplest path is round-trip through
                // the runtime registry).
                RUNTIME_TYPE_REGISTRY
                    .serialize_json_for_any(field.default.as_ref())
                    .ok()
                    .and_then(|json| {
                        RUNTIME_TYPE_REGISTRY
                            .deserialize_json_for_type(type_info, json)
                            .ok()
                    })
                    .unwrap_or_else(|| {
                        // The default MUST be a registered reflectable type.
                        // If we ever hit this something is deeply wrong.
                        tracing::error!("import field {} default not reflectable", field.key);
                        Box::new(())
                    })
            });

        let write_back = Arc::new(
            move |new_val: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
                {
                    let mut v = vs.lock();
                    v.insert(k.clone(), new_val);
                    if let Some(stored) = v.get(&k) {
                        if let Ok(json) =
                            RUNTIME_TYPE_REGISTRY.serialize_json_for_any(stored.as_ref())
                        {
                            if let Ok(mut j) = fj.lock() {
                                j.insert(k.clone(), json);
                            }
                        }
                    }
                }
            },
        );

        render_property_row_runtime(
            &mut self.property_state,
            "import",
            // One flat config panel -- the key IS the unique per-card identity.
            &field.key,
            &field.key,
            &field.label,
            &field.key,
            field.type_info,
            current_value.as_ref(),
            write_back,
            window,
            cx,
        )
    }
}

impl PulsarWindow for ImportConfigurator {
    type Params = ImportConfiguratorParams;

    fn window_name() -> &'static str {
        "ImportConfigurator"
    }

    fn window_options(_: &Self::Params) -> gpui::WindowOptions {
        default_window_options(600.0, 520.0)
    }

    fn build(
        params: Self::Params,
        _window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::Entity<Self> {
        cx.new(|cx| ImportConfigurator::new(params, cx))
    }
}

impl Focusable for ImportConfigurator {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImportConfigurator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.sources.len();
        let ext = self.ext.to_uppercase();
        let heading = if count == 1 {
            format!("Import {ext} file")
        } else {
            format!("Import {count} {ext} files")
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .p_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Link keeps the source file and re-imports when it changes. Convert in place replaces it with the native asset."),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .py_2()
                    .gap_2()
                    .overflow_y_scroll()
                    .children({
                        let fields = std::mem::take(&mut self.fields);
                        let result: Vec<_> = fields
                            .iter()
                            .map(|f| self.render_field(f, window, cx))
                            .collect();
                        self.fields = fields;
                        result
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .pt_3()
                    .child(
                        Button::new("cfg-skip")
                            .label("Don't import")
                            .outline()
                            .on_click(cx.listener(|this, _, w, cx| this.skip_forever(w, cx))),
                    )
                    .child(
                        Button::new("cfg-later").label("Later").outline().on_click(
                            cx.listener(|_this, _, w, _cx| {
                                w.remove_window();
                            }),
                        ),
                    )
                    .child(
                        Button::new("cfg-convert")
                            .label("Convert in place")
                            .outline()
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.run_import(asset_import::ImportMode::ConvertInPlace, w, cx)
                            })),
                    )
                    .child(
                        Button::new("cfg-link")
                            .label("Link")
                            .primary()
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.run_import(asset_import::ImportMode::Link, w, cx)
                            })),
                    ),
            )
    }
}
