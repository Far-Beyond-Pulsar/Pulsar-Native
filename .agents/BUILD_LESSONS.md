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
