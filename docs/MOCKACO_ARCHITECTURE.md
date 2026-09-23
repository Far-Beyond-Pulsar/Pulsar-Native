# Mockaco Architecture

Mockaco is a standalone, high-performance code editor for GPUI/WGPUI. It may
use WGPUI-Component for general UI primitives, but it owns the complete text
editing pipeline.

## Goals

- Make editing latency independent of document size.
- Keep rendering work proportional to the visible viewport and changed ranges.
- Support large files, multi-cursor editing, folding, wrapping, diagnostics,
  syntax highlighting, LSP features, search, diffing, and embedded UI.
- Keep the editor core usable without Pulsar, its plugin ABI, or its filesystem.
- Make integrations replaceable: parsers, LSP clients, storage, themes, and
  rendering policy must not be hard-coded into the document model.

## Non-goals

Mockaco is not a replacement for WGPUI or WGPUI-Component. It should not own
generic buttons, menus, dialogs, popovers, theme infrastructure, icons, or
ordinary single-line form inputs. Those remain dependencies supplied by the
host UI toolkit.

## Dependency direction

```text
                    host application
                           |
                    mockaco-workspace
                           |
                      mockaco-gpui
                    /       |        \
           mockaco-lsp  mockaco-language  WGPUI-Component
                    \       |        /        |
                     mockaco-core           WGPUI
```

Mockaco may depend on WGPUI-Component. WGPUI-Component must never depend on
Mockaco. Text-editing behavior must not leak back into shared component APIs.

## Repository and crate layout

```text
mockaco/
├── Cargo.toml
├── crates/
│   ├── mockaco-core/
│   ├── mockaco-language/
│   ├── mockaco-lsp/
│   ├── mockaco-renderer/
│   ├── mockaco-gpui/
│   └── mockaco-workspace/
├── examples/
│   ├── editor/
│   ├── large-file/
│   └── standalone/
├── benchmarks/
├── fixtures/
└── docs/
```

### `mockaco-core`

The framework-independent editing engine.

- `Document`: immutable/snapshot-friendly text storage.
- `Transaction`: insert, delete, replace, and annotation operations.
- `SelectionSet`: one or more carets and selections.
- `PositionMap`: byte, UTF-16, line/column, and display coordinates.
- `UndoHistory`: grouped, branching-safe undo/redo.
- `DocumentSnapshot`: cheap read-only view used by background workers.
- `EditorState`: selections, viewport intent, mode, and pending composition.

The core should not import GPUI, Tree-sitter, LSP types, filesystem APIs, or
Pulsar types.

### `mockaco-language`

Language services backed by replaceable providers.

- language identification and registry
- incremental parser/highlighter
- syntax themes and semantic token mapping
- folding ranges
- bracket and indentation providers
- injected languages
- syntax-context queries used by auto-close and indentation

Tree-sitter is an initial provider, not part of the core API contract.

### `mockaco-lsp`

Asynchronous language-server integration.

- request scheduling and cancellation
- diagnostics
- completions and inline completions
- hover
- definitions/references
- code actions
- semantic tokens
- document synchronization

LSP results enter the editor through versioned snapshots and are discarded when
their document version is stale.

### `mockaco-renderer`

Renderer-neutral layout and display mapping.

- visible line calculation
- wrapping and folding display map
- line-number/gutter layout
- decoration interval projection
- glyph run preparation
- minimap density data

This crate owns the expensive layout algorithms but not GPUI elements.

### `mockaco-gpui`

The WGPUI presentation and input adapter.

- GPUI element implementation
- keyboard, mouse, touch, and IME handling
- caret and selection painting
- gutter, diagnostics, fold controls, and minimap
- scroll synchronization
- completion, hover, and code-action surfaces
- integration with WGPUI-Component buttons, menus, popovers, themes, and icons

### `mockaco-workspace`

The optional application-level editor shell.

- editor tabs and split groups
- file explorer
- file loading/saving
- dirty state and external file changes
- diff editor
- command routing
- workspace persistence

Pulsar-specific plugin registration, `PanelView`, engine services, and Rust
Analyzer ownership stay in a thin Pulsar adapter outside Mockaco.

## State architecture

The editor is divided into independently invalidated state domains:

```text
DocumentState       text, version, transactions, undo history
ViewState           scroll, viewport, wrapping, folding
SelectionState      carets, selections, rectangular mode
DecorationState     syntax, diagnostics, search, diff, semantic tokens
InteractionState    focus, IME, mouse drag, completion/hover overlays
IntegrationState    LSP requests, filesystem state, save state
```

Document mutations produce a transaction and document version. Derived state
is updated from the transaction rather than recomputed from the entire file.
Rendering consumes a coherent snapshot and never performs filesystem, parser,
or LSP work synchronously.

## Rendering pipeline

```text
transaction
  -> document version + edit map
  -> incremental syntax/LSP/decorations
  -> display map update
  -> visible-line layout
  -> glyph runs and decorations
  -> GPUI paint
```

Only visible lines should create render elements or glyph layouts. Long-line
handling must be bounded separately so one pathological line cannot stall the
whole viewport. Background results are published through version checks and
range-based invalidation.

## Performance rules

1. No full-document scans on keystroke paths.
2. No allocation per painted character.
3. No parser or LSP request on the UI thread.
4. Cache by document version, display-map revision, and viewport revision.
5. Invalidate changed ranges, not whole documents.
6. Keep immutable snapshots cheap and background work cancellable.
7. Measure first-paint, keypress-to-frame, scroll, paste, undo, and large-file
   open separately.
8. Ship benchmarks for 1K, 100K, 1M, and pathological-line documents.

## API seams

The public API should be capability-oriented rather than exposing internal
state structs:

```rust
let editor = Editor::builder()
    .language(LanguageId::new("rust"))
    .theme(theme)
    .build();

editor.dispatch(EditCommand::InsertText(text));
editor.set_language_provider(provider);
editor.set_lsp_client(client);
```

Host integrations receive typed events such as `DocumentChanged`,
`SelectionChanged`, `SaveRequested`, `NavigateRequested`, and
`CompletionAccepted`. They should not reach into renderer internals.

## Migration inventory

### Migrate as behavior references

- `plugins/vendor/code_editor/src/script_editor/text_editor.rs`
- `plugins/vendor/code_editor/src/script_editor/file_explorer.rs`
- `plugins/vendor/code_editor/src/script_editor/workspace_panels.rs`
- `plugins/vendor/code_editor/src/script_editor/languages.rs`
- `plugins/vendor/code_editor/src/script_editor/autocomplete_integration.rs`
- the plugin's locales and standalone harness

### Reuse selectively

- WGPUI-Component's Tree-sitter language queries and theme schema
- WGPUI-Component's minimap and diagnostic visual treatment
- `wgpui-base`'s editor-core concepts: display maps, folding, decorations,
  parser-independent highlighting, and LSP seams

### Keep outside Mockaco

- generic WGPUI-Component inputs used by settings, search, forms, and tools
- Pulsar plugin registration and built-in editor provider code
- Pulsar filesystem, engine backend, Rust Analyzer process management, and
  engine-specific commands
- generic WGPUI-Component theme and component infrastructure

The currently exported `ui::CodeEditor` wrapper is not a migration anchor; it
has no identified production consumers.

## Compatibility strategy

The first Pulsar integration should be an adapter that preserves the existing
script-editor provider ID and file associations. Mockaco should initially expose
compatibility methods for loading, saving, setting content, setting language,
and receiving editor events. Internal implementation should remain new.

During transition, the old editor and Mockaco can coexist behind a feature
flag. The old WGPUI-Component editor path is removed only after the adapter,
standalone harness, and Pulsar integration pass functional and performance
tests.

## Delivery phases

### Phase 0 — contract and measurements

- Freeze the existing dirty work before copying code.
- Add behavior fixtures for editing, selection, IME, undo, search, folding,
  diagnostics, diff, and save behavior.
- Record baseline latency and memory numbers for the current editor.

### Phase 1 — core editor

- Implement `mockaco-core` document, transactions, selections, coordinates,
  undo, and snapshot tests.
- Add property tests for UTF-8/UTF-16 conversion and transaction mapping.

### Phase 2 — display and language services

- Implement display mapping, wrapping, folding, decorations, and incremental
  Tree-sitter highlighting.
- Add the language registry and theme adapter.

### Phase 3 — GPUI editor surface

- Implement viewport rendering, input, IME, caret, selection, gutter, and
  scrolling in `mockaco-gpui`.
- Integrate WGPUI-Component primitives only at the presentation boundary.

### Phase 4 — LSP and workspace

- Add cancellable LSP requests, overlays, tabs, file explorer, save lifecycle,
  diff mode, and workspace commands.

### Phase 5 — Pulsar integration and cutover

- Replace the existing script-editor plugin internals with the Mockaco adapter.
- Run both implementations behind a feature flag.
- Compare fixture results and benchmarks.
- Remove only the obsolete editor-specific implementation.

## Definition of done

Mockaco is ready to replace the current editor when it has a standalone harness,
document and rendering benchmarks, deterministic editor-core tests, IME coverage,
large-file coverage, LSP cancellation/version tests, and a Pulsar adapter that
does not expose Mockaco internals or require Pulsar types in the core crates.
