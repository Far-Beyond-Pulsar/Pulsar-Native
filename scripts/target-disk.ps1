<#
.SYNOPSIS
  Report where a Cargo target directory's disk goes, and prune what is safe to prune.

.DESCRIPTION
  Cargo never deletes the artifact of a crate it no longer builds that way. Every distinct combination of
  features, profile, flags and dependency hashes writes a new `name-<hash>` family member into
  target/<profile>/deps (plus its .fingerprint, build and incremental entries), so a long-lived target
  directory only grows. This script keeps the newest -Keep members of each family and removes the rest.
  Nothing is deleted without -Apply. Everything removed is rebuilt by the next build that wants it; no source,
  project or user data is touched.

  What it does, in the order it runs:
    1. Report  - size of each profile directory by category (deps by extension, incremental, build,
                 .fingerprint, top-level copies), and how much of deps is stale family members.
    2. Prune   - per profile directory (target/<profile> and target/<triple>/<profile>):
                 deps units, .fingerprint, build and incremental entries beyond the newest -Keep (-IncrementalKeep
                 for incremental) of their family. A unit modified within -MinAgeMinutes is never removed, so a
                 build that is running (or just finished) keeps its artifacts.
    3. Compat  - the plugin_compat cache (%APPDATA%/Pulsar/Pulsar_Engine/data/plugin_compat): one folder is
                 written per (plugin, engine libraries) pair and never removed; this keeps the newest folder
                 for each plugin file and removes the others.
    4. Games   - `target` directories of the projects in recent_projects.json (Build & Run builds a game in
                 <project>/target) whose newest file is older than -GameDays.
    5. Scratch - with -ScanDir, sibling directories that are Cargo target directories (they carry Cargo's
                 CACHEDIR.TAG) and have not been modified for -ScratchDays: the leftovers of one-off
                 CARGO_TARGET_DIR runs. Only listed unless -Apply.

.PARAMETER Apply         Delete. Without it the script only reports what it would delete.
.PARAMETER TargetDir     The target directory (default: $env:CARGO_TARGET_DIR, else <repo>/target).
.PARAMETER Keep          Members of each deps family to keep (default 3: the build you are on and the two before it, so alternating between three invocation shapes does not rebuild).
.PARAMETER IncrementalKeep  Members of each incremental family to keep (default 1).
.PARAMETER MinAgeMinutes Never remove anything modified within this many minutes (default 30).
.PARAMETER GameDays      Remove a game's target directory idle for this many days (default 30; 0 = report only).
.PARAMETER ScanDir       A directory whose children are scanned for stale Cargo target directories (e.g. D:\).
.PARAMETER ScratchDays   Idle days after which a scanned target directory is removable (default 7).
.PARAMETER ScratchPattern  Wildcard that a scanned directory's name must match (default tb*).
.PARAMETER ReportOnly    Skip the pruning plan, only print sizes (faster).

.EXAMPLE
  pwsh scripts/target-disk.ps1                     # report and dry run
  pwsh scripts/target-disk.ps1 -Apply              # prune the default target directory
  pwsh scripts/target-disk.ps1 -ScanDir D:\ -Apply # also remove idle scratch target dirs named tb*
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [string]$TargetDir,
    [int]$Keep = 3,
    [int]$IncrementalKeep = 1,
    [int]$MinAgeMinutes = 30,
    [int]$GameDays = 30,
    [string]$ScanDir,
    [int]$ScratchDays = 7,
    [string]$ScratchPattern = 'tb*',
    [switch]$ReportOnly,
    # GNU spellings (`just clean-stale --apply`).
    [Parameter(ValueFromRemainingArguments = $true)]$Rest
)

if ($Rest -contains '--apply') { $Apply = [switch]$true }
if ($Rest -contains '--report-only') { $ReportOnly = [switch]$true }

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $TargetDir) {
    $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
}
$now = [DateTime]::UtcNow
$guard = $now.AddMinutes(-$MinAgeMinutes)
$script:freed = [long]0
$script:failed = 0

function Format-Size([long]$bytes) {
    if ($bytes -ge 1GB) { return '{0:N1} GB' -f ($bytes / 1GB) }
    if ($bytes -ge 1MB) { return '{0:N0} MB' -f ($bytes / 1MB) }
    return '{0:N0} KB' -f ($bytes / 1KB)
}

# Size and newest modification of a directory tree.
function Get-TreeInfo([string]$path) {
    $size = [long]0
    $newest = [DateTime]::MinValue
    $stack = New-Object System.Collections.Generic.Stack[System.IO.DirectoryInfo]
    $stack.Push([System.IO.DirectoryInfo]::new($path))
    while ($stack.Count -gt 0) {
        $d = $stack.Pop()
        try {
            foreach ($e in $d.EnumerateFileSystemInfos()) {
                if ($e -is [System.IO.DirectoryInfo]) {
                    if (-not ($e.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { $stack.Push($e) }
                } else {
                    $size += $e.Length
                    if ($e.LastWriteTimeUtc -gt $newest) { $newest = $e.LastWriteTimeUtc }
                }
            }
        } catch { }
    }
    [pscustomobject]@{ Size = $size; Newest = $newest }
}

function Remove-Entry([string]$path, [long]$size) {
    if (-not $Apply) { return }
    try {
        if (Test-Path -LiteralPath $path -PathType Container) {
            [System.IO.Directory]::Delete($path, $true)
        } else {
            [System.IO.File]::Delete($path)
        }
        $script:freed += $size
    } catch {
        $script:failed++
    }
}

# A profile directory is one that holds .fingerprint.
function Get-ProfileDirs([string]$target) {
    $found = New-Object System.Collections.Generic.List[string]
    foreach ($a in [System.IO.Directory]::EnumerateDirectories($target)) {
        if (Test-Path -LiteralPath (Join-Path $a '.fingerprint')) { $found.Add($a); continue }
        foreach ($b in [System.IO.Directory]::EnumerateDirectories($a)) {
            if (Test-Path -LiteralPath (Join-Path $b '.fingerprint')) { $found.Add($b) }
        }
    }
    $found
}

$unitPattern = [regex]'^(?:lib)?(?<name>.+?)-(?<hash>[0-9a-f]{16})(?<ext>\..*)?$'

function Invoke-ProfilePrune([string]$profileDir) {
    $label = $profileDir.Substring($TargetDir.Length).TrimStart('\', '/')
    Write-Host ""
    Write-Host "== $label" -ForegroundColor Cyan

    # --- deps: group files into units (name + hash)
    $units = @{}      # "name|hash" -> @{ Name; Hash; Size; Newest; Files }
    $byExt = @{}
    $deps = Join-Path $profileDir 'deps'
    $other = [long]0
    if (Test-Path -LiteralPath $deps) {
        foreach ($f in [System.IO.DirectoryInfo]::new($deps).EnumerateFiles()) {
            $ext = $f.Extension.ToLowerInvariant()
            if (-not $byExt.ContainsKey($ext)) { $byExt[$ext] = [long]0 }
            $byExt[$ext] += $f.Length
            $m = $unitPattern.Match($f.Name)
            if (-not $m.Success) { $other += $f.Length; continue }
            $name = $m.Groups['name'].Value.Replace('-', '_')
            $key = "$name|$($m.Groups['hash'].Value)"
            $u = $units[$key]
            if (-not $u) {
                $u = @{ Name = $name; Hash = $m.Groups['hash'].Value; Size = [long]0; Newest = [DateTime]::MinValue; Files = (New-Object System.Collections.Generic.List[string]) }
                $units[$key] = $u
            }
            $u.Size += $f.Length
            if ($f.LastWriteTimeUtc -gt $u.Newest) { $u.Newest = $f.LastWriteTimeUtc }
            $u.Files.Add($f.FullName)
        }
    }
    $depsTotal = ($byExt.Values | Measure-Object -Sum).Sum
    if (-not $depsTotal) { $depsTotal = 0 }

    # A family is a crate's name AND its source identity (the `target` value in its fingerprint), so two
    # versions of one crate (windows-sys 0.52 and 0.59) never count against each other. The hash in a
    # deps file name is the hash of the .fingerprint directory of the same unit.
    $fpDirs = @{}
    $fpRoot = Join-Path $profileDir '.fingerprint'
    if (Test-Path -LiteralPath $fpRoot) {
        foreach ($d in [System.IO.Directory]::EnumerateDirectories($fpRoot)) {
            $n = [System.IO.Path]::GetFileName($d)
            if ($n.Length -gt 17) { $fpDirs[$n.Substring($n.Length - 16)] = $d }
        }
    }
    function Get-SourceKey($unit) {
        $dir = $fpDirs[$unit.Hash]
        if (-not $dir) { return '' }
        foreach ($j in [System.IO.Directory]::EnumerateFiles($dir, '*.json')) {
            $leaf = [System.IO.Path]::GetFileName($j)
            if ($leaf -like 'run-build-script*' -or $leaf -like 'build-script*') { continue }
            $m = [regex]::Match([System.IO.File]::ReadAllText($j), '"target":(\d+)')
            if ($m.Success) { return $m.Groups[1].Value }
        }
        return ''
    }
    $victims = New-Object System.Collections.Generic.List[object]
    $victimHashes = New-Object 'System.Collections.Generic.HashSet[string]'
    $families = @($units.Values | Group-Object { $_.Name })
    foreach ($byName in $families) {
        if ($byName.Count -le $Keep) { continue }
        foreach ($u in $byName.Group) { $u.Source = Get-SourceKey $u }
        foreach ($fam in ($byName.Group | Group-Object { $_.Source })) {
            if ($fam.Count -le $Keep) { continue }
            $ordered = $fam.Group | Sort-Object { $_.Newest } -Descending
            for ($i = $Keep; $i -lt $ordered.Count; $i++) {
                $u = $ordered[$i]
                if ($u.Newest -gt $guard) { continue }
                $victims.Add($u)
                [void]$victimHashes.Add($u.Hash)
            }
        }
    }
    $staleBytes = ($victims | Measure-Object -Property Size -Sum).Sum
    if (-not $staleBytes) { $staleBytes = 0 }

    # --- the other categories
    $cats = [ordered]@{}
    foreach ($c in 'incremental', 'build', '.fingerprint', 'examples') {
        $p = Join-Path $profileDir $c
        if (Test-Path -LiteralPath $p) { $cats[$c] = (Get-TreeInfo $p).Size }
    }
    $top = [long]0
    foreach ($f in [System.IO.DirectoryInfo]::new($profileDir).EnumerateFiles()) { $top += $f.Length }

    Write-Host ("  deps            {0,10}   ({1} units, {2} families; {3} hashed units beyond the newest {4} per family = {5})" -f (Format-Size $depsTotal), $units.Count, @($families).Count, $victims.Count, $Keep, (Format-Size $staleBytes))
    foreach ($e in $byExt.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 6) {
        Write-Host ("    {0,-8}      {1,10}" -f $e.Key, (Format-Size $e.Value))
    }
    foreach ($c in $cats.GetEnumerator()) { Write-Host ("  {0,-15} {1,10}" -f $c.Key, (Format-Size $c.Value)) }
    Write-Host ("  top-level files {0,10}   (hardlinks of deps outputs: mostly not additional)" -f (Format-Size $top))

    if ($ReportOnly) { return }

    # --- prune deps units, with the matching fingerprint / build entries
    $removeBytes = $staleBytes
    foreach ($u in $victims) {
        foreach ($file in $u.Files) { Remove-Entry $file 0 }
    }
    if ($Apply) { $script:freed += $staleBytes }

    foreach ($c in '.fingerprint', 'build') {
        $dir = Join-Path $profileDir $c
        if (-not (Test-Path -LiteralPath $dir)) { continue }
        $fam = @{}
        foreach ($d in [System.IO.DirectoryInfo]::new($dir).EnumerateDirectories()) {
            $m = [regex]::Match($d.Name, '^(?<name>.+)-(?<hash>[0-9a-f]{16})$')
            if (-not $m.Success) { continue }
            if ($victimHashes.Contains($m.Groups['hash'].Value)) {
                $info = Get-TreeInfo $d.FullName
                $removeBytes += $info.Size
                Remove-Entry $d.FullName $info.Size
                continue
            }
            # entries with no unit in deps (build-script runs): the same newest-N rule by name
            $name = $m.Groups['name'].Value
            if (-not $fam.ContainsKey($name)) { $fam[$name] = New-Object System.Collections.Generic.List[object] }
            $fam[$name].Add($d)
        }
        if ($c -eq 'build') {
            foreach ($kv in $fam.GetEnumerator()) {
                if ($kv.Value.Count -le $Keep) { continue }
                $ordered = $kv.Value | Sort-Object { $_.LastWriteTimeUtc } -Descending
                for ($i = $Keep; $i -lt $ordered.Count; $i++) {
                    $d = $ordered[$i]
                    $info = Get-TreeInfo $d.FullName
                    if ($info.Newest -gt $guard) { continue }
                    $removeBytes += $info.Size
                    Remove-Entry $d.FullName $info.Size
                }
            }
        }
    }

    # --- incremental: <crate>-<base36>; newest -IncrementalKeep per crate
    $inc = Join-Path $profileDir 'incremental'
    if (Test-Path -LiteralPath $inc) {
        $fam = @{}
        foreach ($d in [System.IO.DirectoryInfo]::new($inc).EnumerateDirectories()) {
            $name = $d.Name -replace '-[0-9a-z]+$', ''
            if (-not $fam.ContainsKey($name)) { $fam[$name] = New-Object System.Collections.Generic.List[object] }
            $fam[$name].Add($d)
        }
        foreach ($kv in $fam.GetEnumerator()) {
            if ($kv.Value.Count -le $IncrementalKeep) { continue }
            $ordered = $kv.Value | Sort-Object { $_.LastWriteTimeUtc } -Descending
            for ($i = $IncrementalKeep; $i -lt $ordered.Count; $i++) {
                $d = $ordered[$i]
                $info = Get-TreeInfo $d.FullName
                if ($info.Newest -gt $guard) { continue }
                $removeBytes += $info.Size
                Remove-Entry $d.FullName $info.Size
            }
        }
    }
    $verb = if ($Apply) { 'removed' } else { 'would remove' }
    Write-Host ("  {0} {1}" -f $verb, (Format-Size $removeBytes)) -ForegroundColor Yellow
    if (-not $Apply) { $script:wouldFree += $removeBytes }
}

$script:wouldFree = [long]0

Write-Host "Target directory: $TargetDir"
if (-not $Apply) { Write-Host "Dry run (nothing is deleted). Add -Apply to delete." -ForegroundColor Yellow }
if (Test-Path -LiteralPath $TargetDir) {
    foreach ($p in Get-ProfileDirs $TargetDir) { Invoke-ProfilePrune $p }
} else {
    Write-Host "  (does not exist)"
}

# --- plugin_compat: one folder per (plugin, engine libraries) pair, never removed by the engine
$appdata = if ($env:APPDATA) { Join-Path $env:APPDATA 'Pulsar\Pulsar_Engine\data' } else { Join-Path $HOME '.local/share/Pulsar_Engine' }
$compat = Join-Path $appdata 'plugin_compat'
if (Test-Path -LiteralPath $compat) {
    Write-Host ""
    Write-Host "== plugin_compat ($compat)" -ForegroundColor Cyan
    $byFile = @{}
    $total = [long]0
    foreach ($d in [System.IO.DirectoryInfo]::new($compat).EnumerateDirectories()) {
        $info = Get-TreeInfo $d.FullName
        $total += $info.Size
        foreach ($f in $d.EnumerateFiles()) {
            if (-not $byFile.ContainsKey($f.Name)) { $byFile[$f.Name] = New-Object System.Collections.Generic.List[object] }
            $byFile[$f.Name].Add(@{ Dir = $d; Size = $info.Size; Newest = $info.Newest })
        }
    }
    $stale = [long]0
    foreach ($kv in $byFile.GetEnumerator()) {
        $ordered = $kv.Value | Sort-Object { $_.Newest } -Descending
        for ($i = 1; $i -lt $ordered.Count; $i++) {
            if ($ordered[$i].Newest -gt $guard) { continue }
            $stale += $ordered[$i].Size
            Remove-Entry $ordered[$i].Dir.FullName $ordered[$i].Size
        }
    }
    Write-Host ("  total {0}; older copies of a plugin that a newer folder replaces: {1}" -f (Format-Size $total), (Format-Size $stale))
    if (-not $Apply) { $script:wouldFree += $stale }
}

# --- the dialog helper's build cache: src-<rev> and target-<rev> per helper revision, never removed
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
$dialog = Join-Path $cargoHome 'pulsar-dialog'
if (Test-Path -LiteralPath $dialog) {
    Write-Host ""
    Write-Host "== pulsar-dialog helper cache ($dialog)" -ForegroundColor Cyan
    foreach ($prefix in 'src-', 'target-') {
        $dirs = @([System.IO.DirectoryInfo]::new($dialog).EnumerateDirectories("$prefix*") | Sort-Object LastWriteTimeUtc -Descending)
        for ($i = 0; $i -lt $dirs.Count; $i++) {
            $info = Get-TreeInfo $dirs[$i].FullName
            $old = $i -ge 1 -and $info.Newest -le $guard
            Write-Host ("  {0,-24} {1,10}  {2}" -f $dirs[$i].Name, (Format-Size $info.Size), $(if ($old) { 'older revision' } else { 'current' }))
            if ($old) { if ($Apply) { Remove-Entry $dirs[$i].FullName $info.Size } else { $script:wouldFree += $info.Size } }
        }
    }
}

# --- Build & Run: <project>/target of recent projects
$recent = Join-Path $appdata 'recent_projects.json'
if ($GameDays -gt 0 -and (Test-Path -LiteralPath $recent)) {
    Write-Host ""
    Write-Host "== game target directories (recent projects idle for $GameDays days)" -ForegroundColor Cyan
    $paths = @()
    try {
        $json = Get-Content -Raw -LiteralPath $recent | ConvertFrom-Json
        $items = if ($json.projects) { $json.projects } else { $json }
        foreach ($i in $items) {
            $p = if ($i -is [string]) { $i } elseif ($i.path) { $i.path } else { $null }
            if ($p) { $paths += $p }
        }
    } catch { Write-Host "  could not read recent_projects.json: $_" }
    foreach ($proj in $paths | Select-Object -Unique) {
        $t = Join-Path $proj 'target'
        if (-not (Test-Path -LiteralPath (Join-Path $t 'CACHEDIR.TAG'))) { continue }
        $info = Get-TreeInfo $t
        $idle = ($now - $info.Newest).TotalDays
        $mark = if ($idle -ge $GameDays) { 'idle' } else { 'in use' }
        Write-Host ("  {0,-60} {1,10}  last written {2:N0} days ago ({3})" -f $t, (Format-Size $info.Size), $idle, $mark)
        if ($idle -ge $GameDays) {
            if ($Apply) { Remove-Entry $t $info.Size } else { $script:wouldFree += $info.Size }
        }
    }
}

# --- scratch target directories next to the repository (one-off CARGO_TARGET_DIR runs)
if ($ScanDir) {
    Write-Host ""
    Write-Host "== scratch Cargo target directories under $ScanDir ($ScratchPattern, idle $ScratchDays days)" -ForegroundColor Cyan
    $realTarget = [System.IO.Path]::GetFullPath($TargetDir).TrimEnd('\', '/')
    foreach ($d in [System.IO.DirectoryInfo]::new($ScanDir).EnumerateDirectories()) {
        if ($d.Name -notlike $ScratchPattern) { continue }
        if (-not (Test-Path -LiteralPath (Join-Path $d.FullName 'CACHEDIR.TAG'))) { continue }
        if ($d.FullName.TrimEnd('\', '/') -ieq $realTarget) { continue }
        $info = Get-TreeInfo $d.FullName
        $idle = ($now - $info.Newest).TotalDays
        $removable = $idle -ge $ScratchDays
        Write-Host ("  {0,-40} {1,10}  idle {2:N1} days  {3}" -f $d.Name, (Format-Size $info.Size), $idle, $(if ($removable) { 'removable' } else { 'kept (recent)' }))
        if ($removable) {
            if ($Apply) { Remove-Entry $d.FullName $info.Size } else { $script:wouldFree += $info.Size }
        }
    }
}

Write-Host ""
if ($Apply) {
    Write-Host ("Freed {0}; {1} entries could not be removed (in use)." -f (Format-Size $script:freed), $script:failed) -ForegroundColor Green
} else {
    Write-Host ("Would free about {0}. Run with -Apply to delete." -f (Format-Size $script:wouldFree)) -ForegroundColor Yellow
}
