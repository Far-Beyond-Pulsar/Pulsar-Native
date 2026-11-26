# Build lessons: why these builds went wrong, and what not to do

Written after moving the editor's plugins out of the executable (one DLL per plugin), putting the UI, reflection,
profiling, SceneDB and Helio crates into shared Rust dylibs, and building releases for every plugin on five platforms.
Every item below was hit for real; the "how it was found" lines are the techniques that worked. Read this before
changing how anything is linked, how features are chosen, or how a CI build is invoked. The mechanism itself is
described in `.agents/PLUGIN_SYSTEM.md` and `.agents/CI.md`.

## 1. The rule underneath most of it: one copy of everything that is shared

A Rust `dylib`/`cdylib` has no stable ABI, and every image (the executable, each plugin DLL, each shared dylib) that
links a crate statically gets **its own copy of that crate's statics**. Registries built on `inventory`, `OnceLock`,
`thread_local!`, parking-lot tables, id counters and "run once" guards all split per image.

What it looked like:

- Every Helio component was registered twice, once per image that linked `helio_component` (the Extensions window:
  "registers two `component.class` contributions with id `CameraComponent`", 21 times).
- Plugin objects did not render: asset loaders were installed under a `Once` that was spent before a DLL's
  registrations arrived.
- Meshes did not load: `engine_state::get_project_path()` read a context static that nothing sets inside a plugin
  DLL, so every mesh found "no project path".
- 34 copies of `Transform::translate` were registered, one per plugin DLL.

Patterns to avoid:

- A crate that holds process-wide state and is linked statically into more than one image. Put it in one shared
  dylib, or hand the host's state to every other copy (`attach_*` entry points: settings, event bus, profiler, Helio
  runtime, world runtimes, plugin view, engine context) and make that the *only* way the state is read.
- A `Once`/`OnceLock` guard around "register what is linked": registrations can arrive later (a library loaded after
  startup). Make registration incremental and keyed by identity (`pulsar_plugins::Handled`).
- Depending on an implementation crate (`engine_backend`, `ui_common`) for a few types. Every plugin then links a
  private copy of it and its statics. Depend on a thin API crate that holds the types only. (Measured: about 40
  engine crates are duplicated into most of the 34 plugin DLLs; Blueprint's DLL closure would shrink from 842 to
  about 745 crates by depending on thin crates.)
- Trusting that an attach hook exists. A library built before a hook existed returns `None`; log a refusal loudly
  (`refused the host engine context`), never fall back silently to the private copy.

The structural end state is a shared engine dylib holding these crates once; the attach hooks are the stopgap.

## 2. Cargo feature unification decides every hash

A crate's output file name and its symbol disambiguator come from `-C metadata`, which Cargo derives from the
crate's source, **features**, profile, target, flags and the hashes of all its dependencies. A plugin DLL imports
symbols by name, and those names contain the hash of every crate it links in a shared dylib.

Cargo unifies features across everything selected in one invocation. So `cargo build -p pulsar_engine` (editor
only) and `cargo build -p pulsar_engine -p <every plugin> --features <plugin>/dll` give shared crates different
features, therefore different hashes, therefore different shared libraries. Plugins built by one command then fail
to load against libraries built by the other.

- Fix: a generated "workspace hack" crate (`tools/workspace_hack`, written by `cargo run -p hack_gen`, checked with
  `--check`) that depends on every crate inside the shared libraries with the union of the features anyone asks for,
  as a dependency **and** as a build dependency (resolver 2 unifies the copy a build script or proc-macro uses
  apart from the copy target code uses), per platform (`[target.<triple>.*]`). Every build includes it.
- It must be regenerated whenever a dependency of a library or a plugin changes.
- The generator got three things wrong before it was right, and each was invisible until a different build method
  was compared:
  1. It decided "inside the libraries" from a libraries-only graph. A feature a pinned crate has only in the union
     (`glam`'s `approx`) makes more dependencies reachable; `approx` was never pinned, a plugin enabled
     `approx/std`, and every hash above it changed. Decide membership from the union resolve.
  2. It trusted `cargo tree --target`, which lists optional dependencies a platform does not build (a Windows-only
     crate on Linux). Use `cargo metadata --filter-platform <triple>`: exact.
  3. It resolved with the previous hack file in place, so the hack's own pins fed back into the answer. Resolve with
     the hack emptied.

How it was found: build the editor-only way and the plugin-builder way into two fresh target directories with
`CARGO_TERM_VERBOSE=true`, then diff every compiler invocation by crate: same features, same flags, same
dependency hashes? The crates whose own inputs differ are the origins; everything else is inherited. (The scripts
are in the history of this work; the method is the point.)

## 3. Build scripts read the environment, and tools that launch cargo leak it

`ring`'s build script declares `CARGO_MANIFEST_DIR`, `CARGO_PKG_NAME` and `CARGO_PKG_VERSION_*` as
`rerun-if-env-changed` inputs, and Cargo compares them with **its own** process environment. `cargo run -p
plugin_builder` sets those variables for the tool, and the tool's child `cargo` inherited them. Every alternation
between `just plugins` and `cargo run --release` reran `ring`, which rebuilt everything above it (about 130
crates, the shared libraries included).

- A tool that spawns cargo must remove `CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH`, `CARGO_PKG_*`,
  `CARGO_CRATE_NAME`, `CARGO_BIN_NAME`, `CARGO_PRIMARY_PACKAGE`, `CARGO_BIN_EXE_*` from the child's environment.
- Cargo's staleness check is mtimes *and* declared environment variables. When "nothing changed" rebuilds
  happen, read the fingerprints: `target/<profile>/.fingerprint/<unit>/run-build-script-build-script-build.json`
  lists each tracked variable with the value of the last run. Two runs with different values is the answer.
- Different shells have different environments (a developer prompt sets `INCLUDE`, `LIB`, `CC`). Alternating shells
  can do the same thing; keep one way of building.

## 4. Git dependencies float, and a lockfile refresh can move them

A git dependency on a branch (`wgpu`, `branch = "helio/v30.0.1"`) is only stable while the lockfile pins the commit.
`cargo metadata` (without `--locked`), `cargo update --workspace` and even adding one more crate of that source to
a manifest re-resolved the branch to its newest commit; the new commit broke the Windows build (`windows` 0.54
against 0.62 in `wgpu-hal`) and the broken lock was pushed.

- Always pass `--locked` to `cargo metadata`/`tree`/`build` in scripts and in CI, and never commit a `Cargo.lock`
  produced by a command that was allowed to change it without reading the diff (`git diff Cargo.lock | grep
  '^[+-]source'`). Restore a floated source with `cargo update -p <crate> --precise <rev> --offline`.
- Pin git dependencies by `rev`, not `branch`.
- Every git dependency of a shared library also changes the file name of that library (next section).

## 5. Shared library file names carry a hash when the source is not a workspace path

Cargo adds `-C extra-filename=-<hash>` to the outputs of packages that are not workspace members. `scenedb_dylib`
comes from a git dependency, so a plugin DLL imports `scenedb_dylib-<hash>.dll`, while Cargo reports (and the
build tool installed) only the plain `scenedb_dylib.dll`. Launched by `cargo run`, `deps/` is on `PATH` and it
loads; launched directly, Windows error 126 (module not found) for all 34 plugins.

- Install a shared library under every name something imports: the tool now also installs
  `deps/<name>-<hash>.<ext>` when its content equals the plain file.
- Prefer path dependencies for shared libraries, so their names are stable.
- Read the real cause of a load failure: `LoadLibraryExW` error 126 is a missing dependency file, 127 is a missing
  export (a hash/layout mismatch). `dumpbin /dependents plugin.dll` lists the exact file names it needs (under Git
  Bash set `MSYS2_ARG_CONV_EXCL="*"` or the `/dependents` flag is rewritten into a path).

### What actually breaks a plugin when a library is rebuilt (measured)

A plugin imports a shared library by file name and its symbols by mangled name. Tested on Windows with the six
libraries: rebuilding the five workspace-member libraries (source changed, versions unchanged) leaves every
already-built plugin loadable. What breaks them is a change of the **dependency graph**, which Cargo hashes into
the file name of a git-dependency library: bumping `ui_dylib` from 0.1.1 to 0.1.2 changed `scenedb_dylib`'s
hash (the workspace hack puts every library in every other's closure), and every plugin then asked for a file no
build produced. The symbols inside were the same: byte-patching only the import name in a copy of each plugin
(same length, 16 hex digits) loaded all 34 and rendered the scene. Two consequences:

- A version bump of any shared library renames `scenedb_dylib-<hash>.dll`. Bump them together and rebuild plugins,
  or make SceneDB a workspace member so its file name is stable.
- The engine carries a fuzzy renamer for the meantime (`plugin_manager::imports::compat_copy`): it reads a
  plugin's import table, and for a hash-named library it does not have, loads a copy whose import names the one
  it linked itself (Windows; cached under the app-data `plugin_compat`; the integrity manifest still checks the
  original). Same symbols is assumed; a layout change under unchanged symbol names would still misbehave, and the
  plugin API version is the gate for that.
- Two file names for one library are two copies in the process (split statics). Never make both names exist beside
  the engine; rename the plugin's import, not the library.

## 6. Prebuilt crates (Cargo SDK): what an SDK must contain

Cargo recompiles the crates inside a shared library for every consumer, because rustc needs their metadata. The
[Cargo-SDK](https://github.com/Far-Beyond-Pulsar/Cargo-SDK) wrapper serves stored outputs by crate name and hash
(measured: 640 s to 222 s for one plugin). Things that were wrong at first:

- Metadata-only stub rlibs break build scripts and proc-macros, which link the same crates and need the code.
  Store whole rlibs.
- A proc-macro compiled again in another target directory has different metadata (paths of build-script output are
  part of it), and `rustc` then refuses stored rlibs that recorded the original ("can't find crate"). Store and
  serve proc-macros too.
- macOS archives (BSD `ar`) name members in the member data, Cargo moves the final dylib out of `deps/`, and GNU
  `tar` reads `D:\path\x.tar.gz` as a file on host `D`. Test on all three operating systems; the toy workspace in
  Cargo-SDK does.
- An SDK serves a crate only when the hash matches, so every difference listed in sections 2 and 3 silently
  disables it (the build still succeeds, only slowly). The pipelines print how many crates the SDK served and warn
  when it is a handful.

## 7. Platforms behave differently exactly where you did not test

None of these showed on Windows x86_64:

- **macOS** links a `dylib`/plugin with undefined symbols as an error, and a Rust dylib exports Rust symbols
  only. SQLite, zlib and tree-sitter live as static C inside the shared libraries; the Windows build exports them
  with `/EXPORT` directives, macOS needs `-Wl,-exported_symbol,_name` (done in each library's `build.rs`).
  System frameworks a dependency names (`IOSurface`) are not carried into a dylib link; name them on the library.
- **Linux** compiled a Windows-only crate (`windows-future`) because the hack listed it for every platform.
- **32-bit** `usize` overflowed in `1 << 36` (parking-lot layout fingerprint); use `u64` for fingerprints.
- **Windows i686** cannot be built at all: `mach-dxcompiler-rs` (wgpu's DX12 shader compiler) ships no 32-bit
  binaries. The library and plugin matrices have five platforms.
- **`-D warnings`** is part of every crate's hash and turned a deprecated `i64::max_value()` into a failed Unix
  release. The setup action sets it by default; release builds set `rustflags: ""` so the SDK and its consumers
  agree.
- **The pinned toolchain** (`rust-toolchain.toml` in the checkout) is not the one a setup action installs in
  another directory: `rustup target add <triple>` must run inside the checkout, or `std` for a cross target is
  missing.
- **Per-image statics hide in plain sight.** `helio_mats::graph_registry` kept compiled material-graph shaders in
  a `static`; the materials library registered them and the mesh library's G-buffer pass read its own empty copy,
  so every graph material drew as the default. A registry two libraries both touch lives in the host's engine
  state store (`EngineContext::global().store`), reached from a library through the attached host context.
- **A load is gated before the version gate.** Missing hash-named imports, a stale shared library beside the
  engine, a non-listed manifest entry and the layout fingerprints all refuse a plugin before or beside the API
  version. `plugin_manager::imports::explain` now says which import is missing and what to run.
- **Splash and editor must resolve the plugin directory the same way.** The splash prewarmed `plugins/editor`
  (relative); an editor started outside the install folder loaded none. Use `InstallLayout::of_running_engine()`.
- **A derived-data cache smaller than one scene thrashes.** The 4 GiB cooked-texture default was below the 11 GiB
  a large scene cooks, so each launch evicted, then recompressed on the CPU (minutes, UI blocked). Default is now
  32 GiB.
- **ONNX Runtime has no Intel macOS build**, so a plugin using it (`mocap`) lists `macos-x86_64` in the release
  workflow's `optional_platforms` input. `code_editor` lists both macOS targets until `mockaco_language` can link
  the tree-sitter grammar entry point (`_tree_sitter_rust`) on macOS: exporting `_tree_sitter_*` from `ui_dylib`
  (0.1.2) did not make it resolve; the cause is unconfirmed and needs a macOS machine (`nm -gU` on the released
  dylib will show whether the symbol is exported at all).

Build and run each platform's real pipeline early, on one library, before launching all of them; each failure
cost one cycle (about 45 minutes) and the pipelines are identical per library.

## 8. Patterns to avoid, as a checklist

- Linking a state-holding crate statically into more than one image (1).
- Two ways to build the same thing (`cargo run --release` for the editor and a tool for the plugins) unless a test
  proves they give the same hashes (2, 3).
- A tool that spawns cargo without scrubbing `CARGO_*` (3).
- Running cargo in a script without `--locked`; committing a lockfile without reading its diff; git dependencies
  by branch (4).
- Assuming a shared library's file name is its package name (5).
- `cargo tree` for platform questions (2); metadata filters are exact.
- Trusting the Windows build as evidence for the other platforms (7).
- A feature set that depends on what else is in the invocation (2): if a crate's features can change with the build
  command, it needs a pin.
- Silent fallbacks: an attach hook that is missing, an SDK that serves nothing, a plugin that loads without its
  settings. Each cost hours because it "worked". Log it, warn in CI.
- Using timestamps or file names as compatibility checks. Compatibility is the layout fingerprint plus the API
  version the library exports; a rebuild with unchanged inputs must produce an identical hash, and when it does not
  that is the bug to find (compare compiler invocations, section 2).

## 9. Diagnostics that worked

| Question | Tool |
|---|---|
| Why did cargo rebuild? | `.fingerprint/<unit>/*.json` (features, profile, `deps` hashes), `run-build-script-*.json` (`RerunIfEnvChanged` values); `CARGO_LOG=cargo::core::compiler::fingerprint=info` |
| Which crates differ between two build methods? | Two fresh target dirs, `CARGO_TERM_VERBOSE=true`, diff `-C metadata`, features and flags per crate |
| Why will a DLL not load? | `LoadLibraryExW` error code (126 / 127); `dumpbin /dependents`; the app's log |
| What is a crate's real platform graph? | `cargo metadata --filter-platform <triple>` |
| Did a lock refresh float something? | `git diff Cargo.lock \| grep '^[+-]source'` |
| Which units must an SDK serve? | `cargo sdk exec --verbose` lists what was served |

## 10. Disk: why a target directory grows without bound, and what bounds it (measured)

A long-lived `target/` reached 570 GB (`debug` 488 GB, `release` 79 GB) while the repository needs about 30 GB per
build shape. Cargo never deletes the artifacts of a unit it no longer builds that way, so every distinct hash is a
permanent extra copy. Measured on that directory (Windows, 2026-11-25):

| Where it goes | Size |
|---|---|
| `debug/deps` | 454 GB: `.pdb` 198, `.rlib` 120, `.exe` 78, `.rmeta` 50, `.dll` 8 |
| `debug/incremental` | 25 GB |
| `debug/build`, `.fingerprint`, `examples` | 3.8, 0.7, 0.7 GB |
| `release/deps` | 75 GB: `.rlib` 47, `.rmeta` 17, `.dll` 5, `.pdb` 3, `.exe` 2 (release has no debug-info problem; it is hash churn only) |
| `plugin_compat` cache | 1.7 GB (136 folders; one per plugin and engine-library state, never removed by the engine) |

- `debug/deps` holds 162,974 files in 47,789 distinct unit hashes for 1,305 crates; 1,268 of the crates exist under
  more than one hash (`ui` 246, `gpui` 235, `engine_backend` 301, `pulsar_scenedb` 354). About 437 of the 451 GB in
  those crates is older copies. Each integration test of `pulsar_package` (smoke, blueprint_compile, ...) exists under 78 to 88
  hashes, each an 80 MB executable plus a 700 MB PDB (962 executables and 1,750 PDBs in `debug/deps`).
- Why so many hashes (from `.fingerprint/*/lib-*.json`): a crate's own features are the same in 213 of 216
  `ui` variants, so the variants are inherited from dependencies. 250 crates were built with more than one feature
  set (`windows-sys` 73 feature sets, `windows` 23, `syn` 13, `smallvec`, `serde_json`, `wgpu`, `reqwest`, ...: each
  `-p` selection, `--features` and test target unifies a different set, section 2; the workspace hack covers only
  what is inside the shared libraries); 1,137 crates were built under more than one profile hash (`windows-sys` 16:
  `CARGO_PROFILE_*`, `--config profile`, build-override and host/target units); 774 under two `rustflags` sets
  (before and after `prefer-dynamic` was added to `.cargo/config.toml`); 190 under more than one source path (crate
  versions, git revs). Every one of these multiplies everything above it.
- The same applies to scratch directories: 26 `CACHEDIR.TAG` directories on `D:\` (`tb*`, `bench-*`, `ta`,
  `pin-test`) and the `target` directories of two worktrees (12 GB and 2 GB), `target-sdk-src` (11 GB) and `target-spike`
  (13 GB) were each a cold copy.

Measured configuration choices (one test binary, `cargo test -p pulsar_package --test smoke --no-run`, empty
target directory, 8 jobs, while another build ran on the machine, so times are indicative):

| Variant | Disk | Clean build | Rebuild after touching `pulsar_package` |
|---|---|---|---|
| before (`debug = "limited"`) | 27.6 GB (deps 21.5, incremental 5.9; PDB 10.2) | 448 s | 41 s |
| `debug = "line-tables-only"` and dependencies `debug = false` (**adopted**) | 22.2 GB (PDB 5.4) | 439 s | 61 s (noise: the build ran beside another) |
| the above and no incremental (`CARGO_INCREMENTAL=0`) | 16.7 GB | 406 s | 163 s |

- Adopted: the first reduction (-20% per build, PDBs -47%). A backtrace still names every frame (functions of
  dependencies keep their names, only their file:line goes: checked with `std::backtrace` through a dependency at
  `debug = false`), so the log viewer's caller tracking, allocation profiling and the profiler read the same
  symbols. No runtime effect (debug info only). A debugger that needs locals or types builds once with
  `CARGO_PROFILE_DEV_DEBUG=limited` in a scratch directory.
- Rejected: `incremental = false` for dev. It is the biggest further saving (-25%) and a rebuild after an edit then
  costs 163 s instead of 41 to 61 s, which is what a developer waits for dozens of times a day; its growth is what
  `just clean-stale` bounds instead. CI already sets `CARGO_INCREMENTAL=0`; release builds are non-incremental.
- `[env]` in `.cargo/config.toml` sets variables for the processes Cargo starts, not for Cargo itself, so
  `CARGO_INCREMENTAL` there would do nothing; the key is `[build] incremental`.
- Not changed: `strip` and `lto` (release is rlib-bound, not debug-info-bound; the comment on `[profile.release]` explains why
  `panic = "abort"` stays off), `split-debuginfo` (an MSVC PDB is always a separate file).

What bounds it (`scripts/target-disk.ps1`, `just disk`, `just clean-stale`, `AGENTS.md` "Disk and target
directories"): the script groups `deps` files into units (`name-<hash>`), keeps the newest three of each family (a
crate name plus its source identity, so two versions of a crate never count against each other), removes the
rest with their `.fingerprint`, `build` and `incremental` entries, never touches anything modified in the last
30 minutes, and does the same for `plugin_compat` (newest folder per plugin file), idle Build & Run game targets
and, on request, idle scratch target directories. Dry run unless `--apply`. What it does not do: it cannot know
which of the newest three a particular build wants, so an invocation shape that was not used lately rebuilds once.
The cure for growth is fewer hash variants (one shared directory, same invocations, no ad hoc profile
overrides), the script only makes the cost of the variants that remain bounded.
