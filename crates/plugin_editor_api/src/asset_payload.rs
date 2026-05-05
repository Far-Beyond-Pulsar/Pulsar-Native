use gpui::{prelude::*, *};

// Re-export so consumers only need one import.
pub use ui_types_common::{AssetKind, AssetPayload};

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
