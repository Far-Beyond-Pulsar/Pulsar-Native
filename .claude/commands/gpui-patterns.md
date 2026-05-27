# GPUI Patterns for Pulsar Engine

Quick reference for GPUI-ce (the Pulsar fork of GPUI) patterns that come up constantly in this codebase.

---

## Rendering

### Layout primitives
```rust
div()          // block container
h_flex()       // horizontal flexbox (ui::h_flex helper)
v_flex()       // vertical flexbox   (ui::v_flex helper)
```

### Sizing shorthands
```rust
.w_full()  .h_full()  .size_full()
.w(px(240.))  .h(px(32.))
.w(relative(0.5))   // 50% of parent
.min_w(px(100.))  .max_w(px(400.))
```

### Flexbox
```rust
.flex_1()          // flex: 1 (fills remaining space)
.flex_col()        // column direction
.flex_row()        // row direction (default in h_flex)
.gap(px(8.))       // gap between children
.items_center()    // align-items: center
.justify_between() // justify-content: space-between
```

### Positioning (absolute layout)
```rust
.relative()        // position: relative (enables absolute children)
.absolute()        // position: absolute
.top(px(0.))  .right(px(0.))  .bottom(px(0.))  .left(px(0.))
.inset_0()         // top/right/bottom/left = 0
```

### Visibility
```rust
.invisible()              // visibility: hidden (layout preserved)
.group_hover("", |el| el.visible())   // show on parent group hover
.when(condition, |el| el.opacity(0.3))
.when_some(option, |el, val| el.child(render(val)))
```

### Children
```rust
.child(element)                    // single child
.children(iter)                    // iterator of IntoElement
.children(vec.into_iter().map(|x| render(x)))
```

### Text
```rust
.text_sm()  .text_base()  .text_lg()
.font_semibold()  .font_bold()
.text_color(cx.theme().foreground)
.line_height(relative(1.5))
.truncate()   // overflow: hidden; text-overflow: ellipsis
```

### Borders and radius
```rust
.border_1()  .border_2()
.border_color(cx.theme().border)
.border_b_1()   // only bottom border
.rounded(cx.theme().radius)
.rounded_lg()
.overflow_hidden()
```

### Backgrounds and shadows
```rust
.bg(cx.theme().background)
.bg(cx.theme().elevated_surface)
.shadow_md()  .shadow_lg()  .shadow_none()
```

### Padding / margin
```rust
.p_4()    // padding: 16px (scale: 1 = 4px)
.px_3()   // padding horizontal: 12px
.py_2()   // padding vertical: 8px
.pt_3p5() // padding-top: 14px (3.5 × 4)
.m_2()    .mx_auto()
```

---

## Theme Colors

```rust
cx.theme().background        // window background
cx.theme().elevated_surface  // panel/card background
cx.theme().surface           // slightly elevated surface
cx.theme().foreground        // primary text color ← use this for UI elements that match text
cx.theme().muted_foreground  // secondary/hint text
cx.theme().border            // border, dividers
cx.theme().accent            // interactive accent (blue/teal)
cx.theme().success           // green (confirmed, complete)
cx.theme().warning           // yellow/amber
cx.theme().danger            // red/error
cx.theme().info              // blue informational
cx.theme().popover           // notification/popup background
cx.theme().radius            // default corner radius
cx.theme().radius_lg         // large corner radius
```

---

## Interactivity

### Hover state
```rust
.on_hover(cx.listener(|this, hovered: &bool, window, cx| {
    this.is_hovered = *hovered;
    cx.notify();
}))
```

### Click
```rust
.on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
    // handle click
}))
// or without entity access:
.on_click(move |_event, window, cx| { ... })
```

### cx.listener — the key borrow rule
`cx.listener` grants `&mut Self`. **Never call `entity.update(cx, ...)` on the same entity inside a listener** — it causes `BorrowMutError` via a non-unwindable panic at the ObjC event boundary.

**Always defer cross-entity updates:**
```rust
cx.listener(|this, _, window, cx| {
    let val = this.some_field.clone();
    let other = this.other_entity.clone();
    cx.defer(move |cx| {
        other.update(cx, |panel, cx| {
            panel.do_thing(val, cx);
        });
    });
})
```

**If you need `&mut Window` inside defer:**
```rust
let handle = window.window_handle();
cx.defer(move |cx| {
    let _ = cx.update_window(handle, |_, window, cx| {
        // window available here
    });
});
```

---

## Entity Lifecycle

### Creating
```rust
let entity = cx.new(|cx| MyStruct::new(cx));       // in App/Context
let entity = window.new(|cx| MyStruct::new(cx));    // in Window
```

### Updating
```rust
entity.update(cx, |this, cx| {
    this.field = value;
    cx.notify();  // schedule re-render
});
```

### Reading
```rust
let val = entity.read(cx).some_field.clone();
```

### Weak references (for closures)
```rust
let weak = entity.downgrade();       // or cx.entity().downgrade()
// later:
if let Some(strong) = weak.upgrade() {
    strong.update(cx, |_, cx| cx.notify());
}
```

---

## Async Spawning

### Inside `Context<T>` (entity method):
```rust
cx.spawn(async move |this, cx| {
    // this: WeakEntity<T>, cx: AsyncApp
    Timer::after(Duration::from_secs(1)).await;
    this.update(cx, |panel, cx| { panel.done = true; cx.notify(); }).ok();
})
.detach();
```

### Inside `App` (`on_click` closure, `cx: &mut App`):
```rust
cx.spawn(async move |async_app: &mut AsyncApp| {
    let result = do_async_work().await;
    let _ = async_app.update_window(window_handle, |_, window, cx| {
        window.push_notification(Notification::success("Done"), cx);
    });
})
.detach();
```

### Timer inside async (use GPUI's executor, not smol::Timer):
```rust
async_app.background_executor().timer(Duration::from_millis(250)).await;
```

---

## Notifications

```rust
// Simple
window.push_notification(Notification::info("Message text"), cx);
window.push_notification(Notification::success("Done."), cx);
window.push_notification(Notification::warning("Caution."), cx);
window.push_notification(Notification::error("Failed."), cx);

// With title + progress bar
window.push_notification(
    Notification::info("Building project… (42%)")
        .title("Build Core")
        .id::<MyNotificationTag>()   // same-ID push replaces existing
        .progress(0.42)              // 0.0 = empty bar+pulse, 1.0 = full green + auto-dismiss
        .autohide(false),            // keep visible until replaced
    cx,
);

// Auto-dismiss delay (default 5s; used when progress(1.0) or autohide(true))
.autohide_delay(Duration::from_secs(3))
```

**Same ID replaces:** push a notification with `.id::<Tag>()` to replace an existing one with the same tag in-place.

---

## Animations

```rust
element.with_animation(
    "animation-id",
    Animation::new(Duration::from_secs_f32(1.2))
        .repeat()
        .with_easing(cubic_bezier(0.4, 0., 0.6, 1.)),
    |el, delta| {   // delta: 0.0 → 1.0
        el.opacity(delta)
    },
)
```

`cubic_bezier` is in `ui::animation`. Common easings:
- `cubic_bezier(0.4, 0., 0.2, 1.)` — standard material ease
- `cubic_bezier(0.4, 0., 0.6, 1.)` — ease-in-out
