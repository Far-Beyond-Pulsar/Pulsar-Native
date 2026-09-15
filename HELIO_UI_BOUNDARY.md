# Helio / UI integrity boundary

Audit date: 2026-09-12

This is a repository-root audit record for the current Helio/SceneDB work. The
Helio changes must not modify `crates/ui/**`, including the
`crates/ui/wgpui-component` submodule. Existing UI work is independent and is
not to be reverted or folded into the Helio change set.

## Current evidence

- Root `HEAD` is `fb723ba3` (`2026-09-03 20:58:29 -0400`) and contains the
  earlier wgpui pointer at `054f209f`.
- The UI submodule is checked out at `85d39d28` (`2026-09-03 17:11:55 -0400`),
  and its only nested worktree edits are:
  - `crates/ui/src/lib.rs`
  - `crates/ui/src/profiler/mod.rs`
- The parent worktree reports the UI change only as the independent submodule
  pointer `crates/ui/wgpui-component`; no parent-workspace UI file is part of
  the Helio/SceneDB set.
- The Helio submodule is checked out at `8c1d8c42` (`2026-09-12 01:06:02
  -0400`) from parent pointer `c3106597`. Its current changed paths are all
  under Helio crates, Helio examples, or Helio documentation; none is under
  `crates/ui`.
- Parent-workspace Helio/SceneDB paths observed in the current audit are
  `crates/core/engine_backend/src/scene/**`,
  `crates/core/engine_backend/src/subsystems/render/helio_renderer/**`,
  `crates/renderer/helio`, and the existing Helio audit documents/tools. None
  is a UI path.

The UI changes therefore predate this 2026-09-12 audit/task and remain
untouched. The current Helio/SceneDB changes are outside UI paths.

## Repeatable no-UI check

Run from the repository root:

```powershell
$ui = git status --porcelain=v1 --untracked-files=all |
  Where-Object { $_ -match 'crates/ui/' }
if ($ui) { $ui | Write-Host; Write-Host 'UI changes are independent; do not include them in Helio work.' }

$helio = git status --porcelain=v1 --untracked-files=all |
  Where-Object { $_ -match 'crates/(core/engine_backend|renderer/helio)' }
if ($helio | Where-Object { $_ -match 'crates/ui/' }) {
  throw 'Helio/SceneDB change set overlaps crates/ui.'
}

git -C crates/renderer/helio status --porcelain=v1 --untracked-files=all |
  Where-Object { $_ -match 'crates/ui/' } |
  ForEach-Object { throw "Helio submodule contains a UI path: $_" }

Write-Host 'Helio/SceneDB change set has no UI-path overlap.'
```

This check intentionally reports the pre-existing UI work instead of treating
it as a failure or changing it. The separate SceneDB API guard remains at
`tools/check_helio_scene_api_boundary.ps1`.
