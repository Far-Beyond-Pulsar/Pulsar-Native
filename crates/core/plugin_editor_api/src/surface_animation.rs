//! Drive GPU surfaces without invalidating their owning views.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{div, App, Context, Div, ElementGeometry, Entity, WeakEntity, Window};

/// Owned by a view so replacing its element cancels timers for older GPU inputs.
#[derive(Default)]
pub struct SurfaceAnimation {
    generation: Rc<Cell<u64>>,
}

/// An invisible driver for an externally rendered surface. The callback renders
/// and swaps GPU buffers, returning whether another tick is needed. It must not
/// paint GPUI primitives or notify views for animation alone.
///
/// The element supplies fresh geometry on real layout changes. Timers render
/// directly and request only compositing, even when WGPUI skips its tree walk.
/// Replacing the driver invalidates older callbacks; timers hold weak references.
pub fn surface_animation<V: 'static>(
    entity: &Entity<V>,
    state: &mut SurfaceAnimation,
    interval: Duration,
    render: impl Fn(&mut V, ElementGeometry, &mut Window, &mut Context<V>) -> bool + 'static,
) -> Div {
    let generation = state.generation.get().wrapping_add(1);
    state.generation.set(generation);
    let animation = Rc::new(SurfaceDriver {
        generation,
        current_generation: state.generation.clone(),
        entity: entity.downgrade(),
        interval,
        render: Box::new(render),
        geometry: Cell::new(None),
        pending: Cell::new(false),
    });
    div().on_frame(move |geometry, window, cx| {
        animation.geometry.set(Some(geometry));
        animation.tick(window, cx);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{prelude::*, px, size, AnyView, Render, StyleRefinement, TestAppContext};
    use std::cell::RefCell;

    struct Leaf {
        animation: SurfaceAnimation,
        renders: Rc<Cell<usize>>,
        samples: Rc<RefCell<Vec<(usize, f32)>>>,
        revision: usize,
        animate: bool,
    }

    impl Render for Leaf {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let revision = self.revision;
            surface_animation(
                &cx.entity(),
                &mut self.animation,
                Duration::from_millis(33),
                move |this, geometry, _, _| {
                    this.samples
                        .borrow_mut()
                        .push((revision, geometry.bounds.size.width.as_f32()));
                    this.animate
                },
            )
            .size_full()
        }
    }

    struct Root {
        leaf: Entity<Leaf>,
        renders: Rc<Cell<usize>>,
        visible: bool,
        width: f32,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().size_full().when(self.visible, |div| {
                div.child(
                    AnyView::from(self.leaf.clone())
                        .cached(StyleRefinement::default().w(px(self.width)).h(px(100.))),
                )
            })
        }
    }

    fn setup(cx: &mut TestAppContext) -> (gpui::WindowHandle<Root>, Entity<Leaf>) {
        let leaf = cx.new(|_| Leaf {
            animation: Default::default(),
            renders: Default::default(),
            samples: Default::default(),
            revision: 0,
            animate: true,
        });
        let window = cx.open_window(size(px(400.), px(300.)), {
            let leaf = leaf.clone();
            move |_, _| Root {
                leaf,
                renders: Default::default(),
                visible: true,
                width: 200.,
            }
        });
        cx.run_until_parked();
        (window, leaf)
    }

    fn tick(cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(33));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn surface_ticks_do_not_rebuild_views_and_stop_when_hidden(cx: &mut TestAppContext) {
        let (window, leaf) = setup(cx);
        let samples = leaf.read_with(cx, |leaf, _| leaf.samples.clone());
        let leaf_renders = leaf.read_with(cx, |leaf, _| leaf.renders.get());
        let root_renders = window.read_with(cx, |root, _| root.renders.get()).unwrap();
        let initial = samples.borrow().len();
        for _ in 0..4 {
            tick(cx);
        }
        assert_eq!(samples.borrow().len(), initial + 4);
        assert_eq!(
            leaf.read_with(cx, |leaf, _| leaf.renders.get()),
            leaf_renders
        );
        assert_eq!(
            window.read_with(cx, |root, _| root.renders.get()).unwrap(),
            root_renders
        );

        window
            .update(cx, |root, _, cx| {
                root.visible = false;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let hidden = samples.borrow().len();
        for _ in 0..3 {
            tick(cx);
        }
        assert_eq!(samples.borrow().len(), hidden);
        window
            .update(cx, |root, _, cx| {
                root.visible = true;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let shown = samples.borrow().len();
        tick(cx);
        assert_eq!(samples.borrow().len(), shown + 1);
    }

    #[gpui::test]
    fn edits_replace_pending_inputs_and_resize_updates_geometry(cx: &mut TestAppContext) {
        let (window, leaf) = setup(cx);
        let samples = leaf.read_with(cx, |leaf, _| leaf.samples.clone());
        leaf.update(cx, |leaf, cx| {
            leaf.revision = 1;
            cx.notify();
        });
        cx.run_until_parked();
        samples.borrow_mut().clear();
        for _ in 0..3 {
            tick(cx);
        }
        assert_eq!(&*samples.borrow(), &[(1, 200.); 3]);

        window
            .update(cx, |root, _, cx| {
                root.width = 250.;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        tick(cx);
        assert_eq!(samples.borrow().last(), Some(&(1, 250.)));

        leaf.update(cx, |leaf, cx| {
            leaf.animate = false;
            cx.notify();
        });
        cx.run_until_parked();
        let stopped = samples.borrow().len();
        for _ in 0..3 {
            tick(cx);
        }
        assert_eq!(samples.borrow().len(), stopped);
    }
}

type RenderSurface<V> = dyn Fn(&mut V, ElementGeometry, &mut Window, &mut Context<V>) -> bool;

struct SurfaceDriver<V: 'static> {
    generation: u64,
    current_generation: Rc<Cell<u64>>,
    entity: WeakEntity<V>,
    interval: Duration,
    render: Box<RenderSurface<V>>,
    geometry: Cell<Option<ElementGeometry>>,
    pending: Cell<bool>,
}

impl<V: 'static> SurfaceDriver<V> {
    fn tick(self: &Rc<Self>, window: &mut Window, cx: &mut App) {
        if self.current_generation.get() != self.generation || self.pending.get() {
            return;
        }
        let Some(entity) = self.entity.upgrade() else {
            return;
        };
        let Some(geometry) = self.geometry.get() else {
            return;
        };
        if geometry
            .bounds
            .intersect(&geometry.content_mask.bounds)
            .is_empty()
        {
            return;
        }
        let animate = entity.update(cx, |view, cx| (self.render)(view, geometry, window, cx));
        if !animate {
            return;
        }
        self.pending.set(true);
        let weak = Rc::downgrade(self);
        let interval = self.interval;
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(interval).await;
                let Some(animation) = weak.upgrade() else {
                    return;
                };
                animation.pending.set(false);
                // A removed tab may still have an old callback in a retained frame.
                // Check the active scene so it cannot keep rendering in the background.
                let _ = cx.update(|window, cx| {
                    if animation.current_generation.get() == animation.generation
                        && window.was_view_rendered(animation.entity.entity_id())
                    {
                        animation.tick(window, cx);
                        window.refresh_buffers();
                    }
                });
            })
            .detach();
    }
}
