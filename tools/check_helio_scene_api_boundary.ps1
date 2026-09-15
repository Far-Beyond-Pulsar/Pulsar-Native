[CmdletBinding()]
param(
    [string] $RepoRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$helioCore = Join-Path $RepoRoot 'crates\renderer\helio\crates\helio-core'
$manifest = Join-Path $helioCore 'Cargo.toml'
$sceneSource = Join-Path $helioCore 'src\scene'

if (-not (Test-Path -LiteralPath $manifest)) {
    throw "helio-core manifest not found: $manifest"
}
if (-not (Test-Path -LiteralPath $sceneSource)) {
    throw "helio-core scene source not found: $sceneSource"
}

$violations = [System.Collections.Generic.List[string]]::new()

# helio-core is the renderer-neutral scene/resource contract. It must not
# acquire a direct dependency on an individual pass crate.
$manifestLines = Get-Content -LiteralPath $manifest
for ($i = 0; $i -lt $manifestLines.Count; $i++) {
    if ($manifestLines[$i] -match '(?i)helio-pass-[A-Za-z0-9_-]+') {
        $violations.Add("${manifest}:$($i + 1): pass dependency: $($manifestLines[$i].Trim())")
    }
}

# Public scene mutation APIs named for a pass/content feature belong in the
# owning Helio layer or pass crate, not helio-core. Keep this list deliberately
# narrow: generic buffer push/update/flush operations are valid core plumbing.
$passTerms = 'portal|foliage|water|post[_-]?process|reflection|decal|voxel|sprite|sdf|ssao|hiz|occlusion|gbuffer|transparent|sky'
$allow = @('reflection_captures_buffer')
$files = Get-ChildItem -LiteralPath $sceneSource -Recurse -File -Filter '*.rs'
foreach ($file in $files) {
    $lines = Get-Content -LiteralPath $file.FullName
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match "^\s*pub(?:\([^)]*\))?\s+fn\s+(\w+)") {
            $name = $Matches[1]
            if ($name -match "(?i)($passTerms)" -and $allow -notcontains $name) {
                $relative = $file.FullName.Substring($RepoRoot.Length).TrimStart('\')
                $violations.Add("${relative}:$($i + 1): pass-specific scene method '$name'")
            }
        }
    }
}

if ($violations.Count -gt 0) {
    Write-Host 'Helio scene API boundary violations:' -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "  $_" }
    exit 1
}

Write-Host 'Helio scene API boundary: clean' -ForegroundColor Green
Write-Host '  helio-core has no helio-pass dependency and no new pass-specific public scene methods.'

# Hard migration gate: legacy creators and persistent scene containers are
# failures until their complete object-to-component mapping is implemented.
$legacy = @(
    'SceneActor', 'insert_actor', '\.scene_mut\(\)',
    'insert_(mesh|material|light|object|decal|texture|virtual_|sectioned_)',
    'add_(portal|sublevel|foliage|water|reflection|post_process)',
    'custom_actors', 'vg_cpu_(meshlets|instances)',
    'billboard_instances', 'corona_emitters'
)
$productionRoots = @(
    (Join-Path $RepoRoot 'crates\renderer\helio\crates'),
    (Join-Path $RepoRoot 'crates\core\engine_backend\src'),
    (Join-Path $RepoRoot 'crates\editor\src')
)
$legacyViolations = [System.Collections.Generic.List[string]]::new()
foreach ($root in $productionRoots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    Get-ChildItem -LiteralPath $root -Recurse -File -Filter '*.rs' | ForEach-Object {
        $file = $_
        $lines = Get-Content -LiteralPath $file.FullName
        for ($i = 0; $i -lt $lines.Count; $i++) {
            foreach ($term in $legacy) {
                if ($lines[$i] -match $term) {
                    $relative = $file.FullName.Substring($RepoRoot.Length).TrimStart('\')
                    $legacyViolations.Add("${relative}:$($i + 1): legacy scene API/container '$term'")
                    break
                }
            }
        }
    }
}
if ($legacyViolations.Count -gt 0) {
    Write-Host 'Helio SceneDB migration gate: FAILED' -ForegroundColor Red
    $legacyViolations | Select-Object -First 200 | ForEach-Object { Write-Host "  $_" }
    if ($legacyViolations.Count -gt 200) { Write-Host "  ... more violations" }
    exit 1
}
