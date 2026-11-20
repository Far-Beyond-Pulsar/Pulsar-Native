# Readability and organization report

Date: 2026-11-20. Scope: first-party Rust in this repo (1,267 `.rs` files,
about 353k lines). Excluded: `plugins/vendor/*`, `crates/renderer/helio`,
`crates/third-party/*`, `crates/ui/wgpui*` (separate repos or vendored). The
plugin submodules were **not** audited; run the same measurements in each.

Why this exists: developers reading the code by hand say it is hard to follow
and looks "vibe coded". This report separates what the numbers support from
impressions, ranks the causes, and proposes a fix order. Method: file and
function size by script (brace matching, so approximate), grep counts, and
reading the worst offenders. No code was changed.

## Verdict

The architecture is good and documented: the layer gate, the plugin model and
the `.agents/` docs are real strengths. The readability problem is **not**
the shape of the system; it is the texture inside files. Four things make
code read as machine-written to a person:

1. Giant functions and files that do many jobs.
2. Comments that narrate project process (issue numbers, "Phase 4", "P6")
   instead of explaining the code.
3. Documentation that is verbose and repeated, not useful.
4. Repo hygiene noise at the root and in crate layout.

## Findings, by impact

### 1. Oversized functions (highest impact)

13,273 functions measured. 312 are over 100 lines, 65 over 200, 8 over 400.

| Lines | Function |
|---|---|
| ~~749~~ 45 | `ensure_scripts_crate` (first-pass count was wrong, brace-counting tripped on `{{` in format strings; the real long ones in that file were `ensure_embed_lib` 146, `ensure_core_cargo_toml` 144, `ensure_engine_main` 121) |
| ~~640~~ 10 | `edit_scene` (first-pass count was wrong; longest in that file is a 111-line test) |
| 639 | `render_frame_internal`, `engine_backend/.../helio_renderer/renderer.rs:1164` |
| 613 | `build_viewport_ui`, `ui_level_editor/src/ui/viewport/build.rs:164` |
| 481 | `wgsl_declarations`, `one_owner/src/lib.rs:160` |
| 439 | `expand`, `engine_class_derive/src/derive_engine_class.rs:15` |
| 414 | `spawn_input_thread_once`, `ui_level_editor/src/ui/viewport/input_thread.rs:66` |
| 413 | `blueprint`, `pulsar_macros/src/lib.rs:168` |
| 354 | `main`, `engine/src/main.rs:172` |
| 348 | `apply_stream_event`, `ui_core/.../agent_chat_panel/stream_event.rs:60` |
| 347 | `render`, `ui_core/src/app/render.rs:543` |

Also: `render_overlays` 336, `helio_viewport render` 331, `render_footer` 326,
`render_row` 310, `render_list_item` 301, `connect_to_workspace` 297,
`cook_content` 290. A reader cannot hold a 600-line function in their head;
this is the single biggest reason the code feels unreviewable.

Sampled `ensure_scripts_crate`: it builds manifests, creates directories,
writes files and picks names in one body with repeated
`.map_err(|e| format!("Failed to ..."))`. `render_frame_internal` mixes
profiling cfg blocks, timing, gizmo setup and drawing, with `_device` and
`_queue` underscore-prefixed yet used.

Target: no function over 100 lines without a stated reason; none over 200.

### 2. Oversized files and catch-all `lib.rs`

191 files exceed 500 lines, 64 exceed 800, 26 `lib.rs`/`mod.rs` exceed 600.
Largest non-test sources: `helio_renderer/renderer.rs` 2,907,
`engine_fs/src/file_index.rs` 1,834, `helio_renderer/interaction.rs` 1,787,
`pulsar_script_runtime/src/lib.rs` 1,739, `ui_core/.../layout/resolve.rs`
1,655, `pulsar_macros/src/lib.rs` 1,654, `pulsar_world_registry/
component_lifecycle.rs` 1,597, `core_project_builder.rs` 1,576,
`render_runtimes.rs` 1,474. Several crate roots (`pulsar_script_runtime`,
`pulsar_macros`, `pulsar_world_registry`, `editor_task_queue`,
`pulsar_studio_client`, `pulsar_package`, `agent_chat_tools`,
`agent_provider_openai`) hold the whole crate in `lib.rs`. A `lib.rs` should
be a table of contents.

Test files are also huge: `plugins/installed/tests/render_acceptance.rs`
4,916 lines with one 306-line test; `pulsar_game/src/scripting/tests.rs`
3,449 lines.

### 3. Comments narrate process, not code

- 216 comments reference "Phase N", "P#" or issue numbers
  (`Pulsar-Native#1035, Phase 4`, `(#854, #868)`, `(P6)`). Those belong in
  commit messages and `.agents/*_PLAN.md`, not in source a newcomer reads.
- 47 `TODO`s, many with an issue number but no owner or context.
- 140 mentions of "legacy"/"deprecated"/"compat" mark unfinished migrations
  with no removal date.
- The first thing a human sees in `engine/src/main.rs` is a commented-out
  `//#![windows_subsystem...]` and a long marketing-style module doc listing
  "Key Systems". The doc is partly stale ("Window System (now internal to
  GPUI)").

### 4. Documentation is verbose in the wrong places

- `pulsar_std/src/engine/nodes/*/mod.rs` are 64-78% comments. The `and` node
  has the same description written twice (a long `# Inputs/# Returns/#
  Example` block, then a second `# And` paragraph), for a function that is
  `a && b`. This reads as generated boilerplate and buries the code. One
  sentence plus the node macro metadata is enough.
- Conversely, 356 of 1,267 files (28%) have no `//!` module doc, and 1,890
  of 5,942 public functions (32%) are undocumented. Only 20 README files
  exist in the repo, and 44 of the `crates/core` crates have no README at all.
- `crates/README.md` is out of date: says `subsystems/` has physics and
  rendering (it holds `pulsar_scene`), `graphics/` is empty, `external/` is
  not listed.
- `.agents/` is 142k words across 45+ files. Excellent for agents, but a
  human newcomer gets a wall of plan documents. Plans
  (`*_PLAN.md`, `SCRIPTING_EPIC.md`, `VG_DIAGNOSIS.md`) sit next to
  as-built references with no status marker. `INDEX.md` and `AGENTS.md`
  overlap.

### 5. Error handling and diagnostics

- First-pass count: 3,336 `.unwrap()` and 1,005 `.expect()` overall; **1,624 `.unwrap()` outside test paths**. This overstated production use: most sit inside `#[cfg(test)]` modules. The lexer-based gate (`just style`) counts **96** in non-test code.
  non-test code**. Worst: `file_index.rs` 91, `git_hooks.rs` 77,
  `interaction.rs` 63, `layout/format.rs` 57, `texture_assets.rs` 50.
- 173 `pub fn` signatures returning `Result<_, String>` (401 raw occurrences) and 29 `Box<dyn Error>`, against a
  `pulsar_error` crate that exists. Stringly errors with
  `format!("Failed to ...")` repeated everywhere.
- 14 `println!`/`eprintln!` in library code by the gate (111 raw hits including tests, examples and bin crates) while the engine uses tracing.
- 134 `allow(clippy...)` (104 are the documented `disallowed_methods` rule,
  16 `too_many_arguments`). `too_many_arguments` signals functions that need
  a parameter struct (`render_frame_internal` has 8).

### 6. Repo hygiene (cheap, visible, do first)

- Root has `out.txt` (331 KB), `blueprint_graph_debug.json`, empty `state/`,
  `mockups/`, and both `script/` and `scripts/`, plus `tools/`. All of
  `out.txt` and `blueprint_graph_debug.json` are gitignored here but sit in
  the working tree; the empty dirs are on disk only. `script/` vs `scripts/`
  vs `tools/` has no rule for which is which.
- `.env` is tracked (contains `PULSAR_GITHUB_CLIENT_ID`, a public OAuth
  client id, so likely intentional, but a tracked `.env` alarms reviewers;
  rename to `.env.defaults` or document it).
- `diagrams/` has a 1.5 MB drawio and `.$...bkp/.dtmp` lock files committed
  (`.$Renderplugins.drawio.bkp`).
- No `rustfmt.toml`, so formatting is rustfmt defaults. Some `pub use` blocks
  (for example `engine_backend/src/lib.rs`) look hand-wrapped differently from
  rustfmt output. A `scripts/fmt-check.sh` exists; confirm CI runs it (not
  checked here).
- `CONTRIBUTING.md` is a single link to another repo, so there are no in-tree
  conventions for a contributor.
- `crates/core` mixes concerns: UI helper crates (`ui_gen_macros`,
  `window_manager`, `friends_engine`, `pulsar_dialog_embed`) live beside the
  kernel. 102 crates is a lot; `pulsar_script_*` alone is six.
- Filenames: 167 `mod.rs` and 96 `lib.rs` make editor tabs and stack traces
  ambiguous. Flat `foo.rs` + `foo/` is easier to navigate.
- `#[cfg]` density: 763 attribute uses, mostly `feature = "render"` and
  `"editor-ui"` mixed into function bodies (see `render_frame_internal`).

### 7. What is good (keep)

Layer gate and `kind-names.txt`; `.agents/` docs that are accurate where
checked; `clippy::disallowed_methods` for filesystem access with documented
exceptions; `undocumented_unsafe_blocks` warning; 68% of public items already
documented; focused commit subjects on feature work; coherent plugin
registration story. The mess is local, not structural.

## Fix plan

Ordered so each step is reviewable and shrinks the visible mess fastest.

**Step 0: hygiene (one PR, no logic change).** Delete `out.txt`,
`blueprint_graph_debug.json`, `.$*` drawio lock files and the empty
`state/`, `mockups/`; merge `script/` into `scripts/` and state in
CONTRIBUTING what `tools/` is for; rename or explain `.env`; fix
`crates/README.md`; write a real in-tree `CONTRIBUTING.md` (build, `just`
gates, style rules below); add `rustfmt.toml` and enforce `cargo fmt --check`
in CI.

**Step 1: write the style rules down and gate them.** Proposed, enforced by a
ratchet like `kind-names.txt` (shrink-only allowlist):
- function <= 100 lines, file <= 600 lines, `lib.rs`/`mod.rs` <= 150 lines
  of non-`mod`/`pub use` code;
- no issue/phase references in comments (`Phase`, `P[0-9]`, `#NNNN`) except
  in `TODO(#NNNN): what and why`;
- no `println!` in libraries; no new `Result<_, String>` in public API;
- no `.unwrap()` in non-test code without `expect("why this cannot fail")`.
Add the checks to `just arch` or a sibling `just style`, with a baseline file
so existing violations only shrink.

**Step 2: split the top 20 functions and top 15 files** (list above). Per
function: extract named steps, group parameters into a struct, return
`Result` with `pulsar_error` types. Suggested order by user visibility:
`main`, `render_frame_internal`, `build_viewport_ui`, `edit_scene`,
`ensure_scripts_crate` (split into manifest/template/writer), `ui_core`
`render`, then the `render_*` list rows. Each as its own PR with no behavior
change and existing tests as the safety net.

**Step 3: make crate roots tables of contents.** Move bodies out of the eight
`lib.rs` files named above into modules; `lib.rs` keeps docs, `mod` lines and
curated `pub use`. Prefer `foo.rs` over `foo/mod.rs` for new modules.

**Step 4: comment pass.** Mechanical: strip process references from comments
(move the context into the relevant `.agents/` plan if not already there);
cut `pulsar_std` node docs to one sentence each (macro metadata should
generate the rest); resolve or delete each of the 140 "legacy" markers with a
tracking issue; give each `TODO` an owner and a reason. Add `//!` docs to the
356 files that lack them, starting with crate roots and `mod.rs`.

**Step 5: error and logging cleanup.** Convert the five worst unwrap files
first; adopt `pulsar_error` in public APIs crate by crate, starting with
`engine_fs` and `core_project_builder`; replace library `println!` with
`tracing`.

**Step 6: documentation for humans.** A one-page `ARCHITECTURE.md` at the root
(10-15 diagram-light paragraphs: the layers, the plugin model, a request
trace from editor click to GPU) linking into `.agents/`; mark every
`.agents/*_PLAN.md` with `Status: proposed | in progress | as built |
abandoned` and move finished plans to `.agents/archive/`; add a short README
to each `crates/core` crate (what it owns, what it must not depend on).

**Step 7: crate organization, last and optional.** Move UI-ish crates out of
`crates/core`, consider merging the `pulsar_script_*` family behind one
facade. Highest churn, lowest reader payoff; do it only after steps 0-6.

## Measured baseline (for tracking progress)

| Metric | Now | Goal |
|---|---|---|
| Functions > 100 lines | 299 | < 100, then 0 |
| Functions > 200 lines | 65 | 0 |
| Files > 800 lines | 64 | 0 |
| Files without `//!` | 356 | 0 |
| Public fns undocumented | 1,890 | < 500 |
| Phase/issue refs in comments | 216 | ~0 |
| Non-test `.unwrap()` | 96 | < 300 |
| Library `println!` | 14 | 0 |
| `Result<_, String>` | 173 | shrink-only |

Caveats: function lengths come from a brace-counting script that ignores
macros and may mis-measure functions inside `macro_rules!`; unwrap and
`println` counts include doc examples; "non-test" excludes any path containing
`test`. Treat numbers as ordering, not exact.

## Corrections (2026-11-20)

The first-pass numbers above came from grep and a naive brace counter. The
ratchet script (`scripts/style-check.py`, `architecture/STYLE.md`) blanks
comments and strings and skips `#[cfg(test)]` modules, so its figures are the
authoritative baseline: 299 functions over 100 lines, 96 non-test unwraps, 14
library prints, 173 public `Result<_, String>`, 624 process-comment lines
(larger than the 216 first counted, because bare `#NNN` references count),
115 files over their line limit. The remaining findings stand.

Step 2 batch A (as built): split `ensure_embed_lib`, `ensure_core_cargo_toml`,
`ensure_engine_main`, `main`, `cook_content` and `connect_to_workspace`;
new modules `core_project_builder/templates.rs`, `engine/src/startup.rs`,
`pulsar_package/src/content.rs`, `pulsar_studio_client/src/link.rs`.

## Progress as of 2026-11-20

Done, uncommitted in the working tree: Step 0 (hygiene), Step 1 (`just style`
ratchet), Step 2 batches A and B (17 long functions split), Step 3 (seven
oversized crate roots/modules split), Step 4 (process comments, stale docs,
module docs, `.agents/LEGACY_MARKERS.md`), Step 5 (unwraps, prints, 27 public
`Result<_, String>` signatures), Step 6 (`ARCHITECTURE.md`, plan status lines,
45 crate READMEs).

Gate totals now: fn_length 283, file_length 106, process_comments 3, println
13 (kept on purpose, needs a gate exemption), unwrap 1, pub_result_string 138.

Open: 283 functions and 106 files still over their limits (mostly large test
files and UI render code), `pulsar_auth` has no `tracing` dependency, step 7
(crate reorganization) not started, `.agents/*.md` still name some old
`lib.rs` paths. Test failures seen and not caused by these changes as far as
the diffs show (not compared against a clean HEAD): `scene_inventory` ledger
tests, `pulsar_package` smoke (Windows file-in-use), `pulsar_game` roles tests.
