# Language plugins: audit and plan

**Status: Phase 1 (the audit) is written and Phases 2 to 6 are built (section 6 "As built", section 8); what remains is the cleanup pass ("After P6").** Every claim about
current code was read for this document, with `file:line` (lines are at the commit this
branch is based on, `d4ae72365`). Anything not read is marked **UNVERIFIED** (section 9).
The decisions in section 5 are **Proposed**; section 6 says which need the user's answer
before the phase that depends on them.

Constraints already decided with the user (not reopened here):

- A language is contributed by **one plugin registration** carrying everything about it:
  language metadata, an optional script compiler, an optional language server spec. The
  plugin handles all of it; the core hosts and routes and knows no language.
- Rust (rust-analyzer) becomes a plugin, `pulsar.rust`, enabled by default.
- The existing TypeScript plugin (`plugins/vendor/typescript`,
  `crates/core/pulsar_script_ts`) adds typescript-language-server, and its results appear
  everywhere LSP results are consumed.
- No dual-mode fallbacks, no compatibility aliases: delete the old path and let compile
  errors guide the fixes. Multi-phase work is built in full first; non-blocking problems
  go to the issues log (section 8) and are fixed in a cleanup pass.
- Core (`crates/core`, `crates/editor`, `crates/subsystems`) depends on no plugin and
  names no language (`just arch`, `architecture/layers.toml`, `architecture/kind-names.txt`).

## 0. Findings that change the plan (read these first)

1. **The LSP "service" is a rust-analyzer manager, not a client.** `RustAnalyzerManager`
   (`crates/core/pulsar_lsp/src/rust_analyzer/mod.rs:100`) hard-codes the binary
   (`discovery.rs:61`), the rust-analyzer initialization options (`process.rs:535-566`,
   `checkOnSave`, `check.command = "clippy"`, `cargo.buildScripts`, `procMacro`), the
   `rust-analyzer/serverStatus` notification (`notifications.rs:27`, `:309`) and the
   rust-analyzer experimental capability (`process.rs:~530`). The generic JSON-RPC
   transport, document refcounting and diagnostics parsing live in the same `impl`. It is
   one process, one workspace, one language.
2. **The client never answers a server request.** The stdout reader (`process.rs:329-366`)
   treats any message with an integer `id` as a *response*: it looks the id up in
   `pending_requests` (`:331`), compares it to `init_id` (`:343`) and otherwise drops it;
   `handle_lsp_message` returns early on any `id` (`notifications.rs:15`). So
   `client/registerCapability`, `workspace/configuration`, `window/workDoneProgress/create`
   (the match arm at `notifications.rs:24` is unreachable because that message has an id)
   and `workspace/applyEdit` are never replied to, and a server request id that equals a
   pending client id is delivered as that request's *response* (an id-space collision).
   rust-analyzer tolerates this; **typescript-language-server does not** (it sends
   `workspace/configuration` and `client/registerCapability` and waits). The generic client
   in P2 must separate the two id spaces and answer server requests.
3. **The Problems drawer shows only the last file published, and never clears.** The
   aggregate `AnalyzerEvent::Diagnostics` is sent only when a file's set is non-empty
   (`notifications.rs:101-104`); `on_diagnostics` hands that one file's set to
   `ProblemsDrawer::set_diagnostics`, which *replaces* the server's diagnostics
   (`ui_problems/src/screen.rs:85-104`, `last_published`). The per-file event
   (`FileDiagnostics`, sent also when empty) is ignored by the shell
   (`ui_core/src/app/event_handlers.rs:84`). The drawer already has source-keyed
   slots (`set_external_diagnostics`, `screen.rs:109`, used by `script_problems.rs:70`), so
   the store in P2 is a fix as well as a generalization.
4. **The diagnostic path is also wrong for non-Windows or percent-encoded URIs.**
   `handle_diagnostics_notification` derives `file_path` by trimming `file:///` and
   replacing `%20` (`notifications.rs:93`) instead of `path_utils::uri_to_path`; the
   per-file *key* does use `uri_key` (`:104`). `Diagnostic.file_path` (what the drawer
   navigates with) can therefore be wrong (`c%3A`, and a Unix path loses its leading `/`).
5. **Dead code in the LSP area** (delete in P2, no alias):
   `LspBackend` (`pulsar_lsp/src/traits.rs:19`, implemented nowhere);
   `GlobalRustAnalyzerCompletionProvider` (`engine_backend/src/services/lsp_completion_provider.rs:17`,
   only its definition and re-export `services/mod.rs:33` mention it); the whole
   `rust_analyzer` module of `ui` (`wgpui-component/crates/ui/src/input/lsp/rust_analyzer.rs`,
   627 lines: `RustAnalyzerClient`, `RustAnalyzerCompletionProvider`, `MockRust*`),
   re-exported by `lsp/mod.rs:12,19` and used by nobody;
   `PulsarApp::new_with_project_and_analyzer`, `new_with_shared_analyzer` and
   `PulsarApp::rust_analyzer()` (`ui_core/src/app/constructors.rs:38,65,199`: no caller);
   the splash's `_analyzer` (`ui_loading_screen/src/screen/mod.rs:32,55,112`) which builds
   a `RustAnalyzerManager` that nothing reads, and whose constructor runs binary discovery
   (`find_or_use_bundled_analyzer`, spawning `rust-analyzer --version` / `rustup which` on
   the UI thread).
6. **The code editor does not use the provider-based `Lsp` struct at all.** The Script
   Editor is Mockaco (`WgpuiEditorView`); it asks the shared manager through raw JSON
   requests (`text_editor/lsp.rs:336-1030`). The `Lsp` struct with `CompletionProvider`,
   `HoverProvider`, `DefinitionProvider` (`wgpui-component/.../input/lsp/mod.rs:23` and
   `wgpui-base/src/input/editor/lsp/mod.rs:33`) is a UI-layer extension point whose only
   non-test user is the inspector's code box (`ui/src/inspector.rs:104-111`). It stays as
   it is (foundation layer, not ours); the target does not route through it.
7. **Both script plugins name each other.** The one-language-per-class check exists twice,
   each side hard-coding the other's marker file: TypeScript refuses a class with
   `graph_save.json` (`plugins/vendor/typescript/src/compile.rs:36`), Blueprints refuses a
   class with `class.ts` (`blueprint_editor/src/features/class_dirs.rs:159`). Neither knows
   a third language. The host must own the check (section 2).
8. **`class.ts` / `graph_save.json` are in the kernel in at least 12 places**, none of
   which the layer gate sees, because the gate matches kind words (`[kinds]` in
   `layers.toml`), and "typescript", "blueprint" and "rust" are not in it (section 2, P4).
9. **Server-specific behaviour has no seam.** The status bar, notifications and the
   code editor's status line all pattern-match `AnalyzerStatus`, and the code editor sends
   `rust-analyzer/*` and `experimental/parentModule` requests by name
   (`text_editor/lsp.rs:931,940,980,1030`). A second server needs a capability-gated
   surface, not a second copy of each.
10. **One `.ts` problem is outside the LSP**: `pulsar_script_ts` compiles a *subset*
    (`pulsar_script_ts/src/lib.rs`); tsserver accepts constructs the compiler rejects
    (imports, arrow functions, generics ...). Compiler problems must reach the editor as a
    second diagnostic source (section 4.3), or a file shows green in the editor and
    red at Play.

## 1. The LSP stack today

### 1.1 `crates/core/pulsar_lsp`: generic client versus rust-analyzer

2,571 lines in `src/rust_analyzer/` (plus `lib.rs` 14, `traits.rs` 25). Depends on
`gpui-ce`, `ui` (for `ui::diagnostics::Diagnostic`), `lsp-types`, `editor_task_queue`.

| Part | Where | Generic LSP client | rust-analyzer specific |
|---|---|---|---|
| Binary discovery and install | `discovery.rs` (514) | no | **all of it**: `PULSAR_RUST_ANALYZER_PATH`, `rustup which`/`component add`, `~/.cargo/bin`, engine deps dir, GitHub release download (`:61`, `:250`, `:303`) |
| Spawn, stdio, stderr pump | `process.rs:111-270` | yes (but stderr lines are tagged `[rust-analyzer stderr]`; on spawn failure it calls the installer `:148`) | the fallback-install branch |
| `initialize` request | `process.rs:377-592` | the capabilities block (`:395-533`) is a general client capability set | `initializationOptions` (`:535-566`: `checkOnSave`, `check.command`, `cargo`, `procMacro`, `completion`, `inlayHints`) and `experimental.serverStatusNotification` (`:~530`); `rootUri` built by hand (`:382-387`, a Windows `C:/` special case instead of `path_to_uri`) |
| Reader loop | `process.rs:285-372` | framing yes; id handling wrong (finding 2) | |
| Notification dispatch | `notifications.rs:12-32` | `$/progress`, `publishDiagnostics` | `rust-analyzer/serverStatus` (`:27`, `:309`: quiescent -> Ready) |
| Progress mapping | `notifications.rs:34-84` | `$/progress` begin/report/end shape | title heuristics: "Fetching"/"Loading" -> "Loading metadata...", "Indexing"/"Building" -> "Indexing crates..." (cargo/rustc wording) |
| Status machine | `progress.rs:17-43,95-160` | the coalescing (100 ms / 5% milestones, `mod.rs:79-84`) | "Ready" by *timer*: 3 s without progress (`progress.rs:31`) or diagnostics seen for 2 s (`:143-149`), because it does not use work-done end tokens; `ProcessExited` text names rust-analyzer |
| Document sync | `document_sync.rs` | `didOpen`/`didChange`/`didSave`/`didClose`, refcounted `open_documents` (`mod.rs:136`), incremental change (`:112`), cached per-file diagnostics (`:152`), server trigger characters (`:147`) | `did_open_file` takes a `language_id` argument but callers pass `"rust"` (`lsp_completion_provider.rs:53`, `text_editor/lsp.rs:186`) |
| Requests | `requests.rs`, `transport.rs:45-119` | `send_request_async`, hover, definition, code actions + resolve | code-action parsing carries rust-analyzer's `data` quirks (`notifications.rs:170-310`) |
| Paths | `path_utils.rs` (110) | all | |
| Types | `mod.rs:29-74` | `AnalyzerStatus`, `AnalyzerEvent` (`StatusChanged`, `IndexingProgress`, `Ready`, `Error`, `Diagnostics`, `FileDiagnostics`) are neutral in meaning, rust-flavoured in name | |
| Lifecycle | `process.rs:22` `start`, `:594` `stop`, `:630` `restart` (500 ms sleep on the UI thread) | | `restart` is "restart the one workspace" |

What is reusable almost as is: framing, request/response correlation (after the id fix),
document refcounting, incremental sync, status coalescing, path helpers, the capabilities
block. What must be parametrised: the executable and its acquisition, the initialization
options, the progress-token interpretation and readiness rule, the server-status
extension, the language id, the roots.

`LspBackend` (`traits.rs:19`: `start(&Path)`, `stop()`, `status() -> &AnalyzerStatus`) has
no implementer and no user. It is the right *idea* (a generic backend) with the wrong
shape (synchronous, no GPUI, no documents, no diagnostics); P2 deletes it and replaces it
with `LanguageServer` (section 3.4).

### 1.2 Process model today

One `Entity<RustAnalyzerManager>`, created in `ui_core::construction::connect_rust_analyzer`
(`construction.rs:287-313`), started with the project path (`:302`), handed to the
plugin manager as `EditorServices::language_server` (`construction.rs:333`) and so to
every editor. It is driven from `render` (`ui_core/src/app/render.rs:671`,
`update_progress_from_thread` drains the reader channel on the UI thread every frame).
The status bar Stop/Restart/Start buttons call it directly (`render.rs:369,397`). There is
no per-language selection, no per-file routing (`is_rust` gate in the editor), no root
discovery (the project root *is* the workspace; the generated project has a root
`Cargo.toml`, `core_project_builder.rs:449-590`), and no tie to plugin enablement.

### 1.3 Consumers

"Needs" is what the consumer takes from the server today; "Target" is what it consumes
after P2 (section 3.4). `LS` is the `LanguageServers` service, `DS` the `DiagnosticsStore`.

| Consumer | File:line | Needs today | Target |
|---|---|---|---|
| `EditorServices.language_server` (plugin API) | `plugin_editor_api/src/editor_element.rs:42,52,63,72,87`; dep `pulsar_lsp` behind `editor` (`Cargo.toml:28,57`) | typed `Entity<RustAnalyzerManager>` to every editor, DLL plugins included (a plugin links its own copy of the SDK, so this entity type crosses the DLL boundary by layout) | `language_servers: Entity<LanguageServers>` (a host-owned type; same `Weak` treatment as today), `server_for(path)` |
| Plugin manager test | `plugin_manager/tests/editor_services.rs:12,70-168`; dep `plugin_manager/Cargo.toml:41` | constructs a manager, checks weak handling | rebuild on `LanguageServers` |
| Script editor panel | `code_editor/src/script_editor/mod.rs:61,101,155,167-174` | holds the entity, forwards `set_rust_analyzer` to the text editor; the diff view gets `EditorServices::default()` so it is never announced (`:104`) | same, renamed `set_language_servers` |
| Text editor: create | `text_editor.rs:169-171` (attach), `:194` (`lsp::is_rust(path)` turns on `set_language_intelligence`) | one server, `.rs` only | per-open-file `server_for(path)`; the gate is "a server claims this path" |
| Text editor: LSP client | `text_editor/lsp.rs` (1,676 lines): attach `:63-92`; `is_rust` `:56`; status line `:97-116` (names rust-analyzer, matches `AnalyzerStatus`); events `:122-148`; `didOpen` with `"rust"` `:186`; trigger fallback `:53`; `didSave` `:248`, `didClose` `:260,272`; hover `:336`, completion `:395,419,488`, definition `:549`, references `:669`, document symbols `:699`, formatting `:868`, workspace symbols `:915`; **rust-analyzer only**: `experimental/parentModule` `:931`, `rust-analyzer/openCargoToml` `:940`, `rust-analyzer/expandMacro` `:980`, `rust-analyzer/reloadWorkspace` `:1030` | one manager, per-file diagnostics, request channel | per-file `server_for(path)`; standard requests unchanged; the four extension requests become **server commands** a language contributes (`LanguageServerSpec::commands`) and the editor's menu lists what the active server offers; per-file diagnostics from `DS` filtered to this file |
| Builtin provider | `code_editor/src/builtin_provider.rs:167` (warns when `language_server` is `None`), file types `rs`/`js`/`ts`/`py`/`lua`/`toml`/`md` `:27,53,126`; same list in `code_editor/src/lib.rs:63-162` | | file types come from `Language` registrations (section 3.1), the code editor keeps only languages with no plugin (toml, md) |
| Code editor example | `code_editor/examples/standalone.rs:11,48-53` | builds a manager | builds a `LanguageServers` with no specs |
| AI tools | `code_editor/src/script_editor/ai/tools.rs:230` `code_editor_get_diagnostics`; `ai/inline.rs:311` `lsp_diagnostics()` | the open view's diagnostics (from the server through the view) | `DS.for_file(path)` (all sources) |
| Shell: construction | `ui_core/src/app/construction.rs:287-340`; `constructors.rs:13,41-72,79,114-136`; `state.rs:3,37` | create/start the manager, subscribe `on_analyzer_event`, store it in app state | construct `LanguageServers` once, `start_for_project(path)` (it starts the servers of the enabled languages); subscribe to `LanguageServersEvent` |
| Shell: events | `ui_core/src/app/event_handlers.rs:14,24-35` (`status_summary`), `:58-90` (dispatch), `:94-112` (notifies "Rust Analyzer Ready" via `t!("Notification.Title.RustAnalyzerReady")`), `:132-175` | footer text and a success/error toast per status | one footer entry per running server, text from the language's `display_name`; toasts from the same event, titled by server |
| Shell: diagnostics | `ui_core/src/app/event_handlers/analyzer_diagnostics.rs:15,53,91,260-380` | `AnalyzerEvent::Diagnostics` (one file's batch), then a code-action round trip per diagnostic through `rust_analyzer.read(cx).request_code_actions_*`, `parse_code_actions`, `resolve_code_action_async` | subscribe to `DS`; push the aggregate to the drawer with `set_external_diagnostics(source, ..)` per source; fetch quick fixes through the diagnostic's `server` id (a `DS` entry records which server and which raw diagnostic) |
| Shell: footer and controls | `ui_core/src/app/render.rs:3,35-75` (status colour/icon from `AnalyzerStatus`), `:369,397,401-403` (stop / restart / start), `:671` (poll per frame); `status_bar.rs:4` (doc only) | `status()`, `is_running()`, `stop`, `restart`, `start` | per-server status from `LS`; start/stop/restart act on a server id; the poll moves into the client (a task, not `render`) |
| Problems drawer | `ui_problems/src/screen.rs:85-109`, `components/content.rs:734-748` (extension -> highlighter language table: rs, js, ts, py, toml, json, md, html, css) | `set_diagnostics` for the server, `set_external_diagnostics(source, ..)` for others; language table | unchanged API; fed from `DS`. The table is replaced by the `Language` registry (`language_for_path(path).highlight_id`) |
| Loading screen | `ui_loading_screen/src/tasks.rs:44` ("Checking language server"), `:298-318` (`task_check_lsp`: scans PATH for `rust-analyzer(.exe)`); `screen/mod.rs:5,32,55,112` (dead manager) | PATH probe of one binary | one "Checking language servers" step per language with a server spec, asking its `locate()`; the dead manager is deleted |
| `engine_backend` | `services/mod.rs:6,24-39` (`rust_analyzer_manager` module, three re-exports), `lib.rs:29` (`pub use services::RustAnalyzerManager`), `Cargo.toml:77,122` (`editor-ui` feature pulls `pulsar_lsp`), `services/lsp_completion_provider.rs` (dead, finding 5) | a re-export seam so editor and plugins reach the manager through `engine_backend` | deleted; hosts use `pulsar_lsp` (or its renamed successor) directly |
| `core_project_builder.rs` | `engine_backend/src/services/core_project_builder.rs:449-590` (writes the project's `Cargo.toml` and workspace), `:735` comment | produces the Cargo project rust-analyzer indexes; does not talk to the server | stays in core (it generates the *game* crate, which is Rust by construction); the `pulsar.rust` server spec only *reads* its output (workspace discovery, section 4.1) |
| `pulsar_project/src/apply.rs` | `:180-190` (atomic write; comment: "Cargo and rust-analyzer read these files") | no code dependency; atomic writes avoid half-written manifests reaching the server's file watcher | unchanged; reword the comment |
| `wgpui-component` rust-analyzer module | `crates/ui/wgpui-component/crates/ui/src/input/lsp/rust_analyzer.rs` | nothing (`RustAnalyzerClient`, `RustAnalyzerCompletionProvider`, `MockRust*`: no references outside the file, verified by grep over `crates` and `plugins`) | **delete in P2**. It is a submodule (`crates/ui/wgpui-component`, `heads/main`): a WGPUI-Component PR first, then re-pin (`.agents/CRATES.md`, "Submodule pins") |
| Provider-based `Lsp` struct | `wgpui-component/.../input/lsp/mod.rs:23`; `wgpui-base/src/input/editor/lsp/mod.rs:33` | `CompletionProvider`/`HoverProvider`/`DefinitionProvider` hooks on `InputState`; real user: `inspector.rs:104-111`; `ui::input::{HoverProvider,DefinitionProvider}` re-exported by `pulsar_lsp/src/traits.rs:16` | untouched (foundation). The `pulsar_lsp` re-export goes with `traits.rs` |

Four `Diagnostic` types are in play and the store must not add a fifth: `ui::diagnostics::Diagnostic`
(`wgpui-component/.../diagnostics.rs:37`, what `pulsar_lsp` emits, carries `raw_lsp_diagnostic`
and `code_actions`), `ui_common::Diagnostic` (what `analyzer_diagnostics.rs` takes),
`ui_problems::Diagnostic` (what the drawer renders, `to_problems` converts), Mockaco's
`DiagnosticInput` (`text_editor/lsp.rs` converts). The store holds the `ui` one (a foundation
type both the client and the shell may name) and the consumers keep their existing
conversions.

## 2. Script-language hardcoding today

The engine's script model: a class is a directory `src/classes/<Class>/`; its compiled
module lives in the project module store keyed by class id, with a `language` marker file
(`pulsar_class/src/modules.rs:41,107-151`); every language compiles to
`pulsar_script_vm` bytecode. What names a language or its source file in the kernel or the
hosts:

| # | Place | What it hard-codes | The language registration must declare |
|---|---|---|---|
| 1 | `pulsar_class/src/registry.rs:333` `is_class_dir` | `[CLASS_META_FILE, PREFAB_FILE, "graph_save.json", "class.ts"]` are the files that make a directory a class | `class_markers: ["graph_save.json"]` / `["class.ts"]`; `is_class_dir` asks the registry (it runs in headless tools too, so the registry must be reachable without the editor) |
| 2 | `pulsar_class/src/modules.rs:10,41,263-269,319,368-376` | language ids `blueprint`, `typescript` in docs, the legacy migration (`unwrap_or_else(|| "blueprint")` `:267`) and tests | the id is data the language owns; the migration's `blueprint` default becomes "the language whose marker file the class has" |
| 3 | `pulsar_class/src/id.rs:13`, `lib.rs:57` | `class.json`, `prefab.json` (class metadata, not a language) | nothing: host-owned, stays |
| 4 | `pulsar_package/src/compile.rs:117-132` | `graph_save.json` -> "build this tool with the `blueprint` feature"; `class.ts` -> "...`typescript` feature"; `:352` names the features. `pulsar_package` is the *host* layer, so naming is allowed, but the knowledge is duplicated | `class_markers` and `display_name`; the message is built from the registry ("has `<marker>` source but no compiled module: language `<name>` is not linked") |
| 5 | `plugin_manager/src/registry.rs:56-59` | comment and logic for folder classes: any registered `FolderBased { marker_file }` file type claims a directory; the Blueprint plugin registers `graph_save.json` (`blueprint_editor/src/lib.rs:71-72`) | `class_markers` double as folder-file-type markers: the registry derives the class folder type from the language registration, so a TS class folder is recognised as a class too (today it is not) |
| 6 | `pulsar_project/src/model.rs:55-57,219` | `COMPILE_TARGET_KEY` is a key of `graph_save.json`; `rust_classes` reads that file directly to find classes whose saved target is Rust (`:219`) | the generate-to-Rust option is a *language capability* (`ScriptLanguage::compile_target(class_dir)`), not a Blueprint-file peek. UNVERIFIED how much of `generate_class` is Blueprint-specific (section 9) |
| 7 | `blueprint_editor/src/features/class_dirs.rs:159-163` | refuses a class with `class.ts` (names TypeScript) | host-owned check, section 2.2 |
| 8 | `typescript/src/compile.rs:9-12,34-51` | `GRAPH_FILE`, the check naming Blueprints and the module's `language` | same |
| 9 | `ui_common/src/file_utils.rs:16,39-43` and `ui_core/src/file_utils.rs:14,37-39` | `FileType::Class` is "a folder containing graph_save.json", displayed "Blueprint Class" (`:21`) | `is_class_folder` asks the registry; display name from the language |
| 10 | `ui_file_manager/src/utils/tree.rs:21,73` | the folder tree hides directories holding `graph_save.json`, `struct.json`, `enum.json`, `trait.json`, `alias.json` (they appear as files) | `class_markers` of all languages + the other kinds' markers from `FileTypeDefinition::FolderBased` |
| 11 | `ui_level_editor/src/ui/viewport/helio_viewport/assets.rs:26,405-408` | a dropped Blueprint asset must contain `graph_save.json`; error "Not a valid blueprint class" | "a class dir", from `is_class_dir` |
| 12 | `pulsar_traits/src/roles.rs:167-176` | `RoleBinding::class_name`: asset `<Class>/graph_save.json` -> class name | the asset-to-class rule is the `TraitImplementerSource` of the language (`blueprint_editor/src/builtin_provider.rs:~105` already registers one); a language with traits registers its own. Out of scope for P3 unless TS declares traits (open question Q9) |
| 13 | `ui_level_editor/src/ui/world_settings/panel/mod.rs:695,~761` | label of a `.../graph_save.json` role binding | same as 12 |
| 14 | `ui_types_common/src/asset_kind.rs:53`, `ui_level_editor/src/ai/tools/mod.rs:161-173` | `"blueprint"` asset kind and `ObjectType::Blueprint` | out of scope: an asset *kind* (placing a class), recorded as a known name; moves with #1602 |
| 15 | `ui_problems/src/components/content.rs:734-748` | extension -> highlight language | `Language::highlight_id` by extension |
| 16 | `wgpui-component/.../code_editor.rs:29-33`, `highlighter/languages.rs:91-165` | highlighter's own language table (`rs`, `ts`, `tsx`, ...) | foundation layer: a grammar table, not a language registration. Stays; `Language::highlight_id` names an id in it |
| 17 | `code_editor/src/lib.rs:63-162`, `builtin_provider.rs:27-126`, `file_explorer.rs:435` | file types and icons per extension; `examples.rs` per language | `Language::file_extensions`, `icon`; examples stay the code editor's |
| 18 | `pulsar_script_ts/src/lib.rs`, `pulsar_script_ts` crate itself in `crates/core/` | a language compiler as a core crate (`.agents/CRATES.md` lists it among core) | moves to `plugins/vendor/typescript/` as a package of the plugin in P5/P6 (the layer gate allows a plugin crate to be a workspace member anywhere in `plugins/vendor/<dir>`); `pulsar_script_vm` stays core |
| 19 | `engine/tests/linked_editors.rs:9-43` | asserts the editor's and headless lists agree | updated to `Language` |

Not in this list because they are the generated *game* crate, not a language registration:
`core_project_builder.rs` writes Rust by construction, `pulsar_script_codegen` is the Rust
exporter. They name Rust as the engine's own implementation language.

### 2.1 The one-language-per-class check, today

`class.ts` + `graph_save.json` in one directory is refused by *each* plugin when it
compiles (`compile.rs:36`, `class_dirs.rs:159`), and a module written by a different
language is refused by *each* (`compile.rs:43-49`, `class_dirs.rs:~164-172`, through
`ModuleStore::language`). Nothing refuses it at class scan, in the file manager or in the
editor, so the user finds out at compile; a third language would have to be added to both
existing checks.

### 2.2 Target

The host (`pulsar_class`) owns it: `ClassRegistry` assigns each class dir the languages
whose `class_markers` are present; more than one is a `ClassProblem::MultipleLanguages
{ class, languages }` raised at scan, shown in the Problems drawer and refusing compile for
that class from either language; the module's recorded `language` must be the one language
the registry resolved, else `ClassProblem::ModuleFromOtherLanguage`. The plugins' own
checks are deleted; a plugin's `compile_class_dir` can assume it was handed a class it owns.

## 3. How registration works, and the `Language` contribution

### 3.1 The machinery (what exists)

- A plugin is `plugins/vendor/<dir>/` with `pulsar-plugin.toml` (`id`, `enabled_by_default`,
  `root`, `editor_only`, `provides`/`requires`: `.agents/PLUGIN_SYSTEM.md`). Its root crate's
  `build(p: &mut PluginBuilder)` is the only place it registers (`p.add(x)`; `x.origin ==
  p.origin()` is checked).
- A registration type implements `pulsar_plugins::Contribution` (`CATEGORY`, `origin()`,
  `id()`) and `pulsar_plugins::registration!(T)`; registries iterate
  `pulsar_plugins::registered::<T>()`, which skips origins whose plugin is disabled
  (`pulsar_plugins/src/lib.rs:352-369,768`). `generation()` (`:748`) changes on every
  `activate`/`deactivate`, so a cache rebuilds when it moves.
- A project's `Pulsar.toml` `[plugins]` list decides enablement
  (`pulsar_project/src/descriptor.rs`); `engine_state::set_project_path` activates it
  (`engine_state/src/lib.rs:136-146`), as do the packager's entry points
  (`pulsar_package/src/lib.rs:143,398`).
- **Today's language registrations** (all gated like any other): `LinkedScriptLanguage
  { origin, id, create }` (`plugin_editor_api/src/scripting.rs:186-206`, category
  `script.language`), read by `linked_script_languages()` (`:212`); `PluginEditorProvider`
  (`plugin_manager`; the TS plugin registers `TypeScriptProvider`, a provider with no file
  types and no editors, only to return `script_languages()` -
  `typescript/src/provider.rs:48-88`), read by `PluginManager::script_languages`
  (`plugin_manager/src/contributions.rs:55`) for pre-Play `validate_scripts` (`:68`); and
  `EditorPluginScripting::script_languages` for DLL plugins (`scripting.rs:170`,
  `export_plugin!(T, scripting)`). So a language is registered **twice** (linked, for the
  packager; provider, for the editor), each side listing it independently
  (`engine/tests/linked_editors.rs` exists to keep them equal). The Blueprint plugin does
  both (`blueprint_editor/src/builtin_provider.rs:46,119`).
- A DLL plugin library exports its root's registrations as data
  (`pulsar_plugins::library`, fingerprinted by type name, size, alignment, `VERSION`;
  `.agents/PLUGIN_SYSTEM.md`, "Plugin libraries"). A registration type containing `fn`
  pointers and `Arc<dyn Trait>` crosses it only if host and library agree on the layout,
  and the library's code stays resident (libraries are never unloaded), which is what
  lets a `dyn ScriptLanguage` live. A language server spec is plain data plus function
  pointers: it fits. **A running server process, however, is the host's**, never the
  library's (a library has its own copy of every crate's statics).

### 3.2 The proposed single contribution

One registration type, `Language`, owned by `plugin_editor_api` (which is runtime-safe by
default; `ScriptLanguage` is already there). It replaces `LinkedScriptLanguage` and the
provider-with-no-editors trick. It is a *sketch*; names are for review.

```rust
// plugin_editor_api::language (default features: no GPUI, no process spawning)

/// Everything one language brings. Registered once from a plugin's root `build`.
pub struct Language {
    pub origin: &'static str,                 // pulsar_plugins::origin!()
    pub id: &'static str,                     // "typescript": the module store's `language` marker
    pub display_name: &'static str,           // "TypeScript"
    /// Extensions without the dot, lower case: ["ts", "mts", "cts"].
    pub file_extensions: &'static [&'static str],
    /// The LSP `languageId` per extension when it is not `id` ("tsx" -> "typescriptreact").
    pub language_ids: &'static [(&'static str, &'static str)],
    /// A grammar id in the UI highlighter's table (foundation owns the table).
    pub highlight_id: &'static str,
    /// Files whose presence makes a directory one of this language's classes
    /// (`class.ts`, `graph_save.json`). Empty: the language has no classes.
    pub class_markers: &'static [&'static str],
    /// Project files that change this language's server configuration
    /// (`tsconfig.json`, `Cargo.toml`): the host restarts or reconfigures on change.
    pub config_files: &'static [&'static str],
    /// A compiler to engine bytecode. `None`: editor-only language (Rust as a *source*
    /// language; its game code is the engine's own crate, not a script).
    pub compiler: Option<fn() -> Arc<dyn ScriptLanguage>>,
    /// A language server. `None`: highlighting only.
    pub server: Option<fn() -> LanguageServerSpec>,
}
impl Contribution for Language { const CATEGORY: &str = "language"; /* origin, id */ }
pulsar_plugins::registration!(Language);
```

```rust
/// What the host needs to run and talk to one server. Data and functions, no process.
pub struct LanguageServerSpec {
    /// Stable id for status, logs, settings ("rust-analyzer", "typescript-language-server").
    pub id: &'static str,
    pub display_name: &'static str,
    /// Find (or obtain) the executable. Called on a worker thread, may block and may
    /// download, reporting progress through the handle. Returns how to launch it.
    pub locate: fn(&LocateContext) -> Result<ServerCommand, LocateError>,
    /// The directories to start a server in: a server per root. Default: the project root.
    pub roots: RootStrategy,                  // ProjectRoot | NearestMarker(&'static [&'static str]) | Fn(&Path)->Option<PathBuf>
    /// The `initializationOptions` value; also the answer to `workspace/configuration`.
    pub initialization_options: fn(&ServerContext) -> serde_json::Value,
    pub configuration: fn(&ServerContext, section: &str) -> serde_json::Value,
    /// Extra client capabilities merged into the host's base set (e.g. rust-analyzer's
    /// `experimental.serverStatusNotification`).
    pub client_capabilities: fn() -> serde_json::Value,
    /// Hooks for what only this server knows (all optional, default no-op).
    pub hooks: ServerHooks,
    /// Server-specific requests the editor may offer ("Open Cargo.toml", "Expand macro").
    pub commands: &'static [ServerCommandSpec],
    /// Written into the workspace before the server starts (tsconfig for TypeScript).
    pub prepare_workspace: Option<fn(&ServerContext) -> Result<(), String>>,
}

pub struct ServerHooks {
    /// Map a `$/progress` begin/report/end (token, title, message, percentage) to a status
    /// text and whether it counts as indexing. Default: show `title: message`.
    pub progress: fn(&ProgressEvent) -> Option<ProgressUpdate>,
    /// A server-specific notification (`rust-analyzer/serverStatus`): may report Ready.
    pub notification: fn(&str /*method*/, &serde_json::Value) -> Option<ServerSignal>,
    /// When is the server Ready? `WorkDoneEnd` of all tokens (default) or a hook signal.
    pub readiness: ReadinessRule,             // AllProgressEnded | Signal | Immediate
    /// A file was saved: extra work (rust-analyzer `check on save` is server-side already).
    pub on_save: fn(&Path) -> Vec<ServerMessage>,
    /// Post-process a diagnostic (rust-analyzer's `data` quirks, tsserver's `code`).
    pub diagnostic: fn(&mut Diagnostic),
}

pub struct ServerCommand { pub program: PathBuf, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)> }
```

`ScriptLanguage` gains a live-check hook (section 4.3):

```rust
pub trait ScriptLanguage {
    /* existing */
    /// Diagnostics for one file's *unsaved* text, without writing anything, for the editor.
    /// The host calls it (debounced) on edit and publishes the result to the DiagnosticsStore
    /// as source `compiler:<id>`. Default: none.
    fn check_source(&self, ctx: &CheckContext, path: &Path, text: &str) -> Vec<CompileDiagnostic> { Vec::new() }
}
```

DLL plugins: `export_plugin!` gains no new symbol; a dynamic plugin returns its
`Language` values from `EditorPlugin::languages()` (default `Vec::new()`), replacing
`EditorPluginScripting`; the loader merges them into the same store as linked ones (an
`ABI`-version bump of the trait; the plugin API is pre-1.0 and has no compatibility
promise, per the no-aliases convention).

### 3.3 Host side (what the core owns)

- **`LanguageRegistry`** (a kernel crate, `pulsar_language`, GPUI-free so `pulsar_class` and
  `pulsar_package` use it): `all()` over `pulsar_plugins::registered::<Language>()`,
  `for_path(&Path)` (extension, longest first), `by_id`, `class_languages(dir)` (markers),
  `is_class_dir`. It caches against `pulsar_plugins::generation()`. This is where the
  one-language check (section 2.2) lives.
- **`LanguageServers`** (in `pulsar_lsp`, GPUI): the service editors receive in
  `EditorServices`. Section 3.4.
- **Disable semantics.** `registered::<Language>()` already drops a disabled plugin's
  languages. `LanguageServers` observes `pulsar_plugins::generation()` (polled by its
  scheduler tick, no new callback seam in `pulsar_plugins`): when the set of enabled
  languages with servers changes it (1) stops every server whose language is gone
  (shutdown/exit, then kill after a timeout), (2) calls
  `DiagnosticsStore::clear_server(id)`, which removes that server's entries *and emits
  the per-file change events* so open editors and the drawer clear, (3) closes its
  documents, (4) emits `LanguageServersEvent::Stopped { reason: PluginDisabled }`. A file
  of a disabled language then has no `server_for`, so the editor turns language
  intelligence off for it the same way it does for an unknown extension. A newly enabled
  plugin starts lazily (below). Compile diagnostics (`compiler:<id>` source) are cleared
  the same way. Same for project switch (`set_project_path` activates a new set).
- **Start policy.** A server starts when (a) the project is open and (b) a file of its
  language is opened in an editor or the project contains `config_files`/class markers of
  it (Q4). Rust is today started at project open unconditionally
  (`construction.rs:302`); keeping that behaviour for `pulsar.rust` is `eager = true` in
  its spec, a `bool` on the spec, not a special case in the host.

### 3.4 The `LanguageServers` service and `DiagnosticsStore`

```rust
pub struct LanguageServers { /* servers by (server id, root), registry snapshot, store */ }
impl LanguageServers {
    /// The running or starting server that handles `path`, creating it lazily.
    pub fn server_for(&self, path: &Path, cx: &mut App) -> Option<Entity<LanguageServer>>;
    pub fn servers(&self) -> Vec<Entity<LanguageServer>>;     // footer, Tasks window
    pub fn diagnostics(&self) -> Entity<DiagnosticsStore>;
    pub fn start_project(&mut self, root: &Path, cx: &mut Context<Self>);   // activation
}
/// One server process (one id, one root): the generic client of section 1.1.
impl LanguageServer {
    pub fn status(&self) -> &ServerStatus;               // Idle|Starting|Indexing{..}|Ready|Error|Stopped
    pub fn did_open/did_change_incremental/did_save/did_close(..);
    pub fn request(&self, method: &str, params: Value) -> Result<flume::Receiver<Value>>;
    pub fn completion_triggers(&self) -> &[char];
    pub fn commands(&self) -> &'static [ServerCommandSpec];
    pub fn start/stop/restart(&mut self, cx);
}
/// Events: StatusChanged { server }, Progress { server, text, percent }, Ready { server },
/// Error { server, message }, Stopped { server, reason }.

pub struct DiagnosticsStore { /* file key -> source -> Vec<Diagnostic> */ }
pub enum DiagnosticSource { Server(&'static str /*server id*/), Compiler(&'static str /*language id*/), Play }
impl DiagnosticsStore {
    pub fn set(&mut self, source: DiagnosticSource, key: &str, d: Vec<Diagnostic>, cx);  // empty clears
    pub fn clear_source(&mut self, source: DiagnosticSource, cx);
    pub fn for_file(&self, path: &Path) -> Vec<(DiagnosticSource, Diagnostic)>;
    pub fn all(&self) -> impl Iterator<Item = (&str, DiagnosticSource, &Diagnostic)>;
    pub fn counts(&self) -> Counts;                       // footer error/warning totals
}
/// Event: Changed { key, source }.
```

The store is an entity so editors, the drawer and the footer subscribe to one thing; the
per-file key is `path_utils::path_key` (case-folded on Windows). The `Server` source's
inputs are `publishDiagnostics` (full set per file, empty clears, which the generic client
already distinguishes: `ProgressUpdate::FileDiagnostics`).

## 4. Language-specific behaviour

### 4.1 Rust: moves into `pulsar.rust` (or is generalised)

| Behaviour | Today | Target |
|---|---|---|
| Binary discovery, rustup, deps dir, GitHub download | `discovery.rs` (514 lines) | `RustPlugin::locate`, in `plugins/vendor/rust` as it is; the generic host only offers `LocateContext` (progress sink, the engine deps dir, a download helper that verifies a checksum, which does not exist today: the download path `discovery.rs:~380-500` has no hash check, an issue) |
| `PULSAR_RUST_ANALYZER_PATH` | `discovery.rs:62` | a per-server user setting (`language_servers.<id>.path`, Q3) with the env var kept only by the Rust spec's `locate` |
| Init options (clippy, `cargo.buildScripts`, `procMacro`, completion, inlay hints) | `process.rs:535-566` | `initialization_options` of the spec; also answered through `configuration` (rust-analyzer asks for `rust-analyzer` section) |
| Check on save | server-side (`checkOnSave` in init options); the client sends `didSave` (`document_sync.rs:160`) | generic `didSave` stays in the client; the option stays in the spec |
| Progress tokens | `notifications.rs:34-84` title heuristics ("Fetching", "Indexing", "Building" -> wording); `rustAnalyzer/Indexing`, `rustAnalyzer/cargo check` are the real tokens | `ServerHooks::progress` of the Rust spec; the generic client forwards tokens and `end` |
| Indexing status / Ready | `rust-analyzer/serverStatus.quiescent` (`:309`) plus a 3 s / 2 s timer fallback (`progress.rs:31,143`) | `ReadinessRule::Signal` with the hook turning `serverStatus.quiescent` into Ready; timers are deleted (a generic default is "all progress ended", which TS needs; Rust's `quiescent` is exact) |
| `experimental.serverStatusNotification` | `process.rs:~530` | `client_capabilities` |
| Workspace discovery | the project root is the workspace | `RootStrategy::NearestMarker(["Cargo.toml"])` with the project root first; the generated project's root `Cargo.toml` (`core_project_builder.rs:449`) and `scripts/` crates are members of one workspace (`:805`), so one root today. UNVERIFIED for plugin-contributed crates under `plugins/` (section 9) |
| Editor requests: `experimental/parentModule`, `openCargoToml`, `expandMacro`, `reloadWorkspace` | `text_editor/lsp.rs:931,940,980,1030` | `ServerCommandSpec`s of the Rust spec (id, label, params builder, result handler: open location / show text); the code editor renders a "Language" menu from the active server's commands |
| `is_rust`, `"rust"` language id, `.rs` trigger fallback `['.', ':', '\'', '(']` | `text_editor/lsp.rs:53,56,186` | `LanguageRegistry::for_path`, `Language::language_ids`, the server's real triggers (the fallback list is deleted: wait for `initialize`) |
| Status strings ("Rust Analyzer Ready", "rust-analyzer exited ...") | `event_handlers.rs:94-112`, `progress.rs:111-119`; i18n keys `Notification.Title.RustAnalyzerReady` | formatted from `ServerSpec::display_name` (i18n keys renamed, not aliased) |
| Dep on `rustup`/cargo being on PATH | `discovery.rs` | stays, in the plugin |

`pulsar.rust` is a **tool-layer-like editor plugin**: `editor_only = true`, `enabled_by_default
= true`, no `provides`/`requires`. It contributes only a `Language { id: "rust", compiler:
None, server: Some(..), file_extensions: ["rs"], config_files: ["Cargo.toml", "rust-project.json"] }`.
It has no class markers: Rust "classes" are generated into the game crate, not scripts.

### 4.2 TypeScript: what the plugin needs

| Need | Detail |
|---|---|
| Server | `typescript-language-server --stdio` (wraps `tsserver`). Capabilities of interest: completion, hover, definition, references, rename, formatting, `publishDiagnostics` (semantic + syntactic), `workspace/configuration`, `client/registerCapability` (finding 2 blocks it) |
| tsconfig | The class files are not modules: no imports, ambient `ScriptClass`, `Vec3`, namespaces, from the generated `Pulsar/types/pulsar.d.ts` (`typescript/src/compile.rs:204-216`, written only when a project compile runs and a class exists: `:179`). The plugin's `prepare_workspace` writes `Pulsar/tsconfig.json` (or the project's `tsconfig.json` if absent, never overwriting a user's): `{ "compilerOptions": { "target": "ES2022", "lib": ["ES2022"], "types": [], "noLib": false, "strict": true, "noEmit": true, "skipLibCheck": true, "moduleDetection": "legacy" }, "include": ["src/classes/**/class.ts", "Pulsar/types/*.d.ts"] }`. `lib: ["ES2022"]` with **no `DOM`** removes `window`/`document` and the DOM names that would shadow script natives. It also calls `write_declarations` so the `.d.ts` exists before the first open, instead of after the first compile (a latent gap today: the d.ts is absent until Play) |
| Binary acquisition | **Open decision (Q2).** Options below |
| Windows `.cmd` shim | npm installs `typescript-language-server.cmd`; `Command::new("...cmd")` on Windows goes through `cmd.exe` (Rust refuses to run batch files with unescaped args since 1.77.2 without careful quoting). `ServerCommand` for a `.cmd` must be `cmd /c <shim> --stdio` built by the host with `raw_arg` quoting, or (better) `node <path>/lib/cli.mjs --stdio` resolved by `locate`. The host `spawn` helper owns this (one implementation, tested on Windows), not each plugin. No `CREATE_NO_WINDOW` flag today (`process.rs:136`), so a console window may flash for any shim on Windows: add to the helper. UNVERIFIED on this machine |
| `languageId` | `typescript` for `.ts`/`.mts`/`.cts`; the plugin's class files are `.ts` only |
| `check_source` | Section 4.3 |
| Trigger characters, resolve | read from `initialize`, as Rust now does |

Binary acquisition options (for Q2):

| Option | Pro | Con |
|---|---|---|
| A. Require Node + a global/`npx` install, found on PATH | smallest; no download code; user controls version | fails on a clean machine; the first TS experience is "not found" |
| B. A user setting `language_servers.typescript-language-server.path` | explicit, easy for CI | needs A or C anyway |
| C. Download Node-free: ship/download the npm package tarball and a Node runtime into the engine deps dir (checksummed) | works out of the box | large (Node ~30 MB), licensing and update burden, new download code (it also needs `LocateContext::download` with a sha256) |
| D. Use `npx --yes typescript-language-server` | no install step | needs network on first use, slow start, silent supply-chain fetch |

Recommendation: **B + A first** (setting, then PATH/`npm root -g`/`node_modules/.bin`
in the project), with a clear "TypeScript language server not found: install Node and run
`npm i -g typescript typescript-language-server`, or set the path" status message and a
status-bar action; **C later** if users ask. Reason: C is a product decision (bundled
runtime, update policy) that does not block the architecture; A+B exercises everything
else.

### 4.3 Compiler diagnostics as a second source

tsserver accepts what the Pulsar subset rejects (finding 10: imports, arrow functions,
generics, `any`, unions ...), and the Pulsar compiler enforces what the VM verifier needs.
So the Problems drawer and the editor underline need **both**, distinguishable:

- `ScriptLanguage::check_source(ctx, path, text)` runs `pulsar_script_ts::compile_class`
  without writing (the existing `compile_class_dir(..., write: false)` path,
  `typescript/src/compile.rs:60-127`, already returns `CompileDiagnostic { class, file,
  location "line:column", message }`), on a debounce after an edit and on save. The
  `NativeRegistry` it needs is host-provided (`NativeRegistry::with_engine_natives()`, as
  `validate_project` does, `provider.rs:31`), the component events likewise (the
  `ComponentEventMetadata` plumbing in `scripting.rs:112-150`).
- The host converts to `Diagnostic` with `source = "Pulsar"` and publishes under
  `DiagnosticSource::Compiler("typescript")`. The editor draws both; the drawer groups by
  source (the drawer's external-source slots exist, finding 3).
- `location` is `line:column` as a string: `check_source` returns a parsed range
  (`end_line/end_column` exist on `Diagnostic`) so squiggles have extents; for the
  compiler this means spans from oxc, which `pulsar_script_ts::diagnostic` already
  carries (`diagnostic.rs`, UNVERIFIED how complete: section 9).
- Blueprint has no text; its `check_source` default is empty, and Blueprint compile
  errors keep going through `validate_project`.
- Cost bound: compile is on the UI-adjacent debounce path; run it as an editor task
  (`editor_task_queue`, `.agents/EDITOR_TASKS.md`), one in flight per file, latest text wins.

## 5. Target architecture, decisions and open questions

### 5.1 Target in one picture

```
plugins/vendor/rust/        pulsar.rust       Language{ rust,  server: rust-analyzer, compiler: None }
plugins/vendor/typescript/  pulsar.typescript Language{ typescript, compiler: pulsar_script_ts, server: tsls }
plugins/vendor/blueprint_editor  pulsar.blueprints? Language{ blueprint, class_markers:[graph_save.json], compiler }
                │ p.add(Language{..})                                (origin gate: disabled plugin = absent)
                ▼
pulsar_language (kernel, GPUI-free)   LanguageRegistry: for_path, class_languages, is_class_dir,
                │                     one-language-per-class check, headless tools use it
                ▼
pulsar_lsp (kernel, GPUI)             LanguageServers (service) → LanguageServer × (server, root)
                │                     generic client: transport, sync, progress, id spaces
                │                     DiagnosticsStore: file × source → diagnostics
                ▼
consumers: code editor (per-file server_for), shell footer (one entry per server), Problems
drawer (aggregated, by source), AI tools, loading screen, plugin API EditorServices
```

### 5.2 Decisions (Proposed, with a recommendation)

| # | Decision | Recommendation | Why |
|---|---|---|---|
| D1 | One `Language` registration replaces `LinkedScriptLanguage` and `EditorPluginScripting` | yes; delete both (and the TS "no-editor provider") | the user's goal; removes the double registration and `linked_editors.rs` |
| D2 | The registry is a new GPUI-free kernel crate `pulsar_language`, not inside `plugin_editor_api` | yes | `pulsar_class` and `pulsar_package` need it headless; `plugin_editor_api` default features are runtime-safe but `pulsar_class` should not depend on the SDK. The *type* `Language` lives in `plugin_editor_api` only if a plugin needs it there; otherwise define it in `pulsar_language` and re-export from the SDK prelude. Pick: define in `pulsar_language`, SDK re-exports |
| D3 | The generic client keeps the process in the host, one process per (server id, root) | yes | libraries never own a process (section 3.1) |
| D4 | Client ids: client ids even, server request ids never matched against pending; server requests answered by the host (`registerCapability`, `workDoneProgress/create`: Ok(null); `configuration`: the spec's answer; `applyEdit`: refuse with a clear error until the editor supports it, Q8) | yes | finding 2 |
| D5 | Ready = a per-server `ReadinessRule`, no timers | yes | `progress.rs:31,143` timers are heuristics that report Ready wrongly on a slow first index |
| D6 | `DiagnosticsStore` entity: file x source; the drawer is a view of it; the "Diagnostics" aggregate event is deleted | yes | finding 3 |
| D7 | The one-language-per-class check is host-owned (`pulsar_class`), raised at scan | yes | section 2.2 |
| D8 | Rust is `pulsar.rust` with `editor_only = true`; its server starts eagerly (`eager = true`) | yes | keeps today's behaviour; no regression in time-to-first-hover |
| D9 | No fallbacks: if the Rust plugin is disabled there is no server for `.rs`; the editor shows a plain text editor with highlighting | yes | the user's convention |
| D10 | Per-server settings are ordinary **plugin settings** (section 5.4): the host adds a standard group for every language that has a server (`enabled`, `path`, `args`, `initialization_options`, `trace`, `start`); the plugin adds its own (TypeScript: compiler diagnostics on/off) | yes | the user's decision: plugins register settings generally, shown under the Settings window's Plugins section, stored in the engine settings database |
| D11 | Language names are added to the layer gate by reusing `[kinds]` (no script change): one entry per *server/language-specific* term, never the bare word "rust" | yes, see P4 | the script is in the Helio submodule (a Helio PR + re-pin per change); `[kinds]` already scans identifiers and key-like strings in kernel code. "rust" is everywhere in kernel code (the engine is written in it) so a bare term is unusable |
| D12 | `pulsar_script_ts` moves out of `crates/core` into the TS plugin | yes, in P5 | it is a language's compiler; its dependency `oxc` becomes a plugin dependency; `pulsar_package` already reaches languages through the registry |
| D13 | Plugin settings are a general contribution (`PluginSettings`), not language-specific; they use `pulsar-config`'s `ConfigManager` (namespace `plugin`, owner path `[plugin id]`), show in the Settings window's new Plugins section, and carry change listeners | yes | section 5.4 |
| D14 | The server binary is located in order: user setting `path`, project-local (`node_modules/.bin`), `PATH`, then an explicit, user-approved **managed install** (pinned version and sha256, in an engine tools directory, never silent) | yes (AAA, not "later") | rust-analyzer already has a managed download in `discovery.rs`; TypeScript gets the same experience, and the footer entry offers "Install" |
| D15 | `workspace/applyEdit` is supported: a language-neutral `WorkspaceEdit` applier (open buffers and files, one undo group per edit, resource operations) used by rename, code actions and refactors | yes (AAA) | rename and refactor are table stakes next to VS Code and Zed |
| D16 | Servers are keyed by (server, root): several roots (Cargo workspaces, nested tsconfigs) get a server each, found by the spec's `RootStrategy`, started lazily on the first matching file or eagerly when the spec says so; idle servers may stop after a setting-controlled timeout | yes (AAA) | monorepos are the normal case |

### 5.3 Questions, resolved

The user's direction: plugins register settings generally (section 5.4), and every other
decision takes the best solution, not the cheapest. The first draft's defaults were
upgraded accordingly.

| Q | Resolution | Phase |
|---|---|---|
| Q1 | Blueprint's plugin id: verified in P3, `pulsar.blueprints` (root `blueprint_editor_plugin`); it registers its `Language` from that root | P3, done |
| Q2 | D14: settings path, project-local, `PATH`, then a managed, user-approved install | P5 |
| Q3 | User value overrides project value, both in the settings database (section 5.4); CI reads the project scope | P2 |
| Q4 | Rust eager when `Cargo.toml` exists; every other server lazily on its first matching file, or at project open when its `start` setting says `project` | P2 |
| Q5 | Compiler diagnostics on by default; a TypeScript plugin setting turns them off | P5 |
| Q6 | D16: one server per (server, root), not project-root-only | P2 |
| Q7 | Yes, through the `path`, `args` and `initialization_options` settings (D10); no second spec | P2 |
| Q8 | D15: supported | P2 |
| Q9 | TypeScript trait implementation via `TraitImplementerSource`: out of scope here, recorded for later | later |
| Q10 | Run the Node entry point (`cli.mjs`) directly, so no `.cmd` shim and no `cmd.exe` window; the shim is a fallback only when no entry point is found | P5 |

### 5.4 Plugin settings (general, and used by the language registration)

What exists (read from the code): `pulsar-config` (`crates/third-party/pulsar-config`) has a
`ConfigManager` with `register(namespace, owner_path, NamespaceSchema)` callable at runtime,
typed `SchemaEntry`s (label, page, `FieldType`, validators), `get`/`set`/`reset_to_default`,
per-key and global change listeners (`on_change`, `on_any_change`), `search`, `list_*`, and
persistence (`persist.rs`). Built-in schemas are registered by
`pulsar_settings::register_all_settings`. No plugin registers any today, and
`crates/editor/ui_settings` has no reference to plugins, so there is no Plugins section. I
found no `unregister` on the manager (UNVERIFIED; check in P2).

Target:

1. A `PluginSettings { origin, schema }` contribution (a `pulsar_plugins::Contribution`,
   category `plugin.settings`), added from a plugin's root `build` with `p.add(..)`. The host
   registers it under namespace `plugin`, owner `[plugin id]` when the plugin is enabled for
   the project, and withdraws it (or hides it, if the manager cannot unregister: a
   `pulsar-config` change, submodule PR first) when it is disabled.
2. The plugin reads and observes its settings through a handle (`get_bool`, `on_change`), so
   a language server restarts when `path` or `args` change with no host knowledge of what the
   plugin does with the values.
3. The Settings window gets a **Plugins** section: one page per enabled plugin, grouped by the
   schema's `page`, searchable with the rest of Settings, each setting showing its scope (user
   or project) and a reset button, plus a restart-required badge where the schema says so.
4. The host adds the standard server group (D10) to every `Language` that has a `server`, so
   Rust and TypeScript expose the same controls without repeating them.
5. Headless tools (`pulsar package`, CI) read the project scope from the same database without
   the editor.


## 6. Phases

Each phase is built in full; non-blocking problems go to section 8 and are fixed in a
cleanup pass after P6. `just check`, `just arch` and `just clippy` at each phase's end.

### P1: the audit (this document)

Done when this file is reviewed. No code.

### P2: generic client, `LanguageServers`, `DiagnosticsStore`; migrate every consumer

**Status: built** (working tree, nothing committed). As built, where it differs from the
text below:

- **Plugin settings.** `pulsar_settings::plugin::PluginSettings { origin, id, schema }`
  (category `plugin.settings`; `schema` is a `fn() -> NamespaceSchema` because a schema is
  consumed by registration; `id` is `""` for a plugin's main group). Owner path
  `[plugin id]` or `[plugin id, group id]`; an engine crate's page key is
  `engine.<package>`. `sync` registers the enabled plugins' groups and withdraws the
  disabled ones (`engine_state::set_project_path` calls it). `pulsar-config` had no
  unregister: `ConfigManager::unregister` and `OwnerHandle::validate` were added in the
  submodule (listeners and handles survive a withdraw and work again on re-register).
  Scopes and persistence are `engine_state::plugin_settings` (user and project TOML, sparse;
  effective = user, else project, else default). The Settings window has one `Plugin: <name>`
  page per plugin, with a scope selector, a reset (setting the default removes the
  override), the restart-required note, and a page parameter (`SettingsWindow::open_page`).
- **Generic client** (`pulsar_lsp`): `protocol` (framing, `Connection`: client ids and the
  server's request ids are separate spaces), `session` (status, progress, readiness by the
  spec's rule plus a hard cap, no timers), `documents` (refcount, held-back opens before
  `initialize`), `server` (`LanguageServer`: one `spawn_service` thread per server whose log
  is the server's log, stderr in it, the handler answers `registerCapability`,
  `workDoneProgress/create`, `workspace/configuration`, `workspaceFolders`, refresh requests,
  `showDocument`; `applyEdit` goes to the UI thread), `settings` (the standard group, D10),
  `spec` (`LanguageServerSpec` with `find` as well as `locate`, `RootStrategy`
  `ProjectRoot | NearestMarker | OutermostMarker | Custom`, `ServerHooks`,
  `ServerCommandSpec`), `rust_analyzer` (the built-in spec: discovery, init options,
  `serverStatus` -> Ready, progress wording, quick fixes in `data.fixes`, the four extension
  requests as commands), `edit` (`WorkspaceEdit` parse and apply, planned before committed,
  open buffers edited through `BufferEdits` as one undo group per file), `diagnostics`
  (`DiagnosticsStore`; sources `Server(ServerId)` and `Compiler(language)`; `Play` was
  dropped, see section 8), `service` (`LanguageServers`). `AnalyzerStatus` is `ServerStatus`,
  `AnalyzerEvent` is `ServerEvent`, the aggregate `Diagnostics` event is gone.
- **Start policy.** Rust is eager only when the project has a `Cargo.toml`/`rust-project.json`
  (Q4); its root is the outermost `Cargo.toml` under the project (one server per workspace).
- **Consumers.** `EditorServices.language_servers`, the plugin test, the code editor
  (`set_language_servers`, per-file `server_for`, no extension gate, `EditorServerCommand`
  and a "Language" menu from the server's commands, F2 rename through the workspace edit
  applier), the shell footer (one entry per server with a menu: restart, stop, open log,
  trace level, settings link), the Problems drawer (one slot per source, several files at
  once, filter by severity and source), the loading screen (one check per server spec, via
  `find`, which installs nothing).
- **Deleted.** `LspBackend`, `GlobalRustAnalyzerCompletionProvider`, `engine_backend`'s
  re-export module and `pulsar_lsp` dependency (and the `lsp-types`, `ropey`, `flume` deps
  only it used), the splash's dead manager, `new_with_project_and_analyzer`,
  `new_with_shared_analyzer`, `PulsarApp::rust_analyzer()`, wgpui-component's
  `rust_analyzer.rs`.

P2 also delivers, first, the plugin-settings contribution and the Settings window's Plugins
section (section 5.4, D13), because the server settings (D10) depend on it; then the
`WorkspaceEdit` applier (D15) and multi-root keyed servers (D16).

UX target: the footer shows one entry per server (state, progress, root); clicking opens a
popover with restart, stop, open log, trace level and a link to that plugin's settings. The
Problems drawer groups by file, filters by source (server, compiler) and severity, and
updates live. Editors take every LSP result (squiggles, hover, completion, code actions,
rename, symbols) from the aggregated store and per-file servers.

Rust is still *in core* this phase (the spec is a Rust value in `pulsar_lsp` behind the
same interfaces so P4 only moves it).

1. Split `pulsar_lsp` into the generic `LanguageServer` (framing, id spaces, requests,
   documents, progress forwarding, status) and a built-in `rust_analyzer` spec
   (`discovery.rs`, init options, `serverStatus`, the progress mapping). Fix finding 2
   (id spaces, answer server requests), finding 4 (`uri_to_path`), and drop the
   timer-based readiness (D5). Rename `AnalyzerStatus`/`AnalyzerEvent`.
2. `LanguageServers` with `server_for(path)` by extension (a small stand-in lookup the
   registry replaces in P3: *temporary and listed*, so P3 deletes it: the one place this
   plan tolerates a stage).
3. `DiagnosticsStore`; migrate `analyzer_diagnostics.rs`, the drawer feed, footer counts,
   the editor's per-file application, the AI diagnostics tool.
4. Migrate `EditorServices.language_server` -> `language_servers`, the plugin test, the
   code editor (`set_language_servers`, per-file server, extension requests behind
   `ServerCommandSpec`), the shell footer/stop/restart, the loading screen.
5. Delete: `LspBackend`, `GlobalRustAnalyzerCompletionProvider`, `engine_backend`'s
   re-export module and `pulsar_lsp` dep (`editor-ui` feature), the splash's dead manager,
   `new_with_project_and_analyzer`, `new_with_shared_analyzer`, `rust_analyzer()`,
   the `wgpui-component` `rust_analyzer.rs` (submodule PR first).
6. Tests: id-collision regression, server-request replies (a scripted fake server over a
   pipe), store clear-on-empty, store clear-on-stop, two-source merge, per-file routing,
   the Problems drawer shows two files at once (finding 3).

Exit: nothing outside `pulsar_lsp` names `RustAnalyzerManager`; `grep -rn rust_analyzer`
in `crates/editor` and `plugins/vendor/code_editor` finds no *behavioural* use.

### P3: the `Language` contribution and registry

**Status: built** (working tree, nothing committed; the P2 work is under it). As built, where it differs
from the text below:

- **`pulsar_language`** (`crates/core/pulsar_language`, GPUI-free, kernel layer, D2): `Language
  { origin, id, display_name, file_extensions, language_ids, highlight_id, class_markers,
  config_files, compiler: Option<fn() -> Arc<dyn ScriptLanguage>>, server: Option<&'static
  LanguageServerSpec> }` (category `language`), `LanguageRegistry` (`current()` caches an `Arc`
  snapshot against `pulsar_plugins::generation()`; `for_path` by longest extension, `class_languages`,
  `is_class_dir`, `class_markers`, `language_of_marker`, `compilers`, `with_servers`,
  `server_for_path`; a duplicate id is ignored and logged). The `server` is a `&'static` spec (a plugin
  keeps it in a `static`), not the sketch's `fn() -> LanguageServerSpec`: no allocation, and the P2
  servers already held `&'static` specs. The types it needs moved here from other crates, so the
  registration can carry them without the registry depending on GPUI: `ScriptLanguage`,
  `CompileDiagnostic`, `CookOptimization`, `ComponentEventMetadata` (from `plugin_editor_api`, which
  re-exports them for plugin authors) and the P2 spec types and `ServerSettings` (from `pulsar_lsp`).
  `ScriptLanguage` lost `id()` and `display_name()` (the registration owns them) and gained
  `compile_target(class_dir)` (row 6). The spec lost `origin`, `file_extensions`, `language_ids`,
  `language_id` and `config_files` (the language owns them), and its `diagnostic: fn(&mut ui
  Diagnostic)` hook became `quick_fixes: fn(&Value, &str) -> Vec<QuickFix>` (the foundation `Diagnostic`
  is not GPUI-free; `pulsar_lsp` converts). `submit_test_language!` registers a marker-only engine
  language for a kernel crate's tests.
- **One registration, one path.** Deleted: `LinkedScriptLanguage`, `linked_script_languages`,
  `EditorPluginScripting` and its `export_plugin!(T, scripting)` arm (the `export_plugin!` macro
  has one form; `EditorPluginFull` lost the bound), `BuiltinEditorProvider::script_languages`,
  `BuiltinEditorRegistry::get_all_script_languages`, `PluginManager::script_languages` (pre-Play
  validation reads `LanguageRegistry::compilers`), the TypeScript "no-editor provider" and with it
  that plugin's `plugin_manager`, `gpui-ce` and `ui` dependencies, the P2 stand-in
  (`known_specs`, `active_specs`, `spec_for`, the spec's extension fields) and
  `linked_editors.rs`'s "editor and headless agree" test (nothing left to keep equal). A DLL plugin that
  brings a language exports its root's `build` (`export_library!`), the one path; the draft's
  `EditorPlugin::languages()` was not added (it would be a second).
- **Plugins.** Q1 verified: Blueprints is `pulsar.blueprints`, root `blueprint_editor_plugin`; its `build`
  registers the `Language` unconditionally (it was inside the `builtin` feature before). TypeScript
  is `pulsar.typescript`, root `plugin_typescript`. Each `Language` is one `const` in the root's
  `lib.rs` (`blueprint_editor_plugin::LANGUAGE`, `plugin_typescript::LANGUAGE`).
- **`pulsar_class::language`**: `ClassProblem::{MultipleLanguages, ModuleFromOtherLanguage}`,
  `language_of(dir)`, `is_owned_by(dir, id)`, `check_owned(dir, id)`, `class_problems(root)`;
  `ClassRegistry::problems()` is filled at scan. `is_class_dir` asks the registry for markers.
  A language's plugin enumerates its classes through `is_owned_by` (it is handed only classes it
  owns) and the Blueprint editor's single-class compile makes one `check_owned` call (a host
  function; no plugin names the other). Both plugins' `check_language` are deleted. The legacy
  module migration attributes an unlabelled module to the one language that owns the class
  (nothing is moved when none does).
- **Reporting.** The packager fails the build on every class problem and builds its "has source
  but no compiled module" message from the registry (marker and language name); pre-Play
  validation (`PluginManager::validate_scripts`) lists them; the Problems drawer shows them under
  the source `Classes` (`ui_core::app::class_problems`: scanned on a worker at project open, on a
  plugin-set change and every 30 s).
- **Consumers of `class_markers`**: `pulsar_class::registry::is_class_dir`, the file manager tree
  (`ui_file_manager::utils::tree`), `ui_common::file_utils` (`FileType::Class(language name)`,
  `FileInfo::class_language`), the viewport placement check (`is_class_dir`), the packager,
  `pulsar_traits::class_name_of_asset` (row 12, used by the World Settings label, row 13) and
  `pulsar_project::model` (row 6: `rust_classes` asks the class's language for
  `ScriptLanguage::compile_target`; Blueprints reads its `compile_target` key). `ui_core`'s
  `file_utils.rs` was an orphan copy (the module is `ui_common`'s re-export) and was deleted.
  `ui_problems` takes the preview grammar from `Language::highlight_id` and keeps a small table
  for the languages with no plugin. `class.ts` and `graph_save.json` are gone from production
  code in `crates/core` and `crates/editor`; they remain in tests and in the two plugins.
- **Settings.** The standard server group is derived from the registrations:
  `pulsar_settings::plugin::SettingsGroups` (a new registration of derived groups; `sync` and
  `sync_groups` register and withdraw each under its `Language`'s origin) and `pulsar_language`
  submits one that lists every `Language` with a server. The plugin adds nothing; disabling it
  withdraws the group. `ServerSettings::read(language.origin, spec.id)` is unchanged in meaning.
- **Disable semantics.** The registry, `LanguageServers` (`server_spec_for` asks it; `drop_disabled`
  checks the language's origin) and the settings groups all follow the plugin gate, so a disabled
  plugin's language, markers and server are gone.
- **Tests.** `plugins/installed/tests/languages.rs` (real plugins and a dummy third language,
  one-language rule, neither plugin compiles a class that is two languages', disabling removes
  markers/language/server/settings group), `pulsar_class::language` (two markers, module of another
  language), `pulsar_language::registry`, `pulsar_settings::plugin::derived_tests`, the TypeScript
  plugin's tests (a class another language owns is not compiled and its artifacts survive), and kernel
  crates' tests that need a class marker register a test language (`submit_test_language!`).
- **Not done (issues log):** a TypeScript class folder is not a file type of the Blueprint
  editor's class type (plan section 2 row 5: there is no editor to open it with, so deriving a
  folder file type from `class_markers` would claim directories for the wrong editor); the code
  editor's per-extension file types (row 17) still name Rust and TypeScript, and the highlighter
  table stays foundation (row 16); the file manager tree still lists the other kinds' marker
  files (`struct.json`, ...); `asset_kind` `blueprint` (row 14).

Original scope, for reference:

`pulsar_language` crate; `Language` registration; the Blueprint and TypeScript plugins
register one each (replacing `LinkedScriptLanguage` + provider); `plugin_manager`,
`pulsar_package`, `linked_editors.rs` updated; `class_markers` consumed by `pulsar_class::is_class_dir`,
the file manager tree, `ui_common`/`ui_core` file utils, the viewport asset check, the
packager messages; the one-language-per-class check moves to `pulsar_class` and is deleted from
both plugins; `class.ts`/`graph_save.json` literals leave `crates/core` and `crates/editor`
except the Blueprint plugin's and the TS plugin's own. Delete the P2 stand-in lookup.
Tests: a class with two markers is one `MultipleLanguages` problem; a third dummy
language needs no change in either plugin; disabling a plugin removes its markers from
`is_class_dir`.

### P4: Rust becomes `plugins/vendor/rust`; language names in the arch gate

**Status: built** (working tree, nothing committed; P2 and P3 are under it). As built, where it
differs from the text below:

- **The plugin.** `plugins/vendor/rust`, id `pulsar.rust`, root crate `rust_plugin`
  (`editor_only = true`, `enabled_by_default = true`, no `provides`/`requires`). Its `build` adds
  one `Language` (`rust_plugin::LANGUAGE`: id `rust`, extension `rs`, `config_files`
  `Cargo.toml`/`rust-project.json`, no compiler, no class markers, `server: Some(&SPEC)`) and one
  `SettingsMove` (below). `src/server.rs` is the spec moved out of `pulsar_lsp` (`find`/`locate`,
  initialization options, `configuration`, the `serverStatus` -> Ready hook, the progress wording,
  quick fixes from `data.fixes` and related information, the four commands, `eager`,
  `ReadinessRule::Signal`, a 20 minute cap); `src/discovery.rs` is the binary discovery and install
  (`PULSAR_RUST_ANALYZER_PATH`, rustup, `~/.cargo/bin`, the engine deps dir, the GitHub release). It
  depends on `pulsar_language`, `pulsar_plugins`, `pulsar_settings` and `pulsar_lsp` (for the LSP
  edit and URI helpers the quick fixes use). Nothing else was needed in `plugin_editor_api`.
- **Wiring.** A plain directory in the tree, like `plugins/vendor/typescript` and `trait_editor`
  (not a submodule yet, issue 26): a member and a workspace dependency in the root `Cargo.toml`, and the
  generated block of `plugins/installed` (`UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins
  --test installed_set`: `rust_plugin` optional, behind `editor`, re-exported). A default editor build
  links it and a project that lists no plugins runs it. `architecture/std-fs-allowed.txt` follows
  `discovery.rs` to its new path.
- **`pulsar_lsp` names no server.** `src/rust_analyzer/` is gone (`mod.rs` is `server.rs` of the
  plugin, `discovery.rs` moved), with `fixes_in_data` and the `inventory` dependency; the crate's
  remaining tests that need a language register a test language with a server under an engine origin
  (`service.rs` tests: routing by `(server, root)`, stop clears diagnostics and only its own, the stop
  reason), and the generic tests' sample strings no longer say rust. Every consumer already reached the
  analyzer through the registry (P2/P3): a grep of `crates`, `plugins` and the workflows finds no
  `rust_analyzer`/`rust-analyzer` outside the plugin and the plan, one dummy server id in UI tests
  (`ui_core`, `ui_problems`: plain strings) and the two reworded comments (`core_project_builder`,
  `pulsar_project::apply`: "the language servers").
- **Settings migration (issue 11).** The group's owner path was `engine.pulsar_lsp/rust-analyzer` (P2)
  and is `pulsar.rust/rust-analyzer` now (the group follows its `Language`'s origin). A new
  registration, `pulsar_settings::plugin::SettingsMove { origin, id, from }` (category
  `plugin.settings.move`), lets a plugin say "this group's saved values lived under `from`";
  `engine_state::plugin_settings::activate` renames the user file and the project file when the old
  exists and the new does not (the new one wins and the old is left when both exist). The Rust plugin
  adds one. The keys are unchanged (the schema is the standard group), so every user value survives.
  Before P2 there were no Rust settings keys at all, so nothing older needs a path.
- **The gate.** `[kinds]` gained `typescript = [typescript, tsserver, tsconfig, class ts, oxc]`,
  `rust-analyzer = [rust analyzer, rustup, rust project]`, `blueprint-lang = [graph save]` and
  `rust-target = [rust class, rust classes, rust script, rust target]`. The scanner is not run by
  python here (issue 27): its `kind_names` / `checked_files` / `strip_comments` / `words` were ported
  to a throwaway script and first reproduced the recorded state exactly (975 files, 21 hits, 0 new, 0
  gone); each term was then run alone against the tree. Hits: `typescript`, `tsserver`, `tsconfig`,
  `class ts`, `rust analyzer`, `rustup`, `rust project`, `graph save` none, except `oxc` in
  `pulsar_script_ts/src/lower.rs`; `rust-target` in `pulsar_project/src/model.rs` and
  `ui_level_editor/src/core/native_scripts.rs`. Those three are recorded in
  `architecture/kind-names.txt` with their reasons (the TypeScript compiler waits on P5; the
  compile-to-Rust target waits on P6); the rest of the tree is clean (24 hits, 24 recorded). Terms tried
  and **not** added: `clippy` and `rustc` (an attribute and toolchain words in 60 kernel files, not a
  server's names), `cargo`, `cargo toml` (the engine builds the game with cargo:
  `core_project_builder`, `pulsar_project`, `ui_build`), `blueprint` (about 60 kernel files: the
  engine's node vocabulary, `pulsar_std`, `#[blueprint]`, not the language plugin's name),
  `node modules`, `ts`/`rs` (extensions in file names), the bare `rust`.
- **Tests.** `plugins/installed/tests/languages.rs`: Rust is a default plugin (enabled by a project
  that lists none) whose registry finds `rust` for `.rs` with the rust-analyzer spec and the settings
  move; with `pulsar.rust = false` the registry has no language or server for `.rs`, the loading
  screen's list (`active_servers`) lacks it, the move is not applied, TypeScript is untouched; a GPUI
  test runs the eager Rust server (configured with a path that does not exist, so nothing is installed or
  spawned), puts a diagnostic of its own and one of another source in the store, disables the plugin and
  lets the service tick: the server and so its status entry are gone (`Removed` is emitted), only the
  server's diagnostics are cleared and the file is announced, the group is withdrawn from the database,
  `server_for` gives none; the server-group test now also withdraws and restores
  `pulsar.rust/rust-analyzer` with the plugin. The plugin's own tests: the language, the registry,
  the descriptor, and the spec tests that moved with the code (wording, `serverStatus`,
  configuration sections, overlay, commands, macro expansion, related-information removals, the
  `data.fixes` test moved from `pulsar_lsp`). `engine_state`: the move keeps the values and never
  overwrites. Nothing was deleted to pass.
- **Not done / issues:** the repository (26), `just arch` (27), the plugin's `pulsar_lsp` edge (28),
  the settings move's limits (29), no `blueprint` term (30). The download checksum (issue 7) is
  unchanged and moved with the code.

Original scope, for reference:

- New submodule `plugins/vendor/rust` (`Plugin_Rust`; id `pulsar.rust`, `editor_only`,
  `enabled_by_default = true`): `pulsar-plugin.toml`, root crate `rust_plugin`, spec moved
  from `pulsar_lsp` (discovery, options, progress hook, commands). Submodule merge order:
  plugin repository first, re-pin, then the Pulsar PR (`.agents/CRATES.md`).
- Delete the Rust spec from `pulsar_lsp`; `pulsar_lsp` names no server.
- `just arch`: add language terms to `[kinds]` in `architecture/layers.toml`
  (`typescript = ["typescript", "tsserver"]`, `rust-analyzer = ["rust analyzer",
  "rustup"]`, `blueprint-lang = ["graph save"]` ...: identifier words, not bare "rust"),
  and record what kernel code still names in `architecture/kind-names.txt` (shrink-only,
  each with its reason), including the ones this plan leaves (#14 `asset_kind`,
  `core_project_builder`'s Rust generation). Verify the scanner against the current tree
  before adding a term (it matches identifiers and key-like strings, comments stripped).
- Test in `plugins/installed/tests`: with `pulsar.rust = false` in a project, no server for
  `.rs`, diagnostics cleared (section 3.3), the status entry absent.

### P5: TypeScript server, tsconfig, live compile diagnostics

**Status: built** (working tree, nothing committed; P2 to P4 are under it). As built, where it differs
from the text below:

- **The server.** `plugin_typescript::LANGUAGE.server` is `server::SPEC` (id `typescript-language-server`).
  `locate.rs` finds it in D14's order: the `path` setting (used or reported, never replaced), the project's
  `node_modules` (`.bin` shim or the package), `PATH`, the managed install. It is launched as
  `node <package>/lib/cli.mjs --stdio` (Q10): a shim found anywhere is resolved to its entry point through
  `package.json`'s `bin` (Windows prefix layout, project `.bin`, Unix `lib/node_modules`, or a link), Node is
  taken from beside it, then `PATH`, then the usual install places, and its major version is checked (18 or newer).
  Only when no entry point is found is the shim run itself (`Command` on a `.cmd` goes through `cmd.exe`, no console
  window: `CREATE_NO_WINDOW` is set for every server). Issue 9 is closed by this.
- **Missing is a status.** `LocateError` became `{ message, missing }` (`LocateError::new` / `::missing`), and
  `ServerStatus::Missing(String)` is its state (with an error event for the toast). No Node, a Node that is too old,
  no server, and a configured path that does not exist each say what is missing and what to do. The Rust spec is
  unchanged but for the constructor.
- **The managed install (D14).** `LanguageServerSpec.installer: Option<&'static ServerInstaller>
  { label, describe, install }`. The footer offers it only on a `Missing` server, with `describe()` (what is
  downloaded, from where, where it goes) above the item, and runs it only when the person chooses it
  (`LanguageServers::install` -> `LanguageServer::install`, which submits an editor task "Install ...", category
  `language-server`, long, its progress shown in the footer and its log in the Tasks window) and then starts the
  server; a failure is `Missing` again ("The install of ... failed: ...") so it is offered again. `install.rs`
  pins typescript-language-server 4.4.0 and typescript 5.9.3 (npm tarballs), downloads each (system `curl`,
  PowerShell on Windows as the second try), checks its digest **before** unpacking, unpacks with a small safe tar
  reader (`archive.rs`: files and folders only, no `..`/absolute/link entries) into `<staging>`, moves the whole
  folder into `pulsar_install_layout::tools_dir()/typescript-language-server/<release>` (new function) and leaves
  nothing behind on any failure. The layout is npm's own, so the server finds TypeScript as its sibling package. The
  pins are npm's published **sha512** `dist.integrity` (as hex), not sha256: a sha256 of the tarballs needs the
  tarballs downloaded, which was not done here (issue 31). `Digest` takes either.
- **rust-analyzer's download (issue 7).** Fixed: it asked for an asset name no release has (`rust-analyzer-<triple>[.exe]`;
  releases publish `.gz`, and `.zip` on Windows) and checked nothing. It now reads the latest release's JSON,
  takes the asset's published `digest` (`sha256:`; an asset with none is refused), verifies the download against
  it, then gunzips (or `Expand-Archive`s) it. The digest comes from the same origin as the file, so it guards against
  corruption and a swapped asset, not against a compromised release (a pin would; rust-analyzer moves too fast for one).
- **Workspace and roots.** `RootStrategy::Custom`: a file under `<project>/src/classes` has that folder as its root
  (one server for all classes), any other `.ts` the nearest `tsconfig.json`/`jsconfig.json`, else the project.
  `prepare_workspace` (only for the class folder) writes `Pulsar/types/pulsar.d.ts` (the first-open gap is closed) and
  `src/classes/tsconfig.json` (`ES2022`, no DOM, `types: []`, strict, `moduleDetection: legacy`, includes
  `**/class.ts` and `../../Pulsar/types/*.d.ts`). The tsconfig is not at the project root (the plan's first guess): the
  root's would be the person's, and the nearest tsconfig is the one the server uses for a class file. Its first line
  marks it generated; it is rewritten while the line is there and never touched once removed.
  Init options: `hostInfo`, no automatic typings acquisition, completion preferences; `workspace/configuration` is
  answered from them. Readiness is `AllProgressEnded` (the server's `Initializing JS/TS language features` work, shown
  as "Loading the TypeScript project..."), with a 45 s cap.
- **Live compiler diagnostics (4.3).** `ScriptLanguage::check_source(&CheckContext { project_root, natives }, path,
  text)` and `live_check_enabled()` (both default off/true-empty). The host side is `LanguageServers::check_source` /
  `forget_source` / `has_compiler_for` (`pulsar_lsp/src/service/checks.rs`): 400 ms debounce per file, the check is an
  editor task ("Check TypeScript: class.ts", category `language-check`), one in flight per file, the latest text wins, the
  result goes to the store as `DiagnosticSource::Compiler(language id)` with source `Pulsar`, the range reaching the end
  of the word at the compiler's `line:column`. The service's tick keeps it true: a language whose plugin is disabled loses
  its entries and files, a language whose `live_check_enabled` turned false is cleared (the latest text is still kept)
  and checked again when it is true. The code editor calls it on open, edit and save and `forget_source` on close
  (`TextEditor::lsp_check`). TypeScript's check runs only for a `class.ts` the language owns, through
  `check_class_source` (no disk reads of the class but its schema, no writes). The plugin's setting
  `compiler_diagnostics` (`PluginSettings`, main group, default on; read through the plugin-settings handle) is Q5.
- **`pulsar_script_ts` moved (D12)** to `plugins/vendor/typescript/compiler` (same package name, a workspace member, `oxc`
  is now that package's dependency in the plugin layer). Nothing in the kernel depended on it (`pulsar_package` reaches
  languages through the registry); its tests moved with it (`blueprint_parity` reads the conformance fixture by path;
  `just check-typescript-declarations` is unchanged). `pulsar_package/tests/typescript_compile.rs` stays: that crate is a
  host (`layers.toml`), which may link plugins. The stale `oxc` line of `kind-names.txt` is gone (the `oxc` term stays in
  `[kinds]`: kernel code may not name the parser).
- **One footer entry (the user's later direction).** `language_server_footer.rs` is one entry, "LSP Servers": the
  aggregate (worst health: error, then missing, then working, ready, idle; a count; the furthest progress) and a dropdown
  with a row per server (name, root, state), each a submenu: state, detail and root, then for a missing server what
  an "Install ..." item that is an approval step (a submenu saying what `describe()` says, with "Download and install" and
  "Cancel": one click never downloads), "Set executable path..." (a file dialog; the choice is saved as the person's `path`
  setting and the server restarted) and "Check again"; Restart/Stop or Start; Open Log; Trace; Settings. Every menu has
  a 400 px maximum width and every text is wrapped to 52 columns (`wrap_text`, tested), after the first version
  rendered the long "not found" and install texts as single lines far past the menu's edge. It is built from `LanguageServers` alone (`aggregate`, `rows`, `server_menu_lines` are
  plain data functions, tested), so a plugin's server appears with no UI. The error toast ends with "Open LSP Servers in
  the status bar for restart, install and settings."
- **TypeScript classes in the file manager (issue 5/17).** Done as a host rule, not a file type of the Blueprint editor:
  a class folder of a language with no editor of its own for the folder is one item (`FileItem::class_language`,
  type column "<language> Class"), not a folder, and opening it (`PulsarApp::open_path`) opens the first of the language's
  class markers that an editor claims (`class.ts` in the code editor). A Blueprint class has its own editor for the
  folder and is left alone.
- **Also.** `LanguageServer::attach(reader, writer)` runs the same conversation over given streams (no locate, no spawn;
  the workspace is still prepared), and `pulsar_lsp::testing::scripted_server` is a server scripted on an OS pipe.
  A bug found by the install test and fixed: the install pump busy-looped once its progress channel closed.
- **Tests.** `plugin_typescript` unit tests (locating in each order, the Windows prefix layout, shim fallback, missing
  Node / old Node / missing server, the install: verified, mismatch refused and nothing left behind, failed download,
  replaced whole, pins well formed, the tar reader's refusals, tsconfig and roots, options, progress wording) and
  `tests/language_server.rs` (a scripted tsserver over a pipe: a `.ts` class's diagnostics under the server's source with
  the workspace prepared and the handshake checked; accepted-by-server/rejected-by-compiler shows only the `Pulsar` source;
  both sources of one file; a missing server's status and offered install; **real Node smoke tests, run on this
  machine**: `node cli.mjs --stdio` over real pipes with a scripted JavaScript server, and on Windows the `.cmd` fallback; they skip
  themselves without Node and CI does not need it; the real typescript-language-server was **not** run: it is not installed
  here and was not installed for this). `plugins/installed/tests/typescript_language.rs`: disabling `pulsar.typescript`
  clears both sources, the server, its status entry and its settings group; the plugin setting turns the compiler source off and
  on. `pulsar_lsp`: the check service (burst is one check of the latest text, an edit during a check, setting off and
  on), the install flow (starting never installs; a failed install is offered again). `ui_core`: the footer data
  functions and the class-folder open rule. `ui_file_manager`: a class folder is one item. `rust_plugin`: asset and
  digest selection, mismatch refused.
- **Not done / issues:** 31 to 36.

TS `Language` gains `server` (locate per Q2, args, init options, `prepare_workspace`
writing tsconfig + `.d.ts`), the `.cmd` spawn helper in the host, the progress hook (tsserver
reports `Initializing JS/TS language features` through work-done progress), the
`ScriptLanguage::check_source` hook and its host debounce task, `pulsar_script_ts` moved
into `plugins/vendor/typescript`. Tests: a `.ts` class shows tsserver diagnostics in the
editor and the drawer; a construct tsserver accepts and the compiler rejects shows only
the `Pulsar` source error; disabling `pulsar.typescript` clears both sources; the shim spawn
on Windows (CI Windows job: `.agents/CI.md`).

### P6: language-neutral `ModuleBuilder`; docs and skill

**Status: built** (working trees, nothing committed; P2 to P5 are under it). As built:

- **Placement (the P6 review question).** `pulsar_script_vm::module_builder`, not a sibling
  `pulsar_script_builder`. The builder is the module format's other half (a format change and the
  builder's rules change in one commit), it needs only `Module`, `verify`, `NativeRegistry` and
  `TypeRegistry`, and a language plugin already depends on `pulsar_script_vm`. A sibling crate would be a
  one-to-one dependency of the VM with a version to keep in step, a new workspace member and nothing a
  plugin could not already reach. It has no syntax, no GPUI and no plugin in it, so the layer gate has
  nothing to say about it (the kind scanner port: 973 files, 23 hits, 0 new; the VM gained no dependency).
- **The API** (`.agents/SCRIPT_LANGUAGES.md` is the reference): `ModuleBuilder` (constant pool and import
  de-duplication, `declare_function` reserving an index before bodies, variables with a stable `id`,
  `set_class_version`, events, subscriptions, native calls resolved and argument-checked against the registry,
  `load_var`/`store_var`/`call`/`call_native`/`load_constant`, `finish()` which verifies), `FunctionBuilder`
  (typed registers, `Label`s with forward patching, a loop stack, scopes of named locals, `SourceLoc` debug
  info, `unreachable`), `binary_result`/`unary_result` (the verifier's operand rules), `BuildError`.
  `SourceLoc::at` was added.
- **The TypeScript lowering is a client.** `lower.rs` lost its own register, label, loop, scope, constant,
  import, variable and debug code (`Fx` is a `FunctionBuilder` plus `is_async`; `if`/`while`/`for`/`&&`/`||`/`?:`
  use labels instead of patching pcs; calls, constants, variable access and the binary result rules go through
  the builder; the old verify step is `finish()`). A builder error is a diagnostic (too many registers keeps its
  text; any other is "internal error: ..."), and a half-lowered method's builder error is dropped when the
  method already has errors. One ordering change: fields are added to the builder after the schema reconciles
  (so each carries its id) instead of while they are read; no output depends on it.
- **No behaviour change, pinned.** `plugins/vendor/typescript/compiler/tests/golden.rs` was recorded
  **before** the refactor against `tests/golden/*.module.txt`: a fixture that uses every construct the lowering
  emits (`Constructs.ts`), the four examples and the Beacon class, each as JSON and as the shipped binary
  encoding, with the new-field ids normalised (they are fresh uuids). They pass unchanged after it, as do the
  conformance, compile, migration, blueprint parity and tsc tests, the plugin's tests, `typescript_compile.rs`
  and the installed `typescript_language`/`languages` tests.
- **A second language.** `crates/core/pulsar_script_vm/tests/module_builder.rs`: an s-expression language (parser,
  typing, `if`, a `sum` loop, calls to functions declared later) compiled through `ModuleBuilder`, verified and run
  on the VM, carrying debug info through the binary format, with its own mistakes reported by the front end and the
  builder's arity check. 19 builder unit tests in `module_builder.rs`, and a doc test.
- **Docs and skill.** `.agents/SCRIPT_LANGUAGES.md`, `.claude/commands/add-language.md`, rows in `AGENTS.md`
  (index and skills), `CRATES.md` and `PLUGIN_SYSTEM.md`.

Extract a `ModuleBuilder` from `crates/core/pulsar_script_ts/src/lower.rs` (2,092 lines;
`Cx` `:116`, `lower_method` `:756`, `Target` `:2018`: emission of functions, locals,
loops, waits, debug info, field identity) into a crate a *second* text language can use
(candidates: keep it in `pulsar_script_vm` as `module_builder`, or a sibling
`pulsar_script_builder`; Q for review in P6). The TS lowering becomes a client of it with
no behaviour change (the conformance suite, `just conformance`, and
`typescript_compile.rs` are the oracle). Then write `.agents/SCRIPT_LANGUAGES.md` (as built)
and `.claude/commands/add-language.md` (a skill beside `/add-plugin`), update
`.agents/CRATES.md`, `.agents/PLUGIN_SYSTEM.md` and AGENTS.md rows.

### After P6: the cleanup pass

Everything in section 8, in order of severity. The open items now (rows marked "no" or "verify by running"),
most severe first:

1. **16 and 33** (medium). The footer, Settings page, Problems menus, the file manager's class item and
   opening a class folder compile and their data is tested; nobody rendered or clicked them. Needs a run.
2. **32** (medium). The real typescript-language-server was never run against a class (progress titles,
   how it reads the generated tsconfig, whether classes type-check clean against `pulsar.d.ts`).
3. **31** (medium). The managed install pins npm's sha512, not a sha256 of the tarballs, and the bundled-single-file
   assumption for typescript-language-server 4.4.0 was not confirmed by unpacking.
4. **13, 23, 27** (medium). `just arch` was never run with python (the node port of the scanner is clean:
   973 files, 23 hits, all recorded); rerun the real gate.
5. **1, 8, 20** (medium). A DLL plugin gets no plugin-settings handle; resource operations of a `WorkspaceEdit` do not
   tell an editor that has the file open; a class of a language that is not linked or enabled drops out silently.
6. **37** (medium for a second text language). `ClassSchema` (stable field ids and the class version) is the
   TypeScript compiler's; a second text language reimplements it. Move the reconcile logic next to `ModuleBuilder`
   when the second language is written.
7. **26** (user action). `plugins/vendor/rust` is not yet its own repository.
8. Low: **2, 3, 4, 10, 15, 18, 19, 21, 22, 25, 28, 29, 30, 34, 35, 36, 38, 39, 40**.

Fixed in the P6 pass: none of section 8's open rows was small and safe without a GUI, a network, another
repository or a design decision, so none was changed.

## 7. Risks

| Risk | Mitigation |
|---|---|
| The generic client regresses rust-analyzer (the readiness rule, the fix to server-request handling) | P2 keeps Rust in core and behind the same UI; a fake-server test suite before any consumer moves; `serverStatus` readiness is the same signal as today |
| Removing the 3 s/2 s Ready timers leaves a server that never reports end tokens permanently "Indexing" | `ReadinessRule` per spec; a hard cap (`max_indexing`, then Ready with a warning in the log) in the host, not a heuristic |
| DLL plugin ABI: `Language` has `fn` pointers and `Arc<dyn>` | layout fingerprint covers type name/size/alignment, so bump `VERSION` on every field change; first-party languages are linked statically anyway |
| A server process outliving its plugin, orphaned on crash | host owns processes; kill on drop and on `Stopped`; the Tasks window lists servers as services (`.agents/EDITOR_TASKS.md`) with stall diagnosis |
| Windows `.cmd` and `CREATE_NO_WINDOW`; paths with `%`, drive-letter case | one spawn helper and the existing `path_utils` with tests on Windows; `uri_to_path` fix (finding 4) |
| tsserver on a project with no Node | explicit, actionable "not found" status (Q2); never a silent no-op |
| Submodule chain: `wgpui-component` (dead-code delete), `plugins/vendor/rust` (new), TS plugin, Pulsar | dependency-first merges per `.agents/CRATES.md`; `just check-submodule-pins` |
| `pulsar_script_ts` move breaks `pulsar_package`/conformance linkage | the packager already reaches languages through the registry; `engine/tests/linked_editors.rs` and `typescript_compile.rs` pin it |
| The new `[kinds]` terms flag legitimate kernel mentions | add terms one at a time against `just arch` output; record the unavoidable ones in `kind-names.txt` with a reason, never widen a term to hide a hit |
| Two diagnostics sources double-report one error | `DiagnosticSource` is shown in the drawer and the hover; no de-duplication across sources (different sources, different meaning) |
| Quick-fix round trips per diagnostic (one LSP request each, `analyzer_diagnostics.rs:91-110`) multiply with two servers | keep the lazy fetch; fetch only for the server that owns the diagnostic |
| `ModuleBuilder` extraction is a 2,000-line refactor with no second consumer to validate the seam | P6 last, oracle = conformance + TS compile tests; if the seam is not obvious, ship P1-P5 and leave the extraction as a separate plan |

## 8. Issues log

Format: `P<n>: <what>, <where>, <severity>, <resolved in cleanup pass: yes/no>`.

| # | Phase | Issue | Where | Severity | Resolved |
|---|---|---|---|---|---|
| 1 | P2 | A DLL plugin gets no `PluginSettingsHandle`: the handle resolves the host database through a static bound by `register_all_settings`, and a plugin library has its own copy of that static. The host must hand the handle over (`EditorServices`) | `pulsar_settings/src/plugin.rs` (`bind`, `handle`) | medium | no |
| 2 | P2 | `LanguageServerSpec::config_files` only gates the eager start; nothing watches `Cargo.toml`/`tsconfig.json` to restart or reconfigure a server (rust-analyzer watches its own) | `pulsar_lsp/src/service.rs` (`project_has_config`) | low | no |
| 3 | P2 | Play (script) problems have a class and function, not a file, so they stay a drawer-direct source (`set_source_diagnostics`) and `DiagnosticSource::Play` was dropped; the footer counts read the drawer (the union), so `DiagnosticsStore::counts` has no consumer yet; nothing publishes the `Compiler` source until P5 | `ui_core/src/app/script_problems.rs`, `pulsar_lsp/src/diagnostics.rs` | low | no |
| 4 | P2 | "Open Log" opens the Tasks window; it cannot preselect the server's task (no API). The server's log is its service task's log (stderr included) | `ui_core/src/app/language_server_footer.rs` | low | no |
| 5 | P2 | Found and fixed: the old quick-fix fetch parsed the whole JSON-RPC message instead of `result`, so no fetched quick fix was ever shown; the fetch is now routed to the server that published the diagnostic | `ui_core/src/app/event_handlers/diagnostics_feed.rs` (was `analyzer_diagnostics.rs:91-110`) | medium | yes |
| 6 | P2 | The audit's "known before building" items are fixed by the rewrite: server request ids (any JSON id), URI decoding of `Diagnostic.file_path`, the 500 ms sleep in `restart`, discovery subprocesses on the UI thread (now on the server's thread) | `pulsar_lsp` | medium | yes |
| 7 | P2 | (P5: fixed, see 36.) The rust-analyzer download had no checksum | `plugins/vendor/rust/src/discovery.rs` (`download_rust_analyzer_binary`; moved from `pulsar_lsp` in P4) | medium | yes (P5) |
| 8 | P2 | The resource operations of a `WorkspaceEdit` (create, rename, delete) go to disk; an editor that has the renamed or deleted file open is not told | `pulsar_lsp/src/edit.rs`, `code_editor/.../text_editor/lsp.rs` | medium | no |
| 9 | P2 | `CREATE_NO_WINDOW` is set for every server on Windows, but running a `.cmd` shim (`cmd /c`, or the Node entry point, Q10) is not done: it is P5's | `pulsar_lsp/src/server.rs` (`run_server`) | low | yes (P5: the entry point, the shim as the fallback) |
| 10 | P2 | One `LanguageServers` per `PulsarApp` window: a second editor window starts its own servers (the old per-window analyzer did too) | `ui_core/src/app/construction.rs` | low | no |
| 11 | P2 | The Rust spec is an engine contribution (`engine:pulsar_lsp`, page "Plugin: rust-analyzer") until P4: its saved settings live under `plugin/engine.pulsar_lsp/` and are not migrated to `pulsar.rust` | `pulsar_lsp/src/rust_analyzer/mod.rs` | low | yes (P4: `SettingsMove`, see 29) |
| 12 | P2 | The listed stand-in: `known_specs()`, `LanguageServers::spec_for` (extension match) and the spec's `file_extensions`/`language_ids` are what P3's `Language` registry replaces | `pulsar_lsp/src/service.rs`, `spec.rs` | n/a | yes (P3) |
| 13 | P2 | (P4: still not run, see 27.) `just arch` was not run: `python` here is the Windows Store stub. New edges, reviewed by hand: `ui_core`/`script_editor_plugin` -> `pulsar_lsp`, `pulsar_lsp` -> `pulsar_settings`/`pulsar_plugins`/`engine_fs`, `ui_settings` -> `pulsar_settings` | `justfile` (`arch`) | medium | rerun |
| 14 | P2 | Seen, not ours, not touched: `plugins/installed/tests/absent_plugins.rs:634` builds a `FrameView` without `coordinate_space`; `cargo check -p ui` on its own fails on `InspectorTab::Profiler` (feature-set dependent, the workspace build is fine) | see location | low | no |
| 15 | P2 | The standard group's `initialization_options` is a multi-line text in the schema, but the Settings window maps every `TextInput` to a one-line field | `ui_settings/src/utils/plugins.rs` | low | no |
| 16 | P2 | The footer menu, the Settings page and the Problems header menus are built and their data is tested; they were not rendered or clicked (no GUI run was allowed) | `ui_core/src/app/language_server_footer.rs`, `ui_settings`, `ui_problems` | medium | verify by running |
| 17 | P3 | A TypeScript class folder is not a class file type of the file manager (row 5 of section 2 was not done): there is no editor for it, and deriving a folder file type from `class_markers` would claim `class.ts` folders for the Blueprint editor's `class` type | `plugin_manager/src/registry.rs` | low | yes (P5: a class folder is one item and opens its source; no folder file type, see P5; still not rendered, 33) |
| 18 | P3 | The code editor's per-extension file types (new-file templates, icons, colours: `rs`, `js`, `ts`, `py`, `lua`, `toml`, `md`) still name Rust and TypeScript (row 17); `Language` carries no icon or template. The highlighter's grammar table stays foundation (row 16) | `plugins/vendor/code_editor/src/builtin_provider.rs`, `src/lib.rs` | low | no |
| 19 | P3 | Not language registrations, left as they are: the file manager tree's other kinds' marker files (`struct.json`, `enum.json`, `trait.json`, `alias.json`: kinds, not languages), the `blueprint` asset kind and `ObjectType::Blueprint` (row 14), the legacy `.blueprint.json` suffix in `class_name_of_asset` | `ui_file_manager/src/utils/tree.rs`, `ui_types_common`, `pulsar_traits/src/roles.rs` | low | no |
| 20 | P3 | A class is found only through a language that is linked and enabled: a packager built without the `scripting` feature, or a project that disables a language's plugin, no longer sees a class that has only that language's marker (before, `is_class_dir` named both markers). That is the intended "disabled = absent", but such a class gets no "has source but no compiled module" message and drops from the file tree's class files | `pulsar_class/src/registry.rs` (`is_class_dir`), `pulsar_package/src/compile.rs` | medium | no |
| 21 | P3 | Class problems refresh in the Problems drawer at project open, on a plugin-set change and every 30 s, not on a file change; the scan runs on a worker but walks the whole project each time | `ui_core/src/app/class_problems.rs` | low | no |
| 22 | P3 | A language of a DLL plugin arrives only through its root's `build` (`export_library!`); `Language` and `SettingsGroups` have `fn` pointers, so their contract `VERSION` must be bumped on every field change; the library import of a `Language` was not exercised by a test | `pulsar_language/src/lib.rs`, `pulsar_plugins/src/library.rs` | low | no |
| 23 | P3 | `just arch` was not run (`python` here is the Windows Store stub). New edges, reviewed by hand against `layers.toml` (all kernel or tool to kernel, plugin to kernel): `pulsar_class`, `pulsar_traits`, `pulsar_project`, `pulsar_package`, `plugin_manager`, `plugin_editor_api` -> `pulsar_language`; `pulsar_language` -> `pulsar_plugins`, `pulsar_settings`, `pulsar_script_vm`, `engine_fs`; `plugin_manager` -> `pulsar_class`; `ui_common`, `ui_file_manager`, `ui_problems`, `ui_core`, `ui_loading_screen` -> `pulsar_language`; `ui_core` -> `pulsar_class`; dev dependencies `pulsar_language` of `engine`, `engine_backend`, `level_migrate`, `ui_level_editor`; the plugins' tests link only `pulsar_language`. The language tests that need plugins are in `plugins/installed/tests` | `justfile` (`arch`) | medium | rerun |
| 24 | P3 | `plugins/installed/tests/{absent_plugins,runtime_render_parity,default_pipelines}.rs` do not compile (`FrameView` without `coordinate_space`, Helio's change): pre-existing, not ours, not touched (extends #14); `cargo test -p installed_plugins --test languages` builds and runs on its own | see location | low | no |
| 25 | P3 | The Blueprint editor's non-compile uses of its class listing (`sibling_class_dirs`, the events lookup) do not filter classes that are two languages': only the compile and validate paths do (`validation::project_class_dirs`) | `blueprint_editor/src/features/class_dirs.rs` | low | no |
| 26 | P4 | `plugins/vendor/rust` is a plain directory in this repository, not yet its own repository (`Plugin_Rust`) or a submodule; it is wired like `plugins/vendor/typescript`. To make it one: create `Far-Beyond-Pulsar/Plugin_Rust` from the directory's contents, `git rm -r --cached plugins/vendor/rust`, `git submodule add <url> plugins/vendor/rust`, commit the plugin first, re-pin to its main (`just check-submodule-pins`), then merge Pulsar. The workspace member and dependency entries and the generated installed block do not change | `plugins/vendor/rust`, `.gitmodules` | low | user action |
| 27 | P4 | `just arch` was not run (`python` here is the Windows Store stub). The scan was ported to a throwaway script and first matched the recorded state (975 files, 21 hits, 0 new, 0 gone), then the added terms: 24 hits, 24 recorded. The dependency half was reviewed by hand: new edges are `rust_plugin` (plugin) -> `pulsar_language`, `pulsar_plugins`, `pulsar_settings`, `pulsar_lsp` (kernel), and dev dependencies of the host `installed_plugins` on `pulsar_lsp` and `ui`; no kernel crate gained a plugin or tool edge, `pulsar_lsp` lost `inventory` | `justfile` (`arch`), `architecture/layers.toml` | medium | rerun |
| 28 | P4 | `rust_plugin` depends on `pulsar_lsp` (GPUI) for three LSP helpers (`code_actions::edits_in_workspace_edit`, `quick_edit_of`, `uri_to_path`); a GPUI-free home (`pulsar_language`) would let a language plugin avoid the service crate | `plugins/vendor/rust/Cargo.toml` | low | no |
| 29 | P4 | The settings move renames the file (an older editor build then finds no values) and runs on the thread that activates plugin settings; it applies only while `pulsar.rust` is enabled (a project that disables it leaves the old file until it is enabled once); a project-scope file is moved only for the open project | `engine_state/src/plugin_settings.rs` (`apply_moves`) | low | no |
| 30 | P4 | There is no `[kinds]` term for Blueprint as a language beyond `graph save`: the kernel names Blueprint in about 60 files as the engine's node vocabulary (`pulsar_std`, `#[blueprint]`, `BlueprintClassRef`), which is not the language plugin's name. The Blueprint plugin's own terms (the `graph_save.json` marker) are covered. Likewise `oxc` (TypeScript parser) is recorded for `pulsar_script_ts` until it moves (P5) | `architecture/layers.toml`, `architecture/kind-names.txt` | low | no |
| 31 | P5 | The managed install pins npm's sha512 integrity (as hex), not a sha256 of the tarballs (computing that needs the tarballs downloaded, which was not done; the verifier takes either, so it is a table edit: `PINS` in `install.rs`). The pinned typescript-language-server 4.4.0 is taken to be a bundled single file (npm lists no dependencies for it); not confirmed by unpacking it | `plugins/vendor/typescript/src/install.rs` | medium | no |
| 32 | P5 | The real typescript-language-server was not run (not installed here; not installed for the test). The scripted-pipe tests and the JavaScript stand-in prove the client, the spec and the spawn, not tsserver's actual behaviour (its progress titles, how it reads the generated tsconfig, whether the class files type-check clean against `pulsar.d.ts`) | tests, `server.rs` | medium | verify by running |
| 33 | P5 | The footer (one "LSP Servers" entry, the dropdown and submenus, Install), the file manager's class item and opening a class folder were **not rendered or clicked** (no GUI run allowed): their data and rules are unit tested, the widgets compile | `ui_core/src/app/language_server_footer.rs`, `ui_file_manager`, `tab_management.rs` | medium | verify by running |
| 34 | P5 | A TypeScript server is lazy (first `.ts` file), so the footer has no "LSP Servers" entry, and so no Install, until a class is opened; a project with classes could start it at open (`start = project` is the person's setting, or `config_files` could gate an eager start). Also the compiler's range is the word at the point, not an oxc span; `CompileDiagnostic` carries no end | `server.rs`, `checks.rs` | low | no |
| 35 | P5 | The install downloads with the system's `curl` (PowerShell on Windows as the second try), like rust-analyzer's, not with an HTTP client of the engine (`reqwest` needs a TLS provider the plugin cannot assume); a machine with neither cannot install. The tar reader skips pax records but `path` and GNU long names, which npm does not use for these packages | `install.rs`, `archive.rs` | low | no |
| 36 | P5 | The rust-analyzer digest is the one GitHub publishes with the asset (same origin as the file), not a pin; a Windows install unpacks the zip with PowerShell. `code_editor` (submodule) was edited for the checks (`lsp_check`, `forget_source`, the `Missing` status), uncommitted like the rest | `plugins/vendor/rust/src/discovery.rs`, `plugins/vendor/code_editor` | low | no |
| 37 | P6 | `ClassSchema` (stable field identity, `class_version`, `@renamedFrom`) lives in the TypeScript compiler and writes `class.schema.json`; `ModuleBuilder` takes the ids and the version as data. A second text language needs the same reconcile logic and would copy it | `plugins/vendor/typescript/compiler/src/schema.rs` | medium | no |
| 38 | P6 | The golden test pins successful compiles only (the emitted bytes); the error paths (diagnostics and their order) are covered by the existing compile tests. A builder error maps to a diagnostic at 0:0 with no span (as too-many-registers always did) | `compiler/tests/golden.rs`, `lower.rs` (`builder_error`) | low | no |
| 39 | P6 | The lowering still emits `Move`, `Branch` operands, `Return` and unary conversions with `emit` and checks types itself with its own messages and literal steering; `FunctionBuilder::unary`/`binary` are exercised by the builder's tests and the toy language, not by TypeScript | `lower.rs`, `module_builder.rs` | low | no |
| 40 | P6 | `cargo clippy --all-targets` on `pulsar_script_vm` fails on the existing `tests/libraries.rs` (`std::fs` and `Path::is_file`, disallowed methods): pre-existing, tests only; the library and the new tests are clean. `golden.rs` allows the lint with a reason (it reads and re-records its own fixtures) | `pulsar_script_vm/tests/libraries.rs` | low | no |

Known before building: the unverified download checksum (row 7) is fixed in P5; the open items are rows 31 to 40 and the earlier ones marked no. Section 7's last risk (a 2,000-line refactor with no second consumer) is closed: the toy language is the second consumer and the golden files are the oracle.

## 9. Unverified or not read

- `rust_classes` / `generate_class` (`pulsar_project/src/model.rs:219-260`): read down to
  where it loads the module; how much of "compile to Rust" is Blueprint-specific beyond the
  `graph_save.json` key was not read (section 2 row 6).
- (Resolved in P3: the Blueprint plugin is `pulsar.blueprints`, root `blueprint_editor_plugin`.) Whether a Blueprint-owning plugin id existed separate from the code editor's builtin
  provider (`blueprint_editor/src/builtin_provider.rs:46,119` registers both the provider
  and `LinkedScriptLanguage`; its `pulsar-plugin.toml` was not opened) (Q1).
- `pulsar_script_ts/src/diagnostic.rs` span completeness for squiggles (section 4.3).
- The Windows spawn behaviour of `.cmd` shims and the console-window flash (section 4.2)
  was not run; it is reasoned from Rust's documented `Command` behaviour.
- `discovery.rs:~380-500` (the GitHub download) was grepped for the hash check, not read
  in full.
- Whether any `.pulsar` settings schema already has a `language_servers` namespace
  (`engine_state::settings`, Q3).
- Mockaco's (external git dependency) own `Language::from_path` (`text_editor.rs:186`) and
  whether it can be driven by `Language::highlight_id` instead of its own table.
- The status bar's i18n keys for the analyzer (`StatusBar.Stop`, `Notification.*`) were
  seen at use sites, not in the locale files.
- Tests and non-`.rs` files (docs, `.toml`) were not scanned for `graph_save.json` /
  `class.ts` beyond the production references listed in section 2.
