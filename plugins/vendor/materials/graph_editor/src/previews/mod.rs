//! Live texture previews on node outputs (the toolbar's debug toggle).
//!
//! A previewable colour output gets an extra row under its pin, reserved by the
//! profile's presenter through Graphy's row hooks (see [`crate::profile`]). The
//! row's content is a GPUI element supplied through `GpuiProfile::render_slot`:
//! a [`PinPreview`] entity that owns a small GPU surface, compiles the pin's
//! shader on a worker and draws it every frame.
//!
//! Graphy decides where the row is and how tall it is; this module only fills it.

pub mod renderer;

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::*;
use graphy::GraphDescription;
use graphy_gpui::canvas::GraphCanvas;

use crate::editor::panel::ShaderEditorPanel;
use crate::features::compilation::lowering::{compile_preview_wgsl_for_pin, node_title};
use crate::features::compilation::CompilationState;
use crate::types::{is_texture_previewable, type_name};
use renderer::PinPreviewRenderer;

/// Height of the preview row, in graph units.
pub const PREVIEW_ROW_H: f32 = 56.0;
/// Narrowest card that shows a preview comfortably, in graph units.
pub const PREVIEW_MIN_WIDTH: f32 = 220.0;
/// Edge of the preview's backing texture, in texels.
const PIN_PREVIEW_TEXELS: u32 = 96;
/// Quiet period after the last graph edit before a preview rebuilds, so dragging
/// a slider-style property rebuilds once it settles instead of on every frame.
const PIN_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(120);
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// A preview that failed to compile or build.
#[derive(Clone, Debug)]
pub struct PinPreviewError {
    /// Human-readable pin name, e.g. `Noise.result`.
    pub label: String,
    pub message: String,
}

/// Hash of everything a preview's shader depends on.
pub fn graph_preview_signature(graph: &GraphDescription) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut ids: Vec<&String> = graph.nodes.keys().collect();
    ids.sort();
    for id in ids {
        let node = &graph.nodes[id];
        node.id.hash(&mut hasher);
        node.node_type.hash(&mut hasher);
        for pin in node.inputs.iter().chain(&node.outputs) {
            pin.id.hash(&mut hasher);
            type_name(pin.data_type()).hash(&mut hasher);
        }
        let mut props: Vec<_> = node.properties.iter().collect();
        props.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in props {
            key.hash(&mut hasher);
            value.to_string().hash(&mut hasher);
        }
    }
    for conn in &graph.connections {
        conn.source_node.hash(&mut hasher);
        conn.source_pin.hash(&mut hasher);
        conn.target_node.hash(&mut hasher);
        conn.target_pin.hash(&mut hasher);
    }
    hasher.finish()
}

/// Coalesces rapid graph edits into one rebuild (a trailing-edge debounce). Pure
/// state: the preview feeds it the graph signature on every frame, and frames keep
/// ticking while previews are shown, so the final state is always built once edits
/// stop.
#[derive(Default)]
pub struct PreviewDebounce {
    signature: Option<u64>,
    changed_at: Option<Instant>,
}

impl PreviewDebounce {
    /// Observe the graph's `signature` at `now`. True once it has not changed for
    /// `delay`; the first state seen builds at once, since there are no edits to
    /// coalesce.
    pub fn settled(&mut self, signature: u64, now: Instant, delay: Duration) -> bool {
        match self.signature {
            Some(seen) if seen == signature => {}
            Some(_) => {
                self.signature = Some(signature);
                self.changed_at = Some(now);
            }
            None => self.signature = Some(signature),
        }
        self.changed_at
            .map_or(true, |at| now.saturating_duration_since(at) >= delay)
    }
}

/// The previews of one editor: the on/off switch the presenter reads, the
/// previews by node and output, and the failures to list in the compiler output.
pub struct PinPreviews {
    enabled: Arc<AtomicBool>,
    canvas: RefCell<Option<WeakEntity<GraphCanvas>>>,
    editor: RefCell<Option<WeakEntity<ShaderEditorPanel>>>,
    views: RefCell<HashMap<(String, u32), Entity<PinPreview>>>,
    errors: Rc<RefCell<HashMap<(String, u32), PinPreviewError>>>,
}

impl PinPreviews {
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            canvas: RefCell::new(None),
            editor: RefCell::new(None),
            views: RefCell::new(HashMap::new()),
            errors: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// The flag the presenter reads to reserve preview rows.
    pub fn enabled_flag(&self) -> Arc<AtomicBool> {
        self.enabled.clone()
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.views.borrow_mut().clear();
            self.errors.borrow_mut().clear();
        }
    }

    /// Tells the previews which canvas holds the document and which editor lists
    /// their failures.
    pub fn attach(&self, canvas: WeakEntity<GraphCanvas>, editor: WeakEntity<ShaderEditorPanel>) {
        *self.canvas.borrow_mut() = Some(canvas);
        *self.editor.borrow_mut() = Some(editor);
    }

    /// Current failures, ordered by label.
    pub fn errors(&self) -> Vec<PinPreviewError> {
        let mut errors: Vec<_> = self.errors.borrow().values().cloned().collect();
        errors.sort_by(|a, b| a.label.cmp(&b.label));
        errors
    }

    /// The content of the preview row `slot` (the output index) of `node`.
    ///
    /// Runs while the canvas renders, so it takes the canvas's `graph` instead
    /// of reading the canvas entity, which is mid-update.
    pub fn slot(
        &self,
        graph: &GraphDescription,
        node: &str,
        slot: u32,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if !self.enabled() {
            return None;
        }
        let canvas = self.canvas.borrow().clone()?;
        let editor = self.editor.borrow().clone()?;
        let key = (node.to_string(), slot);
        if let Some(view) = self.views.borrow().get(&key) {
            return Some(view.clone().into_any_element());
        }
        // Forget previews of nodes that are gone before adding another.
        self.views.borrow_mut().retain(|(id, slot), _| {
            graph.nodes.get(id).is_some_and(|n| {
                n.outputs
                    .get(*slot as usize)
                    .is_some_and(|pin| is_texture_previewable(pin.data_type()))
            })
        });
        {
            let live = self.views.borrow();
            self.errors.borrow_mut().retain(|key, _| live.contains_key(key));
        }
        let errors = self.errors.clone();
        let view = cx.new(|_| PinPreview::new(key.clone(), canvas, editor, errors));
        self.views.borrow_mut().insert(key, view.clone());
        Some(view.into_any_element())
    }
}

impl Default for PinPreviews {
    fn default() -> Self {
        Self::new()
    }
}

/// A pin preview being built on a worker for one graph state. Kept after the
/// build finishes (or fails) so the same state is not rebuilt every frame;
/// replaced when the graph changes, which cancels the superseded build.
struct PendingBuild {
    signature: u64,
    _task: Task<()>,
}

struct Built {
    signature: u64,
    renderer: PinPreviewRenderer,
}

/// One live preview: a GPU surface showing the shader of one output pin.
pub struct PinPreview {
    key: (String, u32),
    canvas: WeakEntity<GraphCanvas>,
    editor: WeakEntity<ShaderEditorPanel>,
    errors: Rc<RefCell<HashMap<(String, u32), PinPreviewError>>>,
    surface: Option<WgpuSurfaceHandle>,
    built: Option<Built>,
    pending: Option<PendingBuild>,
    debounce: PreviewDebounce,
    started: Instant,
    driver: Rc<Cell<u64>>,
}

impl PinPreview {
    fn new(
        key: (String, u32),
        canvas: WeakEntity<GraphCanvas>,
        editor: WeakEntity<ShaderEditorPanel>,
        errors: Rc<RefCell<HashMap<(String, u32), PinPreviewError>>>,
    ) -> Self {
        Self {
            key,
            canvas,
            editor,
            errors,
            surface: None,
            built: None,
            pending: None,
            debounce: PreviewDebounce::default(),
            started: Instant::now(),
            driver: Rc::new(Cell::new(0)),
        }
    }

    /// `Noise.result`-style name of the previewed pin.
    fn label(&self, graph: &GraphDescription) -> String {
        let (node_id, slot) = (&self.key.0, self.key.1 as usize);
        let node = graph.nodes.get(node_id);
        let node_name = node.map_or_else(|| node_id.clone(), node_title);
        let pin_name = node
            .and_then(|n| n.outputs.get(slot))
            .map_or_else(|| slot.to_string(), |pin| pin.pin.name.clone());
        format!("{node_name}.{pin_name}")
    }

    fn pin_id(&self, graph: &GraphDescription) -> Option<String> {
        graph
            .nodes
            .get(&self.key.0)?
            .outputs
            .get(self.key.1 as usize)
            .map(|pin| pin.id.clone())
    }

    /// Records a finished build: a failure is listed (and logged to the compile
    /// history once per distinct message), a success clears the pin's stale error.
    fn record(&mut self, label: String, result: Result<(), String>, cx: &mut Context<Self>) {
        let message = match result {
            Ok(()) => {
                self.errors.borrow_mut().remove(&self.key);
                return;
            }
            Err(message) => message,
        };
        if self
            .errors
            .borrow()
            .get(&self.key)
            .is_some_and(|error| error.message == message)
        {
            return;
        }
        tracing::warn!("Pin preview {label} failed: {message}");
        if let Some(editor) = self.editor.upgrade() {
            let (label, message) = (label.clone(), message.clone());
            editor.update(cx, |editor, cx| {
                editor.push_compilation_history(
                    CompilationState::Error,
                    "preview",
                    format!("Pin preview failed: {label}"),
                    Some(message),
                );
                cx.notify();
            });
        }
        self.errors
            .borrow_mut()
            .insert(self.key.clone(), PinPreviewError { label, message });
    }

    /// Compile and build the preview on a worker (PSGC compile, WGSL validation
    /// and GPU pipeline creation are all slow). The result is installed when it
    /// arrives, unless a newer edit superseded it.
    fn start_build(
        &mut self,
        graph: GraphDescription,
        pin_id: String,
        signature: u64,
        device: wgpu::Device,
        queue: wgpu::Queue,
        cx: &mut Context<Self>,
    ) {
        let label = self.label(&graph);
        let node_id = self.key.0.clone();
        let graph = Arc::new(graph);
        let task = cx.spawn(async move |this, cx| {
            let built = cx
                .background_executor()
                .spawn(async move {
                    let wgsl = compile_preview_wgsl_for_pin(&graph, &node_id, &pin_id)?;
                    let renderer = catch_unwind(AssertUnwindSafe(|| {
                        PinPreviewRenderer::new(&device, &queue, &wgsl)
                    }))
                    .map_err(|_| "The GPU rejected the preview shader (see log)".to_string())?;
                    Ok::<_, String>(renderer)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // A newer edit replaced this build: its result is stale.
                if this.pending.as_ref().map(|p| p.signature) != Some(signature) {
                    return;
                }
                let result = match built {
                    Ok(renderer) => {
                        this.built = Some(Built { signature, renderer });
                        Ok(())
                    }
                    // The graph does not compile into a preview (yet): show none
                    // rather than a stale image.
                    Err(error) => {
                        this.built = None;
                        Err(error)
                    }
                };
                this.record(label, result, cx);
                cx.notify();
            });
        });
        // Replacing an older pending build drops (cancels) it.
        self.pending = Some(PendingBuild {
            signature,
            _task: task,
        });
    }

    /// One frame: keep the surface, rebuild when the graph settled on a new
    /// state, and draw. Returns whether another frame is wanted.
    fn frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self
            .surface
            .as_ref()
            .is_some_and(WgpuSurfaceHandle::is_device_lost)
        {
            self.surface = None;
            self.built = None;
            self.pending = None;
        }
        if self.surface.is_none() {
            if let Some(surface) = window
                .create_wgpu_surface(
                    PIN_PREVIEW_TEXELS,
                    PIN_PREVIEW_TEXELS,
                    PinPreviewRenderer::TEXTURE_FORMAT,
                )
                .filter(|s| !s.is_device_lost())
            {
                self.surface = Some(surface);
                cx.notify();
            }
        }
        let Some(surface) = self.surface.clone() else {
            return false;
        };

        let Some(canvas) = self.canvas.upgrade() else {
            return false;
        };
        let graph = canvas.read(cx).graph();
        let Some(pin_id) = self.pin_id(graph) else {
            return false;
        };
        let signature = graph_preview_signature(graph);
        let settled = self
            .debounce
            .settled(signature, Instant::now(), PIN_PREVIEW_DEBOUNCE);
        if !settled {
            // Edits are still arriving: a build for an earlier state would be
            // thrown away when it lands, so cancel it now.
            if self.pending.as_ref().is_some_and(|p| p.signature != signature) {
                self.pending = None;
            }
        }
        let stale = self.built.as_ref().map_or(true, |b| b.signature != signature);
        // A build for this exact state is running, or already failed: do not
        // start another one every frame.
        let attempted = self.pending.as_ref().is_some_and(|p| p.signature == signature);
        if settled && stale && !attempted {
            let graph = graph.clone();
            self.start_build(
                graph,
                pin_id,
                signature,
                surface.device().clone(),
                surface.queue().clone(),
                cx,
            );
        }

        // Keep drawing the previous image while the new one builds.
        if let Some(built) = &self.built {
            if surface.is_resize_pending() {
                return true;
            }
            if let Some((view, _)) = surface.back_view_with_size() {
                built.renderer.render(
                    surface.device(),
                    surface.queue(),
                    &view,
                    self.started.elapsed().as_secs_f32(),
                );
                drop(view);
                surface.swap_buffers();
            }
        }
        true
    }
}

impl Render for PinPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display: AnyElement = match &self.surface {
            Some(surface) => wgpu_surface(surface.clone())
                .size_full()
                .into_any_element(),
            None => div().size_full().bg(gpui::rgb(0x0d0d0d)).into_any_element(),
        };
        let driver = frame_driver(&cx.entity(), &self.driver)
            .absolute()
            .inset_0();
        div()
            .size_full()
            .flex()
            .justify_end()
            .items_center()
            .pr(px(6.0))
            .py(px(3.0))
            .child(
                div()
                    .relative()
                    .h_full()
                    .aspect_ratio(1.0)
                    .rounded(px(3.0))
                    .overflow_hidden()
                    .child(display)
                    .child(driver),
            )
    }
}

/// Invisible element that draws the preview when GPUI hands it geometry and
/// re-ticks while the preview lives, without rebuilding views.
struct FrameDriver {
    generation: u64,
    current: Rc<Cell<u64>>,
    entity: WeakEntity<PinPreview>,
    pending: Cell<bool>,
}

impl FrameDriver {
    fn tick(self: &Rc<Self>, window: &mut Window, cx: &mut App) {
        if self.current.get() != self.generation || self.pending.get() {
            return;
        }
        let Some(entity) = self.entity.upgrade() else {
            return;
        };
        let again = entity.update(cx, |view, cx| view.frame(window, cx));
        if !again {
            return;
        }
        self.pending.set(true);
        let weak = Rc::downgrade(self);
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(FRAME_INTERVAL).await;
                let Some(driver) = weak.upgrade() else { return };
                driver.pending.set(false);
                let _ = cx.update(|window, cx| {
                    if driver.current.get() == driver.generation
                        && window.was_view_rendered(driver.entity.entity_id())
                    {
                        driver.tick(window, cx);
                        window.refresh_buffers();
                    }
                });
            })
            .detach();
    }
}

fn frame_driver(entity: &Entity<PinPreview>, current: &Rc<Cell<u64>>) -> gpui::Div {
    let generation = current.get().wrapping_add(1);
    current.set(generation);
    let driver = Rc::new(FrameDriver {
        generation,
        current: current.clone(),
        entity: entity.downgrade(),
        pending: Cell::new(false),
    });
    div().on_frame(move |_geometry, window, cx| driver.tick(window, cx))
}

#[cfg(test)]
mod tests {
    use super::{graph_preview_signature, PreviewDebounce, PIN_PREVIEW_DEBOUNCE};
    use graphy::GraphDescription;
    use std::time::{Duration, Instant};

    #[test]
    fn debounce_builds_the_first_state_at_once() {
        let mut debounce = PreviewDebounce::default();
        assert!(debounce.settled(1, Instant::now(), PIN_PREVIEW_DEBOUNCE));
    }

    #[test]
    fn debounce_coalesces_rapid_edits_into_the_final_state() {
        let mut debounce = PreviewDebounce::default();
        let t0 = Instant::now();
        assert!(debounce.settled(1, t0, PIN_PREVIEW_DEBOUNCE));
        let t1 = t0 + Duration::from_millis(10);
        assert!(!debounce.settled(2, t1, PIN_PREVIEW_DEBOUNCE));
        let t2 = t1 + Duration::from_millis(50);
        assert!(!debounce.settled(3, t2, PIN_PREVIEW_DEBOUNCE));
        // Quiet for the full delay: the last state builds.
        assert!(debounce.settled(3, t2 + PIN_PREVIEW_DEBOUNCE, PIN_PREVIEW_DEBOUNCE));
    }

    #[test]
    fn signature_follows_what_a_preview_depends_on() {
        let mut graph = GraphDescription::new("g");
        let mut node = graphy::NodeInstance::new("n", "float_constant", graphy::Position::new(0.0, 0.0));
        node.set_property("value", 1.0);
        graph.add_node(node);
        let before = graph_preview_signature(&graph);
        assert_eq!(before, graph_preview_signature(&graph));
        graph.nodes.get_mut("n").unwrap().set_property("value", 2.0);
        assert_ne!(before, graph_preview_signature(&graph));
        // Moving a node does not change what it previews.
        let after = graph_preview_signature(&graph);
        graph.nodes.get_mut("n").unwrap().position = graphy::Position::new(50.0, 10.0);
        assert_eq!(after, graph_preview_signature(&graph));
    }
}
