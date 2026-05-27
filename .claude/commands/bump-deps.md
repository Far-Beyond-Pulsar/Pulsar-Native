# Bump Dependency Chain

Cascade a git rev update through the full Pulsar repo chain after pushing changes to any lower-level repo.

## The Chain (always go bottom → top)

```
Graphy → PBGC → Pulsar-Native → Plugin_Blueprints → Pulsar-Native (ui_core)
```

## Step-by-Step Procedure

### After pushing to Graphy:
```bash
cd ~/Documents/GitHub/Graphy
NEW_GRAPHY=$(git rev-parse HEAD)

# 1. Update PBGC
sed -i '' "s/rev = \"[0-9a-f]*\" .*#.*[Gg]raphy/rev = \"$NEW_GRAPHY\"/" \
  ~/Documents/GitHub/PBGC/Cargo.toml
# Or edit Cargo.toml manually: find `graphy = { git = ..., rev = "..." }` line

cd ~/Documents/GitHub/PBGC
git add Cargo.toml && git commit -m "Bump Graphy rev to ${NEW_GRAPHY:0:7}" && git push
NEW_PBGC=$(git rev-parse HEAD)

# 2. Update Pulsar-Native workspace
#    Edit Cargo.toml: both `graphy` and `pbgc` workspace.dependencies
cd ~/Documents/GitHub/Pulsar-Native
# graphy line: rev = "..." → $NEW_GRAPHY
# pbgc line:   rev = "..." → $NEW_PBGC
git add Cargo.toml && git commit -m "Bump Graphy $NEW_GRAPHY, PBGC $NEW_PBGC" && git push
NEW_PN=$(git rev-parse HEAD)

# 3. Update Plugin_Blueprints
cd ~/Documents/GitHub/Plugin_Blueprints
# Cargo.toml: all Pulsar-Native rev= lines → $NEW_PN
#             graphy rev= line              → $NEW_GRAPHY
#             pbgc rev= line               → $NEW_PBGC
git add Cargo.toml && git commit -m "Bump deps ..." && git push
NEW_PB=$(git rev-parse HEAD)

# 4. Update Pulsar-Native ui_core (points back to Plugin_Blueprints)
cd ~/Documents/GitHub/Pulsar-Native
# ui-crates/ui_core/Cargo.toml: blueprint_editor_plugin rev= → $NEW_PB
git add ui-crates/ui_core/Cargo.toml && git commit -m "Bump Plugin_Blueprints rev to ${NEW_PB:0:7}" && git push
```

### After pushing to PBGC only:
Skip step 1. Start at "Update Pulsar-Native workspace" with just the pbgc line.

### After pushing to Plugin_Blueprints only:
Skip steps 1-3. Just update `ui-crates/ui_core/Cargo.toml` in Pulsar-Native.

### After pushing to Pulsar-Native only:
Update Plugin_Blueprints `Cargo.toml` (all Pulsar-Native `rev =` lines), push, then update `ui-crates/ui_core/Cargo.toml` in Pulsar-Native.

---

## Rules

1. **Always use `git rev-parse HEAD` immediately after push.** Never reconstruct a SHA from memory.
2. **Never skip a level.** If PBGC changed, Pulsar-Native must pick up the new PBGC rev even if you only "care about" Plugin_Blueprints.
3. **The `[patch]` sections in `Pulsar-Native/Cargo.toml`** redirect all `git = "...Pulsar-Native"` references to local paths. Don't remove them — they prevent duplicate-crate link errors.
4. **Plugin_Blueprints `Cargo.toml`** must have matching revs for all five Pulsar-Native deps (`ui`, `ui_common`, `pulsar_std`, `engine_backend`, `pulsar_reflection`, `pulsar_std_bundle`, `pulsar_bp_executor`) plus `graphy` and `pbgc`.

## Verifying a rev exists

```bash
git -C ~/Documents/GitHub/Graphy cat-file -t <SHA>   # should print "commit"
```

If it prints "missing", the SHA is wrong.

## Common Error: duplicate crate versions

```
note: there are multiple different versions of crate `graphy` in the dependency graph
  944b114 ... this is the expected type
  d18837c ... this is the found type
```

**Cause:** One of the repos still pins the old rev. Grep all Cargo.toml files:
```bash
grep -r "graphy\|pbgc" ~/Documents/GitHub/Pulsar-Native ~/Documents/GitHub/Plugin_Blueprints --include="Cargo.toml" | grep "rev ="
```
Find the stale one and update it.
