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
use pulsar_reflection::{RUNTIME_TYPE_REGISTRY, Reflectable};
use serde_json::Value as JsonValue;
use ui::notification::Notification;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, ContextModal as _, Sizable as _,
};
use ui_common::reflected_properties_panel::PropertyStateManager;
use ui_common::render_property_row_runtime;
use window_manager::{PulsarWindow, default_window_options};

use pulsar_rendering::mesh_cache::{self, ImportField};

/// Parameters for opening the import configurator as its own window.
pub struct ImportConfiguratorParams {
    pub sources: Vec<PathBuf>,
    pub target: PathBuf,
    pub schema: mesh_cache::OptionsSchema,
}

pub struct ImportConfigurator {
    sources: Vec<PathBuf>,
    target: PathBuf,
    fields: Vec<ImportField>,
    values_shared: Arc<Mutex<HashMap<String, Box<dyn Any + Send>>>>,
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
    pub fn new(
        sources: Vec<PathBuf>,
        target: PathBuf,
        schema: mesh_cache::OptionsSchema,
        cx: &mut Context<Self>,
    ) -> Self {
        let values_shared = Arc::new(Mutex::new(HashMap::new()));
        let field_json = Arc::new(Mutex::new(HashMap::new()));

        Self {
            sources,
            target,
            fields: schema.fields,
            values_shared,
            field_json,
            property_state: PropertyStateManager::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn run_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values_shared.lock().unwrap();
        for src in &self.sources {
            let native = mesh_cache::native_mesh_path(&self.target, src);
            match mesh_cache::import_model_to_native(src, &native, &values) {
                Ok(_) => {
                    let n = native
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("mesh")
                        .to_string();
                    window.push_notification(Notification::success(format!("Imported \"{n}\"")), cx);
                }
                Err(e) => {
                    tracing::error!("Model import failed for {}: {}", src.display(), e);
                    let n = src
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("model")
                        .to_string();
                    window.push_notification(Notification::error(format!("Import failed: {n}")), cx);
                }
            }
        }
        drop(values);
        window.remove_window();
    }

    fn render_field(&mut self, field: &ImportField, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                        tracing::error!(
                            "import field {} default not reflectable",
                            field.key
                        );
                        Box::new(())
                    })
            });

        let write_back = Arc::new(move |new_val: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
            if let Ok(mut v) = vs.lock() {
                v.insert(k.clone(), new_val);
                // Drop the guard before re-locking below.
                drop(v);
            }
            if let Ok(g) = vs.lock() {
                if let Some(stored) = g.get(&k) {
                    if let Ok(json) =
                        RUNTIME_TYPE_REGISTRY.serialize_json_for_any(stored.as_ref())
                    {
                        drop(g);
                        if let Ok(mut j) = fj.lock() {
                            j.insert(k.clone(), json);
                        }
                    }
                }
            }
        });

        render_property_row_runtime(
            &mut self.property_state,
            "import",
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
        cx.new(|cx| ImportConfigurator::new(params.sources, params.target, params.schema, cx))
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
        let heading = if count == 1 {
            "Import model".to_string()
        } else {
            format!("Import {count} models")
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .gap_4()
            .w(px(480.))
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
                            .child("Set import options — the model is converted to an engine-native mesh asset."),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .overflow_y_scroll()
                    .max_h(px(400.))
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
                        Button::new("cfg-cancel").label("Cancel").outline().on_click(
                            cx.listener(|_this, _, w, _cx| {
                                w.remove_window();
                            }),
                        ),
                    )
                    .child(
                        Button::new("cfg-import")
                            .label("Import")
                            .primary()
                            .on_click(cx.listener(|this, _, w, cx| this.run_import(w, cx))),
                    ),
            )
    }
}
