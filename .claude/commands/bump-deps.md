# Bump Dependency Chain

Get a change in Graphy or Plugin_Blueprints into Pulsar-Native.

Both are submodules of Pulsar-Native: Graphy at `crates/third-party/graphy`, Plugin_Blueprints at
`plugins/vendor/blueprint_editor`. Pulsar-Native uses them by path (the root `Cargo.toml` has a
path dependency and a `[patch]` entry for Graphy, and `crates/core/engine` /
`crates/core/pulsar_package` depend on the plugin by path). So bumping one means moving the
submodule pin, not editing a `rev =` in Pulsar-Native.

## The rule

Merge the dependency's PR first, then re-pin the submodule to a commit on the dependency's default
branch, then merge the Pulsar-Native PR. CI's "submodule pins are on upstream main" check
(`just check-submodule-pins`) fails otherwise; `.github/submodule-pin-exceptions.txt` lists the
rare allowed exceptions.

## After a Graphy change merges

```bash
git -C crates/third-party/graphy fetch origin main
git -C crates/third-party/graphy checkout origin/main
git add crates/third-party/graphy
```

The Blueprint plugin also names Graphy by git rev, in `plugins/vendor/blueprint_editor/Cargo.toml`
and `plugins/vendor/blueprint_editor/compiler/Cargo.toml`. Inside Pulsar-Native the `[patch]`
redirects them to the submodule, so they only matter for the plugin's own builds; when you change
them, change both to the same rev in a Plugin_Blueprints PR.

## After a Plugin_Blueprints change merges

```bash
git -C plugins/vendor/blueprint_editor fetch origin main
git -C plugins/vendor/blueprint_editor checkout origin/main
git add plugins/vendor/blueprint_editor
```

## After a Pulsar-Native change a plugin needs

The plugins pin Pulsar-Native crates by `rev =` (in the Blueprint plugin: `gpui-ce`, `ui`,
`plugin_editor_api`, `plugin_manager`, `ui_common`, `engine_backend`, `engine_fs`,
`ui_types_common`, `pulsar_reflection`, `pulsar_std`, `pulsar_script_vm`, `pulsar_script_codegen`;
its `compiler/` crate pins `pulsar_script_vm` separately). Inside Pulsar-Native the root
`[patch."https://github.com/Far-Beyond-Pulsar/Pulsar-Native"]` points them at local paths.

```bash
just bump-plugin-pins <rev>   # rewrite every Pulsar-Native rev under plugins/vendor, refresh lockfiles
just check-plugin-pins        # what CI runs: every pinned rev must be an ancestor of HEAD
```

Commit the rewritten manifests in each plugin repo, merge there, then re-pin the submodules as
above.

---

## Rules

1. **Take SHAs from git, never from memory** (`git rev-parse HEAD`, `git -C <submodule> rev-parse HEAD`).
2. **Don't remove the root `[patch]` sections.** They keep one copy of each shared crate in the graph.
3. **Order matters:** dependency PR merged → submodule re-pinned → Pulsar-Native PR merged.

## Verifying a rev exists

```bash
git -C crates/third-party/graphy cat-file -t <SHA>   # should print "commit"
```

## Common error: duplicate crate versions

```
note: there are multiple different versions of crate `graphy` in the dependency graph
```

See `/debug-compile-errors`. Start with:
```bash
grep -rn 'rev =' --include=Cargo.toml plugins/vendor
cargo tree -i graphy -e normal
```
