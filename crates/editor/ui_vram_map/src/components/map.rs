//! The VRAM map: the placed boxes painted on a canvas that pans (drag) and
//! zooms (wheel, around the pointer; double-click a box to frame its group,
//! Fit to see everything).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use engine_backend::services::vram::{self, VramSnapshot};
use gpui::prelude::FluentBuilder;
use gpui::*;
use parking_lot::RwLock;
use ui::{h_flex, v_flex, ActiveTheme};

use crate::utils::format::{bytes, percent};
use crate::utils::layout::Rect;
use crate::utils::model::{self, Arrangement, Node, Placed, Tint};

/// How often the snapshot is taken while the window is open.
const REFRESH: Duration = Duration::from_secs(1);
const MAX_ZOOM: f32 = 4000.0;

/// What the paint pass and the mouse handlers share: the tree, where its
/// boxes are and how the canvas looks at them.
#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) tree: Option<Arc<Node>>,
    /// Laid out for this canvas size; relaid when it changes.
    pub(crate) size: (f32, f32),
    pub(crate) placed: Vec<Placed>,
    pub(crate) zoom: f32,
    /// The layout point at the canvas's top-left corner.
    pub(crate) pan: (f32, f32),
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) hover: Option<usize>,
}

impl Scene {
    fn to_layout(&self, at: Point<Pixels>) -> (f32, f32) {
        let x = f32::from(at.x - self.bounds.origin.x) / self.zoom + self.pan.0;
        let y = f32::from(at.y - self.bounds.origin.y) / self.zoom + self.pan.1;
        (x, y)
    }

    fn to_screen(&self, rect: Rect) -> Bounds<Pixels> {
        let x = (rect.x - self.pan.0) * self.zoom;
        let y = (rect.y - self.pan.1) * self.zoom;
        Bounds::new(
            point(self.bounds.origin.x + px(x), self.bounds.origin.y + px(y)),
            size(px(rect.w * self.zoom), px(rect.h * self.zoom)),
        )
    }

    /// Keep some of the map on screen.
    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, MAX_ZOOM);
        let (w, h) = self.size;
        self.pan.0 = self.pan.0.clamp(0.0, (w - w / self.zoom).max(0.0));
        self.pan.1 = self.pan.1.clamp(0.0, (h - h / self.zoom).max(0.0));
    }

    fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
    }

    /// Frame `rect` on the canvas.
    fn frame(&mut self, rect: Rect) {
        let (w, h) = self.size;
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        self.zoom = (w / rect.w).min(h / rect.h);
        self.pan = (rect.x, rect.y);
        self.clamp();
    }

    fn relayout(&mut self, w: f32, h: f32) {
        self.size = (w, h);
        self.placed = match &self.tree {
            Some(tree) => model::place(tree, Rect::new(0.0, 0.0, w, h)),
            None => Vec::new(),
        };
        self.hover = None;
        self.clamp();
    }
}

pub struct VramMapView {
    pub(crate) arrangement: Arrangement,
    /// Ask the owners how much of their memory is live (the claims). Off, only
    /// the allocator reports are read.
    pub(crate) track_usage: Arc<AtomicBool>,
    pub(crate) snapshot: Arc<RwLock<Option<Arc<VramSnapshot>>>>,
    /// The snapshot the tree was built from.
    pub(crate) built_from: Option<(usize, Arrangement)>,
    pub(crate) scene: Rc<RefCell<Scene>>,
    pub(crate) drag: Option<(Point<Pixels>, bool)>,
    pub(crate) _refresh: Task<()>,
}

impl VramMapView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let snapshot = Arc::new(RwLock::new(None));
        let track_usage = Arc::new(AtomicBool::new(true));
        let refresh = {
            let snapshot = snapshot.clone();
            let track_usage = track_usage.clone();
            cx.spawn(async move |this, cx| loop {
                let usage = track_usage.load(Ordering::Relaxed);
                let taken = cx
                    .background_executor()
                    .spawn(async move { vram::snapshot(usage) })
                    .await;
                *snapshot.write() = Some(Arc::new(taken));
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            })
        };
        Self {
            arrangement: Arrangement::ByOwner,
            track_usage,
            snapshot,
            built_from: None,
            scene: Rc::new(RefCell::new(Scene {
                zoom: 1.0,
                ..Default::default()
            })),
            drag: None,
            _refresh: refresh,
        }
    }

    /// Rebuild the tree when the snapshot or the arrangement changed.
    fn sync_tree(&mut self) -> Option<Arc<VramSnapshot>> {
        let snapshot = self.snapshot.read().clone()?;
        let key = (Arc::as_ptr(&snapshot) as usize, self.arrangement);
        if self.built_from != Some(key) {
            self.built_from = Some(key);
            let tree = Arc::new(model::build(&snapshot, self.arrangement));
            let mut scene = self.scene.borrow_mut();
            scene.tree = Some(tree);
            let (w, h) = scene.size;
            scene.relayout(w, h);
        }
        Some(snapshot)
    }

    fn set_arrangement(&mut self, arrangement: Arrangement, cx: &mut Context<Self>) {
        if self.arrangement != arrangement {
            self.arrangement = arrangement;
            self.scene.borrow_mut().fit();
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = f32::from(event.delta.pixel_delta(px(20.0)).y);
        let mut scene = self.scene.borrow_mut();
        let anchor = scene.to_layout(event.position);
        scene.zoom = (scene.zoom * 1.0025f32.powf(delta)).clamp(1.0, MAX_ZOOM);
        let local = (
            f32::from(event.position.x - scene.bounds.origin.x),
            f32::from(event.position.y - scene.bounds.origin.y),
        );
        scene.pan = (anchor.0 - local.0 / scene.zoom, anchor.1 - local.1 / scene.zoom);
        scene.clamp();
        cx.notify();
    }

    fn on_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.click_count >= 2 {
            let mut scene = self.scene.borrow_mut();
            let (x, y) = scene.to_layout(event.position);
            // Frame the group the box belongs to (the box itself if a group).
            let target = model::hit(&scene.placed, x, y).map(|i| {
                if scene.placed[i].leaf {
                    scene.placed[i].parent.unwrap_or(i)
                } else {
                    i
                }
            });
            match target {
                Some(i) => {
                    let rect = scene.placed[i].rect;
                    scene.frame(rect);
                }
                None => scene.fit(),
            }
            self.drag = None;
            cx.notify();
            return;
        }
        self.drag = Some((event.position, false));
    }

    fn on_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mut scene = self.scene.borrow_mut();
        if let (Some((last, moved)), Some(MouseButton::Left)) = (self.drag.as_mut(), event.pressed_button) {
            let dx = f32::from(event.position.x - last.x);
            let dy = f32::from(event.position.y - last.y);
            if dx.abs() + dy.abs() > 0.0 {
                *moved = true;
                scene.pan.0 -= dx / scene.zoom;
                scene.pan.1 -= dy / scene.zoom;
                scene.clamp();
                *last = event.position;
                cx.notify();
            }
            return;
        }
        let (x, y) = scene.to_layout(event.position);
        let hover = scene.bounds.contains(&event.position).then(|| model::hit(&scene.placed, x, y)).flatten();
        if hover != scene.hover {
            scene.hover = hover;
            cx.notify();
        }
    }

    fn on_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
    }

    fn chip(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id(id)
            .px_2()
            .py(px(3.0))
            .rounded(px(4.0))
            .text_xs()
            .cursor_pointer()
            .bg(if active {
                theme.accent.opacity(0.25)
            } else {
                theme.muted.opacity(0.2)
            })
            .text_color(if active {
                theme.accent_foreground
            } else {
                theme.muted_foreground
            })
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
    }
}

/// A box's base colour: the owner path's first part picks the hue (SceneDB,
/// Helio, the renderer host each their own), the rest shifts it a little so
/// neighbouring owners part; unscoped memory is a muted grey-blue, free space
/// grey.
pub(crate) fn tint_color(tint: &Tint) -> Hsla {
    let hash = |text: &str| {
        text.bytes()
            .fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
    };
    match tint {
        Tint::Owner(owner) => {
            let (top, rest) = owner.split_once('/').unwrap_or((owner, ""));
            if top == vram::UNSCOPED {
                return hsla(0.6, 0.12, 0.42, 1.0);
            }
            let shift = if rest.is_empty() {
                0.0
            } else {
                (hash(rest) % 1000) as f32 / 1000.0 * 0.08 - 0.04
            };
            let hue = ((hash(top) % 360) as f32 / 360.0 + shift).rem_euclid(1.0);
            hsla(hue, 0.55, 0.45, 1.0)
        }
        Tint::Free => hsla(0.0, 0.0, 0.22, 1.0),
        Tint::Frame => hsla(0.0, 0.0, 0.16, 1.0),
    }
}

impl Render for VramMapView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.sync_tree();
        let theme = cx.theme().clone();
        let tracking = self.track_usage.load(Ordering::Relaxed);

        let summary = match &snapshot {
            None => "Reading the allocator reports…".to_string(),
            Some(s) if s.devices.is_empty() => {
                format!("No device reports its allocations ({} without an allocator report)", s.unsupported)
            }
            Some(s) => {
                let claimed: u64 = s.claims.iter().map(|c| c.capacity).sum();
                let live: u64 = s.claims.iter().filter_map(|c| c.used).sum();
                let mut text = format!(
                    "{} allocated of {} reserved in {} driver blocks",
                    bytes(s.allocated()),
                    bytes(s.reserved()),
                    s.devices.iter().map(|d| d.blocks.len()).sum::<usize>(),
                );
                // What no owner scope covers: the places still to give one.
                let uses = s.name_use();
                let unscoped: u64 = s
                    .devices
                    .iter()
                    .flat_map(|d| &d.blocks)
                    .flat_map(|b| &b.allocations)
                    .filter(|a| VramSnapshot::owner_of(a, &uses) == vram::UNSCOPED)
                    .map(|a| a.size)
                    .sum();
                text.push_str(&format!(" · unscoped {}", bytes(unscoped)));
                if tracking && claimed > 0 {
                    text.push_str(&format!(
                        " · owners report {} live of {} claimed ({})",
                        bytes(live),
                        bytes(claimed),
                        percent(live as f64 / claimed as f64)
                    ));
                }
                text
            }
        };

        let toolbar = h_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .flex_wrap()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(self.chip("by-owner", "By owner", self.arrangement == Arrangement::ByOwner, cx, |this, cx| {
                this.set_arrangement(Arrangement::ByOwner, cx)
            }))
            .child(self.chip("by-block", "By block", self.arrangement == Arrangement::ByBlock, cx, |this, cx| {
                this.set_arrangement(Arrangement::ByBlock, cx)
            }))
            .child(self.chip("track-usage", if tracking { "Usage tracking: on" } else { "Usage tracking: off" }, tracking, cx, |this, cx| {
                this.track_usage.fetch_xor(true, Ordering::Relaxed);
                cx.notify();
            }))
            .child(self.chip("fit", "Fit", false, cx, |this, cx| {
                this.scene.borrow_mut().fit();
                cx.notify();
            }))
            .child(div().text_xs().text_color(theme.muted_foreground).child(summary));

        let scene = self.scene.clone();
        let paint_scene = self.scene.clone();
        let tooltip = self.render_tooltip(cx);
        let map = div()
            .id("vram-map")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut scene = scene.borrow_mut();
                        scene.bounds = bounds;
                        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        if (w, h) != scene.size && w > 1.0 && h > 1.0 {
                            scene.relayout(w, h);
                        }
                    },
                    move |bounds, _, window, cx| paint(&paint_scene.borrow(), bounds, window, cx),
                )
                .size_full(),
            )
            .when_some(tooltip, |this, tooltip| this.child(tooltip));

        v_flex().size_full().child(toolbar).child(map)
    }
}

impl VramMapView {
    fn render_tooltip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scene = self.scene.borrow();
        let index = scene.hover?;
        let hovered = scene.placed.get(index)?;
        let theme = cx.theme();
        let mut path = vec![hovered.label.clone()];
        let mut at = hovered.parent;
        while let Some(i) = at {
            path.push(scene.placed[i].label.clone());
            at = scene.placed[i].parent;
        }
        path.reverse();
        let used = match hovered.used {
            Some(fraction) => format!(
                "{} used ({} live)",
                percent(fraction),
                bytes((fraction * hovered.bytes as f64) as u64)
            ),
            None if matches!(hovered.tint, Tint::Free) => "free".into(),
            None => "use not reported by its owner".into(),
        };
        let count = if hovered.count > 1 {
            format!(" · {} allocations", hovered.count)
        } else {
            String::new()
        };
        Some(
            div()
                .absolute()
                .bottom_2()
                .left_2()
                .px_3()
                .py_2()
                .rounded(px(6.0))
                .bg(theme.popover.opacity(0.95))
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.popover_foreground)
                .child(div().font_weight(FontWeight::SEMIBOLD).child(path.join(" › ")))
                .child(format!("{} allocated · {used}{count}", bytes(hovered.bytes)))
                .into_any_element(),
        )
    }
}

/// Paint the visible boxes: each box's colour, its used part darker from the
/// bottom up, frames between groups, labels where they fit.
fn paint(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let theme = cx.theme().clone();
    window.paint_quad(fill(bounds, theme.background));
    let text_style = window.text_style();
    let font_size = px(11.0);
    let line_height = px(14.0);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (index, placed) in scene.placed.iter().enumerate() {
            let rect = scene.to_screen(placed.rect);
            if !rect.intersects(&bounds) || rect.size.width < px(0.75) || rect.size.height < px(0.75) {
                continue;
            }
            let base = tint_color(&placed.tint);
            if placed.leaf {
                // Unused bytes lighter and washed out; used bytes the full colour.
                let unused = Hsla { s: base.s * 0.45, l: (base.l + 0.18).min(0.85), ..base };
                match placed.used {
                    Some(fraction) if !matches!(placed.tint, Tint::Free) => {
                        window.paint_quad(fill(rect, unused));
                        let used_h = rect.size.height * fraction.clamp(0.0, 1.0) as f32;
                        let used_rect = Bounds::new(
                            point(rect.origin.x, rect.origin.y + rect.size.height - used_h),
                            size(rect.size.width, used_h),
                        );
                        window.paint_quad(fill(used_rect, base));
                    }
                    _ => window.paint_quad(fill(rect, base)),
                }
            } else {
                window.paint_quad(fill(rect, Hsla { l: base.l * 0.6 + 0.04, ..base }));
            }
            let border = if scene.hover == Some(index) {
                theme.accent
            } else {
                hsla(0.0, 0.0, 0.0, if placed.leaf { 0.45 } else { 0.7 })
            };
            if rect.size.width > px(3.0) && rect.size.height > px(3.0) {
                window.paint_quad(outline(rect, border, BorderStyle::Solid));
            }

            // Labels: a group's in its header, a leaf's at its top-left.
            let header_h = px(model::HEADER * scene.zoom);
            let fits = rect.size.width > px(48.0)
                && if placed.leaf {
                    rect.size.height > px(30.0)
                } else {
                    placed.header && header_h > line_height
                };
            if !fits {
                continue;
            }
            let mut text = placed.label.clone();
            text.push_str("  ");
            text.push_str(&bytes(placed.bytes));
            if let Some(fraction) = placed.used.filter(|_| !matches!(placed.tint, Tint::Free)) {
                text.push_str(&format!(" · {}", percent(fraction)));
            }
            let run = TextRun {
                len: text.len(),
                font: text_style.font(),
                color: hsla(0.0, 0.0, 0.97, 0.95).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
                letter_spacing: None,
            };
            let shaped = window.text_system().shape_line(text.into(), font_size, &[run], None);
            let origin = point(rect.origin.x + px(4.0), rect.origin.y + px(2.0));
            let clip = Bounds::new(
                rect.origin,
                size(rect.size.width - px(2.0), if placed.leaf { rect.size.height } else { header_h }),
            );
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                let _ = shaped.paint(origin, line_height, window, cx);
            });
        }
    });
}
