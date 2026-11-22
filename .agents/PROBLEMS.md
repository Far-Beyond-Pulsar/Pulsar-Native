# Problems panel and findings

The Problems panel (`crates/editor/ui_problems`, the bottom drawer and its
pop-out window) lists code diagnostics and **findings**: actionable facts about
performance, the project and the editor. Model and store:
`crates/core/pulsar_findings`.

## What the panel shows

- **Tabs:** Code, Performance, Project, Editor. Each tab shows a count coloured
  by its worst item, ignoring dismissed findings.
- **One line per item, grouped:** code is grouped by file and findings by their
  `subject`, worst group first. Groups collapse; searching opens every group.
  Only the open row shows more: its explanation, its location, its actions and
  Dismiss. An open code row also shows the file around the problem and its
  quick fixes.
- **Header, one line:** the tabs, a severity filter, an active source filter
  (from the language server footer), search, and **Dismissed** (shown only when
  something is dismissed).
- **Dismiss:** a finding the project accepts as a trade-off can be dismissed.
  Dismissals are kept in `<project>/.pulsar/dismissed_findings.txt`, one item id
  per line, and the Dismissed view restores them. Code diagnostics are fixed,
  not dismissed.
- **Footer:** the error and warning counts cover every tab.

Keep the panel quiet. A finding is something a person can act on, so it says
what to do (in `detail` and `actions`). A condition that comes and goes from
frame to frame is debounced by its producer and never listed as it flickers.

## Publishing findings

```rust
pulsar_findings::publish("My source", vec![
    Finding::new("stable-key", Category::Performance, Severity::Warning, "One-line title")
        .detail("Why it matters and what to change.")
        .subject("Group heading")
        .metric("2.1 ms")
        .action("Open Streaming settings", Action::OpenSettings {
            window: SettingsWindow::Project,
            page: Some("Streaming".into()),
        }),
]);
```

- A source publishes its **complete** set every time. Publishing an empty set
  removes the source, and an unchanged republish is free.
- Use one source name per instance (`"Label#instance"`). The panel identifies
  items by `label|key`, so two instances' identical findings are listed once,
  and a dismissal survives a restart.
- `key` must be stable: derive it from what the finding is about, never from a
  count. The renderer strips digits from its warning titles to build keys.
- `Action::OpenFile` opens through the panel's navigation event.
  `OpenSettings` and `OpenWindow` (by registered window name) are carried out
  by `ui_core` (`construction.rs`, `run_finding_action`).

### Who publishes today

| Source | Where | What |
|---|---|---|
| Renderer | `engine_backend/src/services/renderer_findings.rs`, from `GpuRenderer::publish_details` | Every `WARNING:` line a render pass reports in `RenderPass::diagnostics` (Performance), debounced. The GPU frame staying over its frame budget, naming the most expensive pass (Performance). A pipeline that does not build, and a rejected shader edit (Project) |
| Streaming | `ui_core/src/app/findings/performance.rs` | Textures drawn below full detail to fit the texture VRAM budget. An asset cache that keeps rebuilding what it evicted |
| Plugins | `ui_core/src/app/findings/project.rs` | The project's plugin resolution problems and the plugin catalog's problems |
| Editor tasks | `ui_core/src/app/findings/editor.rs` | Stalled tasks, recent failed tasks, UI freezes and slow UI work |
| A plugin's own check | `pulsar_findings::FindingSource { origin, id, label, collect }` from its root `build` | Whatever the plugin knows. The editor runs `collect` on a worker about once a second while the plugin is enabled |

A render pass that wants to report something only needs a `WARNING:` line in
its `diagnostics()`. Each line of a pass's text counts, so a pass can report
statistics and warnings together. A plugin's non-render check is a
`FindingSource`.
