//! State the editor's panels share: what is selected, how the designer is
//! looking at the widget, and the project's widget library.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;

use crate::display::PreviewState;
use crate::library::Library;
use crate::model::{NodeId, Vec2, WidgetAsset};

use super::common::document::{Document, SaveBus};

pub type Doc = Entity<Document<WidgetAsset>>;
pub type Sel = Entity<Selection>;
pub type ViewState = Entity<View>;
pub type Lib = Entity<LibraryModel>;

// ── Selection ──────────────────────────────────────────────────────────────

/// The selected nodes, in the order they were picked. The last is the primary
/// one: the details panel shows it, and alignment is relative to it.
#[derive(Default)]
pub struct Selection {
    ids: Vec<NodeId>,
}

impl Selection {
    pub fn ids(&self) -> &[NodeId] {
        &self.ids
    }

    pub fn primary(&self) -> Option<NodeId> {
        self.ids.last().copied()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.ids.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Select exactly `ids`.
    pub fn set(&mut self, ids: Vec<NodeId>, cx: &mut Context<Self>) {
        if self.ids != ids {
            self.ids = ids;
            cx.notify();
        }
    }

    pub fn select(&mut self, id: NodeId, cx: &mut Context<Self>) {
        self.set(vec![id], cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set(Vec::new(), cx);
    }

    /// Add or remove `id` from the selection; an added node becomes primary.
    pub fn toggle(&mut self, id: NodeId, cx: &mut Context<Self>) {
        if let Some(at) = self.ids.iter().position(|&i| i == id) {
            self.ids.remove(at);
        } else {
            self.ids.push(id);
        }
        cx.notify();
    }

    /// Add `id` (making it primary) without removing anything.
    pub fn add(&mut self, id: NodeId, cx: &mut Context<Self>) {
        self.ids.retain(|&i| i != id);
        self.ids.push(id);
        cx.notify();
    }

    /// Forget nodes that no longer exist (after a delete or undo).
    pub fn retain_existing(&mut self, widget: &WidgetAsset, cx: &mut Context<Self>) {
        let before = self.ids.len();
        self.ids.retain(|&id| widget.node(id).is_some());
        if self.ids.len() != before {
            cx.notify();
        }
    }
}

// ── View ───────────────────────────────────────────────────────────────────

/// A screen size to preview the widget at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPreset {
    pub name: &'static str,
    pub size: Vec2,
}

pub const SCREEN_PRESETS: [ScreenPreset; 9] = [
    ScreenPreset { name: "1920 \u{d7} 1080 (Full HD)", size: Vec2::new(1920.0, 1080.0) },
    ScreenPreset { name: "2560 \u{d7} 1440 (QHD)", size: Vec2::new(2560.0, 1440.0) },
    ScreenPreset { name: "3840 \u{d7} 2160 (4K)", size: Vec2::new(3840.0, 2160.0) },
    ScreenPreset { name: "1280 \u{d7} 720 (HD)", size: Vec2::new(1280.0, 720.0) },
    ScreenPreset { name: "1366 \u{d7} 768 (Laptop)", size: Vec2::new(1366.0, 768.0) },
    ScreenPreset { name: "1024 \u{d7} 768 (4:3)", size: Vec2::new(1024.0, 768.0) },
    ScreenPreset { name: "2436 \u{d7} 1125 (Phone, landscape)", size: Vec2::new(2436.0, 1125.0) },
    ScreenPreset { name: "1125 \u{d7} 2436 (Phone, portrait)", size: Vec2::new(1125.0, 2436.0) },
    ScreenPreset { name: "800 \u{d7} 600", size: Vec2::new(800.0, 600.0) },
];

/// The size the widget is previewed at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Screen {
    /// The size the widget says it is designed for.
    Design,
    Preset(usize),
    Custom(Vec2),
}

/// How the designer is looking at the widget.
#[derive(Clone)]
pub struct View {
    /// Canvas pixels per widget pixel.
    pub zoom: f32,
    /// Where the widget's corner is, in canvas pixels from the canvas corner.
    pub pan: Vec2,
    /// Keep fitting the widget to the canvas until the user zooms or pans.
    pub auto_fit: bool,
    pub screen: Screen,
    /// Preview scale, as a game's UI scaling would apply: the layout sees the
    /// screen size divided by this.
    pub dpi: f32,
    pub show_grid: bool,
    pub snap: bool,
    pub grid_size: f32,
    /// Outline every widget, not just the selected one.
    pub outlines: bool,
    pub safe_zone: bool,
    pub placeholders: bool,
    pub state: PreviewState,
    /// A short message about the last command that couldn't do what was asked.
    pub notice: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::new(40.0, 40.0),
            auto_fit: true,
            screen: Screen::Design,
            dpi: 1.0,
            show_grid: true,
            snap: true,
            grid_size: 8.0,
            outlines: false,
            safe_zone: false,
            placeholders: true,
            state: PreviewState::Normal,
            notice: None,
        }
    }
}

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 8.0;

impl View {
    /// The screen size in pixels.
    pub fn screen_size(&self, widget: &WidgetAsset) -> Vec2 {
        match self.screen {
            Screen::Design => widget.design_size,
            Screen::Preset(i) => SCREEN_PRESETS.get(i).map_or(widget.design_size, |p| p.size),
            Screen::Custom(size) => size,
        }
    }

    /// The size layout runs at: the screen size under the preview scale.
    pub fn layout_size(&self, widget: &WidgetAsset) -> Vec2 {
        let s = self.screen_size(widget);
        let d = self.dpi.max(0.25);
        Vec2::new(s.x / d, s.y / d)
    }

    pub fn screen_label(&self, widget: &WidgetAsset) -> String {
        let s = self.screen_size(widget);
        let name = match self.screen {
            Screen::Design => "Design size".to_string(),
            Screen::Preset(i) => SCREEN_PRESETS.get(i).map_or("Screen".to_string(), |p| p.name.to_string()),
            Screen::Custom(_) => "Custom".to_string(),
        };
        if matches!(self.screen, Screen::Preset(_)) {
            name
        } else {
            format!("{name} ({} \u{d7} {})", s.x.round(), s.y.round())
        }
    }

    /// Canvas pixels per layout pixel: the zoom times the preview scale.
    pub fn scale(&self) -> f32 {
        self.zoom * self.dpi.max(0.25)
    }

    pub fn set_zoom_about(&mut self, zoom: f32, anchor: Vec2) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        // Keep the widget point under `anchor` where it is.
        let widget_point = self.to_widget(anchor);
        self.zoom = zoom;
        let scale = self.scale();
        self.pan = Vec2::new(anchor.x - widget_point.x * scale, anchor.y - widget_point.y * scale);
        self.auto_fit = false;
    }

    /// Fit `content` (in widget pixels) into a canvas of `canvas` pixels with `margin` around.
    pub fn fit(&mut self, content: Vec2, canvas: Vec2, margin: f32) {
        if content.x <= 0.0 || content.y <= 0.0 || canvas.x <= margin * 2.0 || canvas.y <= margin * 2.0 {
            return;
        }
        let zoom = ((canvas.x - margin * 2.0) / content.x).min((canvas.y - margin * 2.0) / content.y).clamp(MIN_ZOOM, MAX_ZOOM);
        self.zoom = zoom;
        self.pan = Vec2::new((canvas.x - content.x * zoom) * 0.5, (canvas.y - content.y * zoom) * 0.5);
    }

    /// Canvas pixel to layout space.
    pub fn to_widget(&self, canvas_point: Vec2) -> Vec2 {
        let scale = self.scale();
        Vec2::new((canvas_point.x - self.pan.x) / scale, (canvas_point.y - self.pan.y) / scale)
    }

    pub fn to_canvas(&self, widget_point: Vec2) -> Vec2 {
        let scale = self.scale();
        Vec2::new(widget_point.x * scale + self.pan.x, widget_point.y * scale + self.pan.y)
    }
}

// ── Library ────────────────────────────────────────────────────────────────

/// The project's widgets, kept current: scanned when the editor opens and
/// again whenever any widget is saved or the user asks.
pub struct LibraryModel {
    root: PathBuf,
    library: Arc<Library>,
    scanning: bool,
    /// Counts scans that changed the library, so caches can tell.
    revision: u64,
    _task: Option<Task<()>>,
    _subscription: Subscription,
}

impl LibraryModel {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let bus = SaveBus::get(cx);
        let subscription = cx.observe(&bus, |this: &mut Self, _, cx| this.refresh(cx));
        let mut model = Self {
            library: Arc::new(Library::empty(root.clone())),
            root,
            scanning: false,
            revision: 0,
            _task: None,
            _subscription: subscription,
        };
        model.refresh(cx);
        model
    }

    pub fn library(&self) -> Arc<Library> {
        self.library.clone()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Scan the project again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let scan = cx.background_executor().spawn(async move { Library::scan(&root) });
        self.scanning = true;
        self._task = Some(cx.spawn(async move |this, cx| {
            let library = scan.await;
            this.update(cx, |this, cx| {
                this.scanning = false;
                this.library = Arc::new(library);
                this.revision += 1;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would pull in gpui's own `test` macro.
    use super::{Screen, View, MAX_ZOOM};
    use crate::model::{Vec2, WidgetAsset};

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_fixed() {
        let mut view = View { zoom: 1.0, pan: Vec2::new(100.0, 50.0), ..Default::default() };
        let anchor = Vec2::new(400.0, 300.0);
        let before = view.to_widget(anchor);
        view.set_zoom_about(2.5, anchor);
        let after = view.to_widget(anchor);
        assert!((before.x - after.x).abs() < 1e-3 && (before.y - after.y).abs() < 1e-3);
        assert!(!view.auto_fit);
        view.set_zoom_about(1000.0, anchor);
        assert_eq!(view.zoom, MAX_ZOOM);
    }

    #[test]
    fn fitting_centres_the_widget_with_a_margin() {
        let mut view = View::default();
        view.fit(Vec2::new(1920.0, 1080.0), Vec2::new(1000.0, 800.0), 20.0);
        assert!((view.zoom - 960.0 / 1920.0).abs() < 1e-4);
        let top_left = view.to_canvas(Vec2::ZERO);
        let bottom_right = view.to_canvas(Vec2::new(1920.0, 1080.0));
        assert!((top_left.x - 20.0).abs() < 1e-3);
        assert!((1000.0 - bottom_right.x - 20.0).abs() < 1e-3, "centred horizontally");
        assert!((top_left.y - (800.0 - bottom_right.y)).abs() < 1e-3, "centred vertically");
    }

    #[test]
    fn the_preview_size_follows_the_screen_choice_and_dpi() {
        let widget = WidgetAsset::new();
        let mut view = View::default();
        assert_eq!(view.screen_size(&widget), widget.design_size);
        view.screen = Screen::Preset(3);
        assert_eq!(view.screen_size(&widget), Vec2::new(1280.0, 720.0));
        view.dpi = 2.0;
        assert_eq!(view.layout_size(&widget), Vec2::new(640.0, 360.0));
        view.screen = Screen::Custom(Vec2::new(500.0, 400.0));
        assert!(view.screen_label(&widget).contains("500"));
    }
}
