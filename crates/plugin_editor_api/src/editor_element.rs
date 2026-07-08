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

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyElement, App, Window};
use ui::dock::PanelView;

use crate::error::PluginError;
use crate::identifiers::EditorId;

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
    pub create: Box<dyn Fn(PathBuf, &mut Window, &mut App) -> Result<Arc<dyn PanelView>, PluginError> + Send + Sync>,
}

impl EditorFactory {
    pub fn new(
        editor_id: EditorId,
        create: impl Fn(PathBuf, &mut Window, &mut App) -> Result<Arc<dyn PanelView>, PluginError> + 'static + Send + Sync,
    ) -> Self {
        Self { editor_id, create: Box::new(create) }
    }
}

/// A registry of editor factories, populated by plugins during load.
///
/// The plugin manager collects factories from all loaded plugins and
/// dispatches editor creation requests to the appropriate factory.
pub struct EditorFactoryRegistry {
    factories: Vec<EditorFactory>,
}

impl EditorFactoryRegistry {
    pub fn new() -> Self { Self { factories: Vec::new() } }

    /// Register a single editor factory.
    pub fn register(&mut self, factory: EditorFactory) {
        self.factories.push(factory);
    }

    /// Convenience: register an editor by ID and closure.
    pub fn register_fn(
        &mut self,
        editor_id: EditorId,
        create: impl Fn(PathBuf, &mut Window, &mut App) -> Result<Arc<dyn PanelView>, PluginError> + 'static + Send + Sync,
    ) {
        self.factories.push(EditorFactory::new(editor_id, create));
    }

    /// Look up a factory by editor ID.
    pub fn get(&self, editor_id: &EditorId) -> Option<&EditorFactory> {
        self.factories.iter().find(|f| &f.editor_id == editor_id)
    }

    /// Iterate all registered factories.
    pub fn factories(&self) -> &[EditorFactory] { &self.factories }
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
// Into GPUI AnyElement
// ============================================================================

/// Convert an [`EditorElement`] into a GPUI-compatible `AnyElement` for
/// direct use in the element tree.
///
/// This casts the element via its `AnyElement` representation so it can
/// participate in GPUI's standard lifecycle without plugin code needing
/// to interact with GPUI's element traits directly.
impl<H: EditorHandle> From<EditorElement<H>> for AnyElement {
    fn from(element: EditorElement<H>) -> Self {
        // This conversion works through GPUI's element system:
        // 1. EditorElement implements IntoElement (via derive or manual)
        // 2. IntoElement produces an AnyElement
        // 3. AnyElement participates in request_layout → prepaint → paint
        //
        // At runtime, the EditorElement's render_frame is called each frame
        // via the IntoElement implementation.
        //
        // NOTE: This is a placeholder for the actual GPUI element integration.
        // In practice, EditorElement would implement IntoElement by delegating
        // to the handle's render_frame method each frame.
        todo!("GPUI element integration — EditorElement must implement IntoElement via the ui crate's element system")
    }
}
