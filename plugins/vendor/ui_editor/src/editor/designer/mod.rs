//! The designer: the canvas the widget is drawn and edited on.
//!
//! The widget is laid out and turned into a display list (see
//! [`crate::editor::frame`]), painted with GPUI, and edited by direct
//! manipulation: click to select, drag to move, handles to resize, the
//! wheel to zoom. Everything a gesture does goes through the shared commands,
//! and a whole drag is a single undo step.

mod input;
pub mod overlay;
mod toolbar;

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use plugin_editor_api::AssetPayload;
use ui::menu::context_menu::ContextMenuExt;
use ui::{ActiveTheme, IconName};

use crate::display::DisplayOptions;
use crate::model::{NodeId, Rect, Vec2, WidgetAsset};

use super::canvas::geometry::Guide;
use super::canvas::metrics::WindowMetrics;
use super::commands::Ctx;
use super::common::panel::dock_panel;
use super::frame::{Frame, FrameKey};
use super::icons::{LibraryDrag, NodeDrag, PaletteDrag};
use overlay::{Colors, DropPreview, OverlayData, ViewShot};

/// What a mouse drag on the canvas is doing.
pub(crate) enum Gesture {
    Pan { start_mouse: Vec2, start_pan: Vec2 },
    Marquee { start: Vec2, current: Vec2, additive: bool, base: Vec<NodeId> },
    Move(MoveGesture),
    Resize(ResizeGesture),
    /// Dragging a widget that a box arranges, to a new place in the tree.
    Reorder { ids: Vec<NodeId>, start_mouse: Vec2, active: bool },
}

pub(crate) struct MoveGesture {
    pub before: WidgetAsset,
    pub start_mouse: Vec2,
    pub items: Vec<MoveItem>,
    pub targets: Vec<Rect>,
    pub bounds: Rect,
    /// Canvas pixels per layout pixel under the parent.
    pub k: f32,
    pub moved: bool,
    /// The widget under the pointer was already selected with others: a
    /// click without a drag narrows the selection to it.
    pub narrow_to: Option<NodeId>,
}

pub(crate) struct MoveItem {
    pub id: NodeId,
    pub rect: Rect,
    pub parent: Rect,
}

pub(crate) struct ResizeGesture {
    pub before: WidgetAsset,
    pub id: NodeId,
    pub handle: super::canvas::geometry::Handle,
    pub start_mouse: Vec2,
    pub start: Rect,
    pub parent: Rect,
    pub targets: Vec<Rect>,
    pub k: f32,
}

pub struct DesignerPanel {
    pub(crate) ctx: Ctx,
    pub(crate) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) frame: Option<Arc<Frame>>,
    pub(crate) hover: Option<NodeId>,
    pub(crate) gesture: Option<Gesture>,
    pub(crate) drop_preview: Option<DropPreview>,
    pub(crate) guides: Vec<Guide>,
    /// The pointer, in canvas pixels.
    pub(crate) mouse: Option<Vec2>,
    pub(crate) space_down: bool,
    pub(crate) project_root: PathBuf,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(DesignerPanel, "widget-designer", "Designer");

impl DesignerPanel {
    pub fn new(ctx: Ctx, project_root: PathBuf, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sel, |_, _, cx| cx.notify()),
            cx.observe(&ctx.view, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
        ];
        Self {
            ctx,
            bounds: Rc::new(Cell::new(Bounds::default())),
            frame: None,
            hover: None,
            gesture: None,
            drop_preview: None,
            guides: Vec::new(),
            mouse: None,
            space_down: false,
            project_root,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The canvas's size in pixels (zero before the first paint).
    pub(crate) fn canvas_size(&self) -> Vec2 {
        let b = self.bounds.get();
        Vec2::new(f32::from(b.size.width), f32::from(b.size.height))
    }

    /// A window position as canvas pixels.
    pub(crate) fn to_canvas_px(&self, position: Point<Pixels>) -> Vec2 {
        let b = self.bounds.get();
        Vec2::new(f32::from(position.x - b.origin.x), f32::from(position.y - b.origin.y))
    }

    /// The frame for the widget as it is now, rebuilt only when something it depends on changed.
    fn ensure_frame(&mut self, window: &Window, cx: &mut Context<Self>) -> Arc<Frame> {
        let doc = self.ctx.doc.read(cx);
        let view = self.ctx.view.read(cx);
        let size = view.layout_size(doc.asset());
        let options = DisplayOptions { state: view.state, placeholders: view.placeholders, time: 0.0 };
        let key = FrameKey::new(doc.revision(), self.ctx.lib.read(cx).revision(), size, &options);
        if let Some(frame) = &self.frame {
            if frame.key == key {
                return frame.clone();
            }
        }
        let library = self.ctx.lib.read(cx).library();
        let metrics = WindowMetrics { window, family: cx.theme().font_family.clone() };
        let frame = Arc::new(Frame::build(doc.asset(), &*library, key, size, &options, &metrics));
        self.frame = Some(frame.clone());
        frame
    }

    /// Fit the screen to the canvas while the user hasn't taken over the view.
    fn auto_fit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let canvas = self.canvas_size();
        if canvas.x < 50.0 || canvas.y < 50.0 {
            return;
        }
        let view = self.ctx.view.read(cx);
        if !view.auto_fit {
            return;
        }
        let screen = view.screen_size(self.ctx.doc.read(cx).asset());
        let mut fitted = view.clone();
        fitted.fit(screen, canvas, 48.0);
        if (fitted.zoom - view.zoom).abs() > 1e-4 || (fitted.pan.x - view.pan.x).abs() > 0.5 || (fitted.pan.y - view.pan.y).abs() > 0.5 {
            let view_entity = self.ctx.view.clone();
            window.defer(cx, move |_, cx| {
                view_entity.update(cx, |v, cx| {
                    if v.auto_fit {
                        v.zoom = fitted.zoom;
                        v.pan = fitted.pan;
                        cx.notify();
                    }
                });
            });
        }
    }

    fn overlay_data(&self, frame: Arc<Frame>, cx: &App) -> OverlayData {
        let theme = cx.theme();
        let doc = self.ctx.doc.read(cx);
        let widget = doc.asset();
        let view = self.ctx.view.read(cx);
        let sel = self.ctx.sel.read(cx);
        let primary = sel.primary();
        let marquee = match &self.gesture {
            Some(Gesture::Marquee { start, current, .. }) => Some(Rect::from_corners(*start, *current)),
            _ => None,
        };
        let parent_of_primary = primary.and_then(|p| widget.parent_of(p));
        let primary_is_canvas = primary.is_some_and(|p| widget.slot_kind_of(p) == crate::model::SlotKind::Canvas);
        let anchors = primary.filter(|_| primary_is_canvas).and_then(|p| {
            let node = widget.node(p)?;
            let v = node.slot_view(crate::model::SlotKind::Canvas);
            let parent = parent_of_primary.and_then(|pp| frame.screen.get(&pp)).copied()?;
            Some((v.vec2("anchor_min"), v.vec2("anchor_max"), parent))
        });
        let mut locked = HashSet::new();
        widget.root.walk(&mut |n| {
            if n.editor.locked {
                locked.insert(n.id);
            }
        });
        let screen = Hsla { h: theme.background.h, s: theme.background.s, l: (theme.background.l + 0.03).min(1.0), a: 1.0 };
        OverlayData {
            parent_rect: parent_of_primary.and_then(|p| frame.screen.get(&p)).copied().filter(|_| primary.is_some_and(|p| p != widget.root.id)),
            frame,
            view: ViewShot {
                scale: view.scale(),
                pan: view.pan,
                grid: view.show_grid,
                grid_size: view.grid_size,
                outlines: view.outlines,
                safe_zone: view.safe_zone,
            },
            options: DisplayOptions { state: view.state, placeholders: view.placeholders, time: 0.0 },
            selection: sel.ids().to_vec(),
            primary,
            primary_name: primary.and_then(|p| widget.node(p)).map(|n| n.name.clone()).unwrap_or_default(),
            hover: self.hover,
            locked,
            resizable: primary_is_canvas && primary.is_some_and(|p| widget.node(p).is_some_and(|n| !n.editor.locked)),
            anchors,
            guides: self.guides.clone(),
            marquee,
            drop: self.drop_preview.clone(),
            colors: Colors {
                background: Hsla { h: theme.background.h, s: theme.background.s, l: theme.background.l * 0.72, a: 1.0 },
                screen,
                accent: theme.info,
                muted: theme.muted_foreground,
                text: theme.foreground,
                warning: theme.warning,
                danger: theme.danger,
                popover: theme.popover,
            },
            font_family: theme.font_family.clone(),
            project_root: self.project_root.clone(),
        }
    }
}

impl Render for DesignerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        self.auto_fit(window, cx);
        let frame = self.ensure_frame(window, cx);
        *self.ctx.frame.borrow_mut() = Some(frame.clone());
        let data = self.overlay_data(frame, cx);
        let bounds_cell = self.bounds.clone();

        let ctx_menu = self.ctx.clone();
        let toolbar = self.render_toolbar(window, cx);
        let status = self.render_status(cx);

        div()
            .id("widget-designer")
            .key_context("WidgetDesigner")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(theme.background)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .capture_any_mouse_down(cx.listener(Self::capture_mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_drag_move::<PaletteDrag>(cx.listener(Self::drag_over_palette))
            .on_drag_move::<LibraryDrag>(cx.listener(Self::drag_over_library))
            .on_drag_move::<NodeDrag>(cx.listener(Self::drag_over_nodes))
            .on_drag_move::<AssetPayload>(cx.listener(Self::drag_over_asset))
            .on_drop(cx.listener(Self::drop_palette))
            .on_drop(cx.listener(Self::drop_library))
            .on_drop(cx.listener(Self::drop_nodes))
            .on_drop(cx.listener(Self::drop_asset))
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                        bounds
                    },
                    move |bounds, _, window, cx| overlay::paint_designer(bounds, &data, window, cx),
                )
                .absolute()
                .size_full(),
            )
            .child(toolbar)
            .child(status)
            .context_menu(move |menu, window, cx| super::menus::node_menu(menu, &ctx_menu, window, cx))
    }
}

/// An icon for the empty designer, used when the file can't be shown.
#[allow(dead_code)]
pub(crate) fn empty_icon() -> IconName {
    IconName::Frame
}
