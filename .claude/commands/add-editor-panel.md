# Add an Editor Panel

How to add a new dockable panel to the Pulsar editor (e.g. a new tool window, inspector, or viewer).

## Anatomy of a panel

Each editor panel is a GPUI `Entity<T>` that implements:
- `Render` — draws the panel contents
- `EventEmitter<PanelEvent>` — lets the dock system focus/close it
- `Panel` trait from `ui::dock` — provides title, icon, default size

Panels live in `ui-crates/`. Each panel gets its own crate (e.g. `ui_log_viewer`, `ui_file_manager`).

---

## Step 1 — Create the crate

```bash
cd ~/Documents/GitHub/Pulsar-Native/ui-crates
cargo new --lib ui_my_panel
```

Edit `ui-crates/ui_my_panel/Cargo.toml`:
```toml
[package]
name = "ui_my_panel"
version = "0.1.0"
edition = "2021"

[dependencies]
gpui-ce   = { workspace = true }
ui        = { workspace = true }
ui_common = { workspace = true }
engine_state = { workspace = true }
tracing   = { workspace = true }
```

Add to workspace root `Cargo.toml`:
```toml
# in [workspace] members:
"ui-crates/ui_my_panel",

# in [workspace.dependencies] (if other crates need it):
ui_my_panel = { path = "ui-crates/ui_my_panel" }
```

---

## Step 2 — Implement the panel struct

`ui-crates/ui_my_panel/src/lib.rs`:

```rust
use gpui::*;
use ui::{
    dock::{Panel, PanelEvent},
    h_flex, v_flex, ActiveTheme as _,
    IconName,
};

pub struct MyPanel {
    // your state fields
    focus_handle: FocusHandle,
}

impl MyPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for MyPanel {}

impl FocusableView for MyPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MyPanel {
    fn panel_name() -> &'static str { "MyPanel" }
    fn title(&self, _cx: &App) -> AnyElement {
        "My Panel".into_any_element()
    }
    fn icon(&self, _cx: &App) -> Option<IconName> {
        Some(IconName::Puzzle)      // pick any IconName variant
    }
    fn default_width(&self) -> Option<Pixels> { Some(px(280.)) }
    fn default_height(&self) -> Option<Pixels> { None }
}

impl Render for MyPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(/* your content */)
    }
}
```

---

## Step 3 — Register the panel in ui_core

`ui-crates/ui_core/Cargo.toml` — add the dep:
```toml
ui_my_panel = { workspace = true }
```

`ui-crates/ui_core/src/` — find where other panels are registered (look for `ui_log_viewer` or `ui_file_manager` registration patterns) and add:

```rust
use ui_my_panel::MyPanel;

// In the panel registration block:
workspace.add_panel(
    cx.new(|cx| MyPanel::new(window, cx)),
    DockPosition::Right,  // or Left, Bottom
    cx,
);
```

---

## Step 4 — GPUI patterns to know

### Triggering re-render
```rust
cx.notify();   // inside entity methods — marks entity dirty, schedules render
```

### Deferring updates (avoid BorrowMutError)
```rust
cx.defer(move |cx| {
    entity.update(cx, |panel, cx| { panel.field = value; cx.notify(); });
});
```

### Cross-entity references
Use `WeakEntity<T>` to hold non-owning references in closures:
```rust
let weak = cx.entity().downgrade();   // inside Context<Self>
// later:
if let Some(strong) = weak.upgrade() {
    strong.update(cx, |panel, cx| { ... });
}
```

### Accessing window inside cx.defer
```rust
let handle = window.window_handle();
cx.defer(move |cx| {
    let _ = cx.update_window(handle, |_, window, cx| {
        // use window here
    });
});
```

---

## Step 5 — Verify

```bash
cargo check -p ui_my_panel -p ui_core
```

No errors → commit and push Pulsar-Native.
