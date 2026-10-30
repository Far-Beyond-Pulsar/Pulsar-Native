# Code editor AI integration

AI inside the script editor (`plugins/vendor/code_editor`, a host for the
Mockaco editor): an inline prompt, inline diff review of every AI edit, and
tools that let the agent chat panel read and edit the open buffers.

## Pieces

```text
agent chat panel (ui_core)                    code_editor plugin
  provider + model the user picked              script_editor/ai/
        │  sync_editor_llm()                      prompt.rs   what is asked / how a reply is read
        ▼                                         inline.rs   the prompt box (a Mockaco block)
plugin_editor_api::llm  ───────────────────────►  tools.rs    chat tools + UI-thread bridge
  EditorLlm: complete(request, on_chunk)        text_editor/buffer_tools.rs   answers tool requests

                                      Mockaco (mockaco-gpui / -diff / -renderer)
                                        blocks        reserved rows with a host view in them
                                        review        hunk-by-hunk accept / reject of a rewrite
                                        events        SelectionChanged, InlineAssistRequested (Ctrl+I),
                                                      QuickFixRequested (Ctrl+.), ReviewUpdated/Finished
```

### One language model, owned by the chat panel

Editors never configure providers. `AgentChatPanel::sync_editor_llm`
(`ui_core/.../agent_chat_panel/editor_llm.rs`) publishes the selected provider
and model through `plugin_editor_api::llm::set_editor_llm`, and withdraws it
while the provider is not `Ready`. It runs after anything that changes the
selection or a provider's state. Editors read it with `editor_llm()`; `None`
means "tell the user to set one up in Agent Chat".

The service is a process-wide static: right for the built-in editors (linked
once), invisible to a plugin loaded as its own DLL.

### Mockaco blocks (inline arbitrary UI)

`DisplayMap::set_blocks` reserves N display rows above or below a buffer line.
The rows are ordinary rows of the line's entry in the line index, so scrolling,
wrapping, folds, hit testing and the minimap needed no new coordinate system.
A block row has `DisplayRow::block = Some(..)`, an empty byte range at the
line's start (above) or end (below), and `continuation = true` (no line
number). Carets skip them (`skip_block_rows`), clicks on them are ignored by
the editor, and edits move blocks with their line (`remap_blocks` carries each
block's line start through the edit's change map).

`WgpuiEditorView::set_block(id, line, placement, rows, AnyView, cx)` pairs the
reservation with a GPUI view painted over it. Block ids below
`REVIEW_BLOCK_BASE` (`1 << 48`) belong to the host. A view in a block has its
own focus; the editor ignores key events unless it is the focused element.

### Inline review

`WgpuiEditorView::begin_review(proposed, cx)` applies the proposal to the
buffer in one transaction, makes the editor read-only, tints added lines, and
shows each hunk's replaced lines in a block above it with Accept / Reject.
The model is `mockaco_diff::review::Review` (line based, no GPUI): accepting
forgets a hunk, rejecting returns the edit that restores its original lines and
shifts later hunks. Keys: Ctrl+Enter accept all, Esc reject all,
Alt+Up/Down move between hunks. Hosts get `ReviewUpdated { pending }` and
`ReviewFinished { accepted, rejected }`.

Every AI edit goes through this -- from the inline box and from chat tools --
so the user always sees what changed and nothing is written until they decide.

### Inline box (Ctrl+I)

`InlineAssist` opens above the selection (or caret line) as a Mockaco block.
Kinds: Edit, Fix, Ask, Explain, Document, Tests. Edit-like kinds ask for a
single fenced block replacing the whole-line region, splice it into the file
and open a review; Ask/Explain stream prose into the box. The request carries
the region, trimmed surrounding code, and the diagnostics inside the region.
A reply is discarded if the file changed while it was written.

Entry points: Ctrl+I (Edit), Ctrl+. (Fix), the editor context menu.

### Chat tools

`code_editor_list_open_files`, `_read`, `_get_selection`, `_get_diagnostics`,
`_propose_edit`, `_replace_file`, `_go_to`. The chat runs tools on a worker
thread; open documents are GPUI entities. Each `TextEditor` registers a request
channel per open path (`ai::tools::register`) and answers on the UI thread
(`buffer_tools.rs`); the tool waits up to 20 s. Tools read the live buffer
(unsaved edits included). Edits are proposals: they open the inline review and
return `awaiting_user_review`. One pending review per file.

## Not built yet

- Inline completions (ghost text) -- Mockaco renders none; `mockaco-lsp` can
  already issue `textDocument/inlineCompletion`.
- Code-action lightbulb / menu -- `mockaco-lsp` has code actions, the view has
  no surface for them; Ctrl+. currently opens the AI Fix prompt.
- Chat tools for hover / definition / references (the LSP calls are
  callback-based on the UI thread; they need a reply bridge like `buffer_tools`).
- Syntax highlighting of the replaced lines in a review hunk (plain text now).
- A floating "Ask / Edit" affordance on selection (`SelectionChanged` exists).
- Passing the selection / active file to the chat panel automatically.

## Dev wiring

Mockaco changes live on its `ai-inline-integration` branch. Until it is pushed
and the `rev` in `plugins/vendor/code_editor/Cargo.toml` re-pinned, the root
`Cargo.toml` and the plugin's own manifest carry a local `[patch]` to the
checkout under `Documents/GitHub/Mockaco`. Remove both when re-pinning.
