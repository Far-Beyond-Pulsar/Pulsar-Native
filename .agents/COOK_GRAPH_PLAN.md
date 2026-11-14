# The cook reference graph: plan

**Status: Phase 0 — design. No code changes.** Read against `main` at
`36b00eff6`. Part of the build pipeline work (`.agents/QUICK_BUILD_PLAN.md`):
the graph decides what the pack step cooks and packs.

The goal: a build ships exactly what its levels can reach (their placed
classes, the classes those scripts spawn, the assets all of them use,
transitively), knows *why* each item is in, cooks and validates only that,
and fails with a clear reason when a reachable item cannot be cooked.

---

## 0. Findings

1. **There is no reference graph.** `pulsar_package::cook_content` cooks
   *every* `.level` in the project (`cook::find_levels`) and compiles and
   validates *every* class it can find (`compile::compile_project` →
   `ClassRegistry::scan`), referenced or not.
2. **Assets are reference-driven, but shallowly and by guessing.**
   `cook::rewrite_assets` walks cooked levels and prefabs and treats a string
   as an asset reference when its key is in `ASSET_KEYS`, ends in `_asset`,
   or its value has an asset file extension. Assets' own references (a
   material's textures, a mesh's materials, an anim graph's clips) are not
   followed: they ship only if a level or prefab names them directly.
3. **The one dependency store is an editor cache.** The project file index
   has an `asset_deps` table (`engine_fs::file_index`, typed edges with a
   `hard`/`hint` role), but only the animation editor fills it, and only when
   an asset is opened or saved. A cook cannot trust a cache that is filled as
   a side effect of editing.
4. **The three errors of the failed pack** (the voxel reference project):
   - 47 × *"has a graph but no compiled module"*: the Blueprints plugin's
     `examples/` classes, served read-only into every project
     (`plugins/Blueprints/examples/...`). The cook compiles every class it
     finds; a language cannot write a module into a read-only folder, and
     nothing references these classes anyway.
   - *`input::Look.ongoing` is not declared*: the input plugin declares each
     project action's events when a game session begins
     (`input_plugin::runtime::session::begin` →
     `events::declare_project_events`). The cook's validation runtime
     (`compile::link_all`) starts no session and declares only component
     events, so a class handling an input action fails to link there and
     nowhere else.
   - *`NewBlueprintClass` is already loaded*: two class folders share a name
     (`NewBlueprintClass.class/` and `src/classes/NewBlueprintClass.class/`).
     The runtime keys classes by name, so a build can hold only one; the
     message does not say that, or which two.

---

## 1. How others do it

| | Roots | Edges | Unreachable dynamic loads |
|---|---|---|---|
| Unreal | Maps to cook (project settings), `AlwaysCook` directories, Primary Asset rules (Asset Manager) | Package imports (hard), soft object paths found by the cooker (soft, followed when cooking) | `DirectoriesToAlwaysCook`, Primary Asset Labels; the cooker warns on unresolved soft refs |
| Unity | Scenes in Build Settings, `Resources/` folders, Addressables groups | Serialized object references (YAML GUIDs) | `Resources`/Addressables only; a string path load of anything else fails at run time |
| Godot | The main scene + export filter | `ext_resource` lines in `.tscn`/`.tres` | Export filter globs |

Common shape: explicit roots, typed edges extracted by each format's owner,
an "always include" escape hatch for loads by computed name, and a report of
why each item was included. That is the design below.

---

## 2. Design

### 2.1 Nodes and edges

A node is a cookable item, identified by its kind and content-relative path:
`level`, `class`, `asset:<kind>` (mesh, material, texture, animgraph, input
map, render graph, ...), and the project files every build ships
(`Pulsar/project.json`, `Pulsar/scripting.json`, the class index, plugin
folder snapshots of enabled plugins).

An edge is `(from, to, role, via)`: `role` is `hard` (needed to load) or
`soft` (named by a script or setting; cooked, but a missing target is a
warning); `via` names the field or node it came from, for the report. Same
shape as the index's `AssetDep`, on purpose (2.5).

### 2.2 Roots

- the startup level (`Pulsar/project.json`), and the levels a project lists to
  ship: `Pulsar.toml` `[cook] levels = ["scene/main.level", ...]`;
- every global script (`Pulsar/scripting.json`);
- `Pulsar.toml` `[cook] always = ["assets/ui/**", "classes/Spawnables/**"]`:
  what scripts load by a name computed at run time (2.4).

Plugin `examples/` folders are never roots. They are cooked only if something
reachable references them.

### 2.3 Extractors: one owner per format

Edges come from a `ReferenceExtractor` registration per file kind, submitted
by the format's owner (the layer gate's rule: core knows no kind):

| Format | Owner | Edges |
|---|---|---|
| `.level` | core (`pulsar_scene` / cook) | placed `ClassInstance`s → class (hard); world-setting roles → class (`WorldRoles::for_level`, hard); component property values → assets (typed, 2.4) |
| class (`prefab.json`, compiled module) | core (`pulsar_class`, `pulsar_script_vm`) | prefab components → assets (typed); module → classes and assets (2.4) |
| `.material`, `.mesh`, `.animgraph`, `.inputmap`, `.rendergraph`, ... | each plugin | its own asset references (the animation plugin's `asset_references` already exist) |

Component property references are found by **type**, not key name: a
reflected property whose type is an asset path (`MeshAssetPath`, texture and
material path types) is a reference. That replaces `ASSET_KEYS` and the
extension guessing.

### 2.4 Script references

A compiled module calls natives with constant arguments. Natives declare which
parameters name a class or an asset (`world::spawn`'s `class`, a level-open
node's `level`, an asset load's `path`) with a parameter attribute
(`ref = "class"` / `"asset:<kind>"` / `"level"`). The extractor walks each
function's instructions and takes the constant that reaches such a parameter
as a soft edge. A non-constant argument (a computed name) cannot be followed:
the cook reports it ("`Spawner.tick` spawns a class by a computed name; list
the candidates in `[cook] always`"), as Unreal's cooker reports unresolved
soft references.

### 2.5 The graph is built by the cook, and shared with the editor

The cook walks from the roots with the extractors, reading sources, never a
cache, so a build is correct whatever the editor did. The same extractors
fill the file index's `asset_deps` when files change, so the editor's
"Used By" / "References" and the cook agree, and the animation editor's
private extraction becomes one of the registrations.

### 2.6 What the cook does with it

- Cooks, validates and packs **only reachable** nodes. Unreachable classes are
  neither compiled nor linked, so the examples' errors disappear unless a
  level uses an example.
- A reachable class with a graph but no module is **compiled in memory**: a new
  `ScriptLanguage::compile_class(dir, natives) -> Result<Module, Vec<CompileDiagnostic>>`
  that writes nothing (a read-only plugin folder's class can ship).
- Two reachable classes with one name are one error that names both folders.
- Writes `.pulsar/build/cook/graph.json`: every included node with the path
  from a root that brought it in (`scene/main.level → DemoGameMode (role) →
  AdvPickup (spawn)`), and every excluded class or asset under a root folder.
  The build dashboard shows the counts and links the file; a "Why is this in
  my build?" lookup reads it.

### 2.7 Script events at validation

A `ProjectEventsRegistration` (engine `game_hooks`): a plugin declares the
script events a project's data defines (the input plugin: one set per
action). The game session runs the registrations at begin (the input
plugin's direct call becomes its registration), and the cook's validation
runtime runs the same ones, so a class links in the cook exactly as in the
game.

---

## 3. What gets deleted

- `cook::find_levels` (every `.level`) as the source of levels.
- `ASSET_KEYS` and the extension-guessing in `rewrite_assets` (references by
  type and by extractor). Path rewriting to content-relative ids stays.
- `compile_project`'s "compile and link every class" (only reachable ones).
- The input plugin's private `declare_project_events` call at session begin
  (it becomes the registration both the game and the cook run).
- The animation editor's private dependency extraction path (it becomes a
  registered extractor; the index stays its store).

---

## 4. Decisions (need the user)

- **Q1. Where the cook roots live.** *Proposed:* `Pulsar.toml` `[cook]`
  (`levels`, `always`), next to the plugin list: build configuration the user
  owns and commits, read by the editor's build and `pulsar package` alike.
  Alternative: `Pulsar/project.json` (but that file ships in the game).
- **Q2. Levels when `[cook] levels` is absent.** *Proposed:* the startup
  level and every level reachable from it (through level-open references),
  not every `.level` in the project.

---

## 5. Phases

- **C1 — classes from levels, and the three errors.** The graph core
  (nodes, edges, walk, report) with the level and class extractors
  (ClassInstances, roles, global scripts, module string constants that name a
  class or level by `ref` attribute); cook/validate only reachable classes;
  `compile_class` in memory; the duplicate-name error; `ProjectEventsRegistration`
  with the input plugin moved onto it (submodule: merge dependency-first).
  Done when the voxel reference project packs with no errors.
- **C2 — assets by type and by extractor.** Typed property references,
  `ReferenceExtractor` registrations in the materials, meshes, animation and
  input plugins; the asset part of the graph; delete `ASSET_KEYS`.
- **C3 — the report and the editor.** `graph.json`, the dashboard summary,
  "Why is this in my build?", extractors filling `asset_deps`.
- **C4 — incremental cook.** Cook output cached per node by its source hash
  and its extractor's version (QUICK_BUILD_PLAN 4, "Incremental cook"): with
  the graph, a quick build re-cooks only changed nodes and their dependents.

## 6. Issues log

(empty)
