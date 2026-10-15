//! # Editor Element Lifecycle
//!
//! Following GPUI's element architecture, this module separates:
//!
//! - **One-time init** (`EditorHandle::init` / `EditorHandle::new`): Creates the editor
//!   state, sets up subscriptions, spawns child entities. Runs once when the editor
//!   panel is created.
//!
//! - **Per-frame render** (`EditorHandle::render_frame`): Builds the ephemeral element
//!   tree each frame. The tree flows through GPUI's standard three-phase lifecycle:
//!   `request_layout` → `prepaint` → `paint`, then is dropped.
//!
//! This prevents the entire editor from being reconstructed every frame — the
//! persistent state lives in the `EditorHandle`, only the element tree is ephemeral.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Element, ElementArenaScope, ElementId, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, WeakEntity, Window,
};
use pulsar_lsp::RustAnalyzerManager;
use ui::dock::PanelView;

use crate::error::PluginError;
use crate::identifiers::EditorId;

/// Typed, host-owned preference values supplied when a plugin editor opens.
/// Engine services the host hands every editor it opens.
///
/// This is the plugin API's context: an editor must not reach for process
/// globals (a plugin library has its own copy of them), it receives what it
/// needs here. Both ways editors are created carry it -- a dynamic plugin's
/// factory gets it through [`EditorSettingsSnapshot::services`], a built-in
/// provider through the editor context.
#[derive(Clone, Default)]
pub struct EditorServices {
    /// The one rust-analyzer the engine runs for the project. Editors attach
    /// to it; they never start, restart or replace it.
    pub language_server: Option<Entity<RustAnalyzerManager>>,
    /// The host's undo histories, one per document (Pulsar-Native#931).
    /// The host makes one and hands the same to every editor.
    pub undo: crate::undo::UndoService,
}

impl EditorServices {
    /// These services without keeping them alive.
    pub fn downgrade(&self) -> WeakEditorServices {
        WeakEditorServices {
            language_server: self.language_server.as_ref().map(Entity::downgrade),
            undo: self.undo.clone(),
        }
    }
}

/// [`EditorServices`] held weakly: what a holder that outlives the app (the
/// process-wide plugin manager) keeps, so the services still end with the app
/// that owns them.
#[derive(Clone, Default)]
pub struct WeakEditorServices {
    language_server: Option<WeakEntity<RustAnalyzerManager>>,
    /// Holds only the registry; each history lives with its views.
    undo: crate::undo::UndoService,
}

impl WeakEditorServices {
    /// The services that are still alive.
    pub fn upgrade(&self) -> EditorServices {
        EditorServices {
            language_server: self.language_server.as_ref().and_then(WeakEntity::upgrade),
            undo: self.undo.clone(),
        }
    }
}

impl std::fmt::Debug for WeakEditorServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.upgrade().fmt(f)
    }
}

impl std::fmt::Debug for EditorServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorServices")
            .field("language_server", &self.language_server.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
pub struct EditorSettingsSnapshot {
    values: HashMap<String, EditorSettingValue>,
    services: EditorServices,
}

#[derive(Clone, Debug)]
pub enum EditorSettingValue {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl EditorSettingsSnapshot {
    pub fn new(values: HashMap<String, EditorSettingValue>) -> Self {
        Self {
            values,
            services: EditorServices::default(),
        }
    }

    /// The same settings with the host's services attached.
    pub fn with_services(mut self, services: EditorServices) -> Self {
        self.services = services;
        self
    }

    /// Engine services for the editor being created.
    pub fn services(&self) -> &EditorServices {
        &self.services
    }

    pub fn boolean(&self, key: &str, fallback: bool) -> bool {
        match self.values.get(key) {
            Some(EditorSettingValue::Boolean(value)) => *value,
            _ => fallback,
        }
    }

    pub fn integer(&self, key: &str, fallback: i64) -> i64 {
        match self.values.get(key) {
            Some(EditorSettingValue::Integer(value)) => *value,
            _ => fallback,
        }
    }

    pub fn float(&self, key: &str, fallback: f64) -> f64 {
        match self.values.get(key) {
            Some(EditorSettingValue::Float(value)) => *value,
            _ => fallback,
        }
    }

    pub fn text(&self, key: &str, fallback: &str) -> String {
        match self.values.get(key) {
            Some(EditorSettingValue::Text(value)) => value.clone(),
            _ => fallback.to_owned(),
        }
    }
}

// ============================================================================
// Editor Factory
// ============================================================================

/// A factory that creates an editor panel instance for a specific editor type.
///
/// Each `EditorFactory` is associated with a single [`EditorId`]. A plugin
/// provides one factory per editor type it supports.
pub struct EditorFactory {
    /// The editor type this factory creates.
    pub editor_id: EditorId,
    /// The creation function.
    pub create: Box<
        dyn Fn(
                PathBuf,
                &EditorSettingsSnapshot,
                &mut Window,
                &mut App,
            ) -> Result<Arc<dyn PanelView>, PluginError>
            + Send
            + Sync,
    >,
}

impl EditorFactory {
    pub fn new(
        editor_id: EditorId,
        create: impl Fn(PathBuf, &mut Window, &mut App) -> Result<Arc<dyn PanelView>, PluginError>
            + 'static
            + Send
            + Sync,
    ) -> Self {
        Self {
            editor_id,
            create: Box::new(move |path, _settings, window, cx| create(path, window, cx)),
        }
    }

    pub fn new_with_settings(
        editor_id: EditorId,
        create: impl Fn(
                PathBuf,
                &EditorSettingsSnapshot,
                &mut Window,
                &mut App,
            ) -> Result<Arc<dyn PanelView>, PluginError>
            + 'static
            + Send
            + Sync,
    ) -> Self {
        Self {
            editor_id,
            create: Box::new(create),
        }
    }
}

/// A registry of editor factories, populated by plugins during load.
///
/// The plugin manager collects factories from all loaded plugins and
/// dispatches editor creation requests to the appropriate factory.
pub struct EditorFactoryRegistry {
    factories: Vec<EditorFactory>,
    /// Panel restorers registered alongside the factories (see
    /// [`crate::panel_restore`]). Empty for a plugin that registers none.
    restorers: crate::panel_restore::PanelRestorerRegistry,
}

impl EditorFactoryRegistry {
    pub fn new() -> Self {
        Self {
            factories: Vec::new(),
            restorers: Default::default(),
        }
    }

    /// Register a restorer that rebuilds panels saved with restore kind `kind`
    /// (see [`crate::panel_restore`]).
    pub fn register_restorer(
        &mut self,
        kind: impl Into<String>,
        restorer: impl Fn(&serde_json::Value, &mut Window, &mut App) -> Option<Arc<dyn PanelView>>
            + Send
            + Sync
            + 'static,
    ) {
        self.restorers.register(kind, restorer);
    }

    /// Take the registered restorers; the host merges them into its global set.
    pub fn take_restorers(&mut self) -> crate::panel_restore::PanelRestorerRegistry {
        std::mem::take(&mut self.restorers)
    }

    /// Register a single editor factory.
    pub fn register(&mut self, factory: EditorFactory) {
        self.factories.push(factory);
    }

    /// Convenience: register an editor by ID and closure.
    pub fn register_fn(
        &mut self,
        editor_id: EditorId,
        create: impl Fn(PathBuf, &mut Window, &mut App) -> Result<Arc<dyn PanelView>, PluginError>
            + 'static
            + Send
            + Sync,
    ) {
        self.factories.push(EditorFactory::new(editor_id, create));
    }

    pub fn register_fn_with_settings(
        &mut self,
        editor_id: EditorId,
        create: impl Fn(
                PathBuf,
                &EditorSettingsSnapshot,
                &mut Window,
                &mut App,
            ) -> Result<Arc<dyn PanelView>, PluginError>
            + 'static
            + Send
            + Sync,
    ) {
        self.factories
            .push(EditorFactory::new_with_settings(editor_id, create));
    }

    /// Look up a factory by editor ID.
    pub fn get(&self, editor_id: &EditorId) -> Option<&EditorFactory> {
        self.factories.iter().find(|f| &f.editor_id == editor_id)
    }

    /// Iterate all registered factories.
    pub fn factories(&self) -> &[EditorFactory] {
        &self.factories
    }
}

// ============================================================================
// Plugin-Level Editor Trait
// ============================================================================

/// Plugin-level trait for providing editor panel instances.
///
/// Editor creation lives alongside the render lifecycle here — both are
/// the same concern (building and running editors), not a separate plugin
/// capability.
///
/// # Relationship Cardinalities
///
/// | Pattern | How | Example |
/// |---------|-----|---------|
/// | **1→1** | One `register_fn` call | A plugin with a single editor |
/// | **1→N** | N `register_fn` calls | A plugin providing graph + properties editors |
/// | **N→1** | Multiple plugins register same `EditorId` | Plugin manager resolves by priority |
/// | **N→N** | N plugins × N editors | General case |
pub trait EditorPluginEditor: crate::plugin::EditorPlugin {
    /// Populate the registry with editor factories for this plugin.
    ///
    /// Call `registry.register_fn(...)` or `registry.register(...)` once
    /// per editor type the plugin provides.
    ///
    /// The `&'static self` receiver is required because factories capture
    /// the plugin reference for later invocation — the plugin must live
    /// for the process lifetime (guaranteed by the plugin system).
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry);
}

// ============================================================================
// Per-Frame Render Context
// ============================================================================

/// Per-frame context passed to [`EditorHandle::render_frame`].
///
/// Provides access to the window and app context for building the
/// element tree each frame. This is the plugin equivalent of GPUI's
/// per-frame rendering context.
pub struct EditorFrameCtx<'a> {
    pub window: &'a mut Window,
    pub cx: &'a mut App,
}

// ============================================================================
// Editor Handle Trait
// ============================================================================

/// A persistent editor handle that separates one-time initialization from
/// per-frame rendering, mirroring GPUI's View/Element lifecycle split.
///
/// # Lifecycle
///
/// 1. [`EditorHandle::init`] — **one-time**: Set up persistent state, spawn
///    child entities, register subscriptions. Called once when the editor panel
///    is first created.
///
/// 2. [`EditorHandle::render_frame`] — **per-frame**: Build the element tree
///    for the current frame. The tree is ephemeral — it goes through GPUI's
///    `request_layout` → `prepaint` → `paint` pipeline, then is dropped.
///    Persistent state lives in the handle across frames.
///
/// 3. [`EditorHandle::teardown`] — **one-time**: Clean up resources when the
///    editor panel is closed.
///
/// # GPUI Correspondence
///
/// | GPUI Concept       | Plugin Equivalent        | When        |
/// |--------------------|--------------------------|-------------|
/// | `View::new()`      | `EditorHandle::init`     | Once        |
/// | `Element::paint()` | `EditorHandle::render_frame` | Every frame |
/// | Drop               | `EditorHandle::teardown` | Once        |
///
pub trait EditorHandle: Send + Sync + 'static {
    /// One-time initialization for this editor.
    ///
    /// Use this to:
    /// - Set up persistent state
    /// - Spawn child entities via `cx.new()`
    /// - Register event subscriptions
    /// - Load initial file content
    fn init(
        &mut self,
        editor_id: &EditorId,
        file_path: &std::path::Path,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), PluginError>;

    /// Build the per-frame element tree.
    ///
    /// Called every frame. Returns an `AnyElement` that GPUI will push through
    /// its standard `request_layout` → `prepaint` → `paint` pipeline.
    ///
    /// The returned element tree is ephemeral — it is dropped after painting.
    /// All persistent state should live on `self` (the `EditorHandle`).
    fn render_frame(&self, ctx: &mut EditorFrameCtx) -> AnyElement;

    /// One-time teardown when the editor panel is closed.
    ///
    /// Use this to flush files, release GPU resources, or clean up
    /// background tasks.
    fn teardown(&mut self, window: &mut Window, cx: &mut App) {}
}

// ============================================================================
// Editor Element Wrapper
// ============================================================================

/// A GPUI element that wraps an [`EditorHandle`], providing the standard
/// element lifecycle for plugin editors.
///
/// This is the primary integration point between plugin editors and GPUI's
/// rendering pipeline. Each frame:
///
/// 1. `EditorElement` calls `handle.render_frame()` to get the element tree
/// 2. The tree flows through GPUI's `request_layout` → `prepaint` → `paint`
/// 3. The tree is dropped; `handle` persists for the next frame
///
/// # Type Parameters
///
/// * `H` — The editor handle type that implements [`EditorHandle`].
///
/// # Usage
///
/// ```rust,ignore
/// use plugin_editor_api::editor_element::*;
///
/// struct MyEditor { /* state */ }
///
/// impl EditorHandle for MyEditor {
///     fn init(&mut self, editor_id: &EditorId, file_path: &Path,
///             window: &mut Window, cx: &mut App) -> Result<(), PluginError> {
///         // One-time setup
///         Ok(())
///     }
///
///     fn render_frame(&self, ctx: &mut EditorFrameCtx) -> AnyElement {
///         // Per-frame element tree
///         div().child("Hello").into_any_element()
///     }
/// }
///
/// // In your EditorPlugin::create_editor:
/// fn create_editor(&self, ...) -> Result<Arc<dyn PanelView>, PluginError> {
///     let editor = EditorElement::new(MyEditor { /* ... */ });
///     // ... register with panel system
/// }
/// ```
pub struct EditorElement<H: EditorHandle> {
    handle: Arc<H>,
}

impl<H: EditorHandle> EditorElement<H> {
    /// Create a new editor element from an [`EditorHandle`].
    ///
    /// The handle's [`EditorHandle::init`] will be called during
    /// [`EditorElement::init`] (one-time), and [`EditorHandle::render_frame`]
    /// will be called every frame during rendering.
    pub fn new(handle: H) -> Self {
        Self {
            handle: Arc::new(handle),
        }
    }

    /// Access the underlying handle.
    pub fn handle(&self) -> &Arc<H> {
        &self.handle
    }
}

impl<H: EditorHandle> Clone for EditorElement<H> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
        }
    }
}

// ============================================================================
// Element Trait Implementation
// ============================================================================

/// Implementation of GPUI's [`Element`] trait for [`EditorElement`].
///
/// Each frame, [`Element::request_layout`] calls [`EditorHandle::render_frame`]
/// to obtain the child element tree, then delegates layout/prepaint/paint to it.
///
/// # DLL boundary: element arena
///
/// `EditorElement<H>` is monomorphized inside the plugin DLL (since `H` is the
/// plugin's own concrete handle type), so everything `render_frame` builds —
/// every `div()`, every `AnyElement::new` — runs through this crate's own
/// statically-linked copy of `gpui`, with its own separate copy of the
/// `CURRENT_ELEMENT_ARENA` thread-local. The host's per-frame
/// `ElementArenaScope::enter` (in `Window::draw`) only ever sets the *host's*
/// copy, so without the `enter` call below, every element a plugin
/// constructs silently falls back to `gpui`'s private, per-DLL,
/// never-reset `ELEMENT_ARENA` — a real, unbounded leak (one freshly grown
/// 1 MiB chunk per frame's worth of elements, forever). Entering the scope
/// here, using the `App` reference already crossing the FFI boundary, points
/// this DLL's thread-local at the *same* arena the host resets every frame
/// instead. See Pulsar-Native issue #261 and the `plugin-leak-demo` repro
/// for the full writeup and before/after measurements.
impl<H: EditorHandle> Element for EditorElement<H> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Fix for #261: point this DLL's element-arena thread-local at the
        // host's real, per-frame-reset arena before building or laying out
        // anything. See this impl's doc comment above for why this is
        // needed at all.
        let _arena_scope = ElementArenaScope::enter(cx.element_arena());

        let mut ctx = EditorFrameCtx { window, cx };
        let mut element = self.handle.render_frame(&mut ctx).into_any_element();
        let layout_id = element.request_layout(ctx.window, ctx.cx);
        (layout_id, element)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        // request_layout's `_arena_scope` guard is dropped by the time
        // prepaint runs — a separate, later call from the framework's
        // traversal. Re-enter here too: if this plugin's element tree
        // allocates anything new during prepaint (e.g. lazy/virtualized
        // content), it needs the host's arena, not this DLL's fallback.
        let _arena_scope = ElementArenaScope::enter(cx.element_arena());
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Same reasoning as `prepaint` above.
        let _arena_scope = ElementArenaScope::enter(cx.element_arena());
        element.paint(window, cx);
    }
}

// ============================================================================
// IntoElement Trait Implementation
// ============================================================================

impl<H: EditorHandle> IntoElement for EditorElement<H> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

// ============================================================================
// Into GPUI AnyElement
// ============================================================================

impl<H: EditorHandle> From<EditorElement<H>> for AnyElement {
    fn from(element: EditorElement<H>) -> Self {
        element.into_any_element()
    }
}
