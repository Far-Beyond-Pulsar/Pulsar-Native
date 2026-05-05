use gpui::{prelude::*, *};
use std::path::Path;
use ui_types_common::AssetKind;

// Re-export so consumers only need one import.
pub use ui_types_common::AssetKind;

/// The canonical drag payload for any asset dragged out of the file manager
/// drawer or any other engine panel.
///
/// # Usage — emitting a drag
///
/// ```rust,ignore
/// .on_drag(AssetPayload::from_path(&item.path), |drag, _, _, cx| {
///     cx.new(|_| drag.clone())
/// })
/// ```
///
/// # Usage — receiving a drop (via [`AssetDropArea`])
///
/// ```rust,ignore
/// AssetDropArea::new("level-viewport")
///     .accepts(vec![AssetKind::Mesh, AssetKind::Scene])
///     .on_asset_drop(cx.listener(|this, payload, window, cx| {
///         this.spawn_asset(payload, window, cx);
///     }))
///     .child(viewport_content)
/// ```
#[derive(Clone, Debug)]
pub struct AssetPayload {
    /// Engine-FS relative path using forward-slash separators.
    /// May be an absolute OS path when the file has not yet been imported.
    pub engine_path: String,

    /// File name (without parent directory), used in the drag ghost and toasts.
    pub name: String,

    /// Semantic kind, derived from the file extension.
    pub kind: AssetKind,

    /// Lowercase extension without the leading dot (e.g. `"fbx"`).
    pub extension: String,
}

impl AssetPayload {
    /// Build a payload from any filesystem path. Kind is inferred from the
    /// extension. For best results, pass the engine-FS relative path.
    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = AssetKind::from_extension(&extension);
        // Normalise to forward slashes for engine-FS compatibility.
        let engine_path = path.to_string_lossy().replace('\\', "/");
        Self { engine_path, name, kind, extension }
    }
}

/// GPUI requires the drag type to implement `Render` so GPUI can render the
/// drag ghost while the payload is in flight.
impl Render for AssetPayload {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::ActiveTheme as _;
        let theme = cx.theme();

        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .bg(theme.primary)
            .text_color(theme.primary_foreground)
            .text_sm()
            .shadow_lg()
            .child(format!("{} · {}", self.kind.display_label(), self.name))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AssetDropArea — convenience wrapper around DropArea<AssetPayload>
// ─────────────────────────────────────────────────────────────────────────────

/// A drop target that accepts [`AssetPayload`] drags, with kind-based
/// accept/reject filtering and automatic visual feedback.
///
/// This is a thin specialisation of [`ui::drop_area::DropArea`] that adds the
/// [`AssetDropAreaExt::accepts`] builder so callers can filter by
/// [`AssetKind`] without writing the predicate manually.
///
/// # Example
///
/// ```rust,ignore
/// AssetDropArea::new("level-viewport")
///     .accepts(vec![AssetKind::Mesh, AssetKind::Scene])
///     .on_asset_drop(cx.listener(|this, payload, window, cx| {
///         this.import_and_spawn(payload, window, cx);
///     }))
///     .child(my_viewport)
/// ```
pub type AssetDropArea = ui::drop_area::DropArea<AssetPayload>;

/// Extension methods on [`AssetDropArea`] for ergonomic kind-based filtering.
pub trait AssetDropAreaExt: Sized {
    /// Accept only payloads whose [`AssetKind`] is in `kinds`.
    ///
    /// Calling this replaces any previous `can_accept` predicate.
    fn accepts(self, kinds: impl IntoIterator<Item = AssetKind>) -> Self;

    /// Register a drop handler. Equivalent to [`DropArea::on_drop`] but named
    /// to avoid ambiguity when both are in scope.
    fn on_asset_drop(
        self,
        f: impl Fn(&AssetPayload, &mut Window, &mut App) + 'static,
    ) -> Self;
}

impl AssetDropAreaExt for AssetDropArea {
    fn accepts(self, kinds: impl IntoIterator<Item = AssetKind>) -> Self {
        let kinds: Vec<AssetKind> = kinds.into_iter().collect();
        self.can_accept(move |payload: &AssetPayload| kinds.contains(&payload.kind))
    }

    fn on_asset_drop(
        self,
        f: impl Fn(&AssetPayload, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drop(f)
    }
}
