# Dependency waste analysis for the Cargo workspace.
#
# Produces three reports in tools/dep_analysis-out/:
#   1. duplicates.txt       raw `cargo tree -d` (whole workspace, normal+build edges)
#   2. duplicates-scan.tsv  one row per duplicated version-set, with reverse-dep roots
#   3. unused-deps.md       declared deps whose name token never appears in the
#                           declaring crate's .rs files (incl. tests/build.rs)
#
# Usage: powershell -File tools/dep_analysis.ps1  (from repo root)
# The unused-dep check is textual; an allowlist below suppresses the known
# derive-only crates (token never appears in source). Every hit still needs a
# human look before removal.

$ErrorActionPreference = 'Stop'
# PS 5.1 decodes native-command output in the OEM codepage; cargo emits UTF-8.
# Without this, the box-drawing glyphs arrive as mangled bytes and every
# regex/tree parse below breaks.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$outDir = Join-Path $PSScriptRoot 'dep_analysis-out'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

# Crates commonly used only through macros/derives, so the crate-name token
# never appears: `#[derive(Serialize)]`, `#[derive(Error)]`, etc.
$deriveOnly = @(
    'serde', 'thiserror', 'bytemuck', 'schemars', 'bincode', 'clap',
    'pin-project', 'pin-project-lite', 'num_enum', 'strum'
)

############################################
# 1 + 2. Duplicate versions
############################################

Write-Host "=== duplicates ==="
cargo tree -d --workspace --edges normal,build > (Join-Path $outDir 'duplicates.txt')

# Versions per package straight from Cargo.lock.
$lock = Get-Content (Join-Path $root 'Cargo.lock') -Raw
$pkgs = @{}
$name = $null
foreach ($line in ($lock -split "`n")) {
    if ($line -match '^\s*name\s*=\s*"([^"]+)"') { $name = $Matches[1] }
    elseif ($line -match '^\s*version\s*=\s*"([^"]+)"' -and $name) {
        if (-not $pkgs.ContainsKey($name)) { $pkgs[$name] = New-Object System.Collections.Generic.List[string] }
        $pkgs[$name].Add($Matches[1])
        $name = $null
    }
}
$dupes = @()
foreach ($k in $pkgs.Keys) {
    $vers = $pkgs[$k] | Sort-Object -Unique
    if ($vers.Count -gt 1) {
        $dupes += [pscustomobject]@{ Package = $k; Versions = ($vers -join ', '); Count = $vers.Count }
    }
}
$dupes = $dupes | Sort-Object -Property Count, Package -Descending
$extraCopies = (($dupes | Measure-Object -Property Count -Sum).Sum - $dupes.Count)
Write-Host ("duplicated packages: {0} ({1} extra copies)" -f $dupes.Count, $extraCopies)

# Reverse trees: who depends on each copy.
# Parse "name vX.Y.Z" pairs from the duplicates dump: those are exactly the
# copies that compile on this target, so that is what we query with -i.
$dupBlock = (Get-Content (Join-Path $outDir 'duplicates.txt') -Raw)
$graphIds = New-Object System.Collections.Generic.List[string]
foreach ($m in ([regex]::Matches($dupBlock, '\n([A-Za-z0-9_.\-]+) v([0-9][^ \t\r\n]*)'))) {
    $id = $m.Groups[1].Value + '@' + $m.Groups[2].Value
    if (-not $graphIds.Contains($id)) { $graphIds.Add($id) }
}

$rows = New-Object System.Collections.Generic.List[object]
foreach ($d in $dupes) {
    $dependents = New-Object System.Collections.Generic.List[string]
    $compiled = 'no'
    foreach ($v in ($d.Versions -split ', ')) {
        $id = "$($d.Package)@$v"
        if (-not $graphIds.Contains($id)) { continue }
        $compiled = 'yes'
        try {
            $txt = cargo tree -i $id --workspace 2>$null | Out-String
            $rf = Join-Path $outDir 'reverse'
            New-Item -ItemType Directory -Force -Path $rf | Out-Null
            Set-Content -Path (Join-Path $rf ($id + '.txt')) -Encoding UTF8 -Value $txt
            $block = $false
            # Tree glyphs exist only in cargo output, not safely in this
            # source file (PS 5.1 reads scripts in the ANSI codepage), so
            # build the depth-1 detection from code points.
            $glyphV = [char]0x2502
            $glyphT = [char]0x251C
            $glyphL = [char]0x2514
            foreach ($l in ($txt -split "`n")) {
                if ($l -match ("^([A-Za-z0-9_.\-]+) v[0-9]")) {
                    $block = ($Matches[1] -eq $d.Package)
                    continue
                }
                if (-not $block) { continue }
                # Depth-1 lines are the package's direct dependents.
                $stripped = $l.TrimStart($glyphV, ' ')
                if ($stripped.Length -lt 4) { continue }
                if ($stripped[0] -ne $glyphT -and $stripped[0] -ne $glyphL) { continue }
                if ($stripped -notmatch '([A-Za-z0-9_.\-]+) v[0-9]') { continue }
                $dep = $Matches[1]
                if ($dep -ne $d.Package -and -not $dependents.Contains($dep)) { $dependents.Add($dep) }
            }
        } catch { }
    }
    $rev = if ($dependents.Count) { ($dependents | Select-Object -Unique) -join ', ' } else { '(external/crates.io)' }
    $rows += [pscustomobject]@{
        Package = $d.Package; Compiled = $compiled; Versions = $d.Versions; Count = $d.Count; Dependents = $rev
    }
    Write-Host ("  {0} [{1}] {2}" -f $d.Package, $compiled, $d.Versions)
}
$rows | Export-Csv -NoTypeInformation -Delimiter "`t" -Encoding UTF8 -Path (Join-Path $outDir 'duplicates-scan.tsv')

############################################
# 3. Unused deps per workspace package
############################################

Write-Host "=== unused deps ==="
$meta = (cargo metadata --no-deps --format-version 1 | Out-String | ConvertFrom-Json)
$memberIds = @{}
foreach ($id in $meta.workspace_members) { $memberIds.Add($id, $true) }
# The code token is the library target name (gpui-ce -> gpui; rust-i18n-macro
# -> rust_i18n_macro), not the package name.
$libName = @{}
foreach ($p in $meta.packages) {
    foreach ($t in $p.targets) {
        if (($t.kind -contains 'lib') -or ($t.kind -contains 'proc-macro')) {
            $libName[$p.name] = ($t.name -replace '-', '_')
            break
        }
    }
   if (-not $libName.ContainsKey($p.name)) { $libName[$p.name] = ($p.name -replace '-', '_') }
}

$report = New-Object System.Collections.Generic.List[string]
$report.Add('# Unused dependency scan (textual token check)')
$report.Add('')
$report.Add('Token = crate name with `-` -> `_`. Derive-only candidates are marked.')
$report.Add('A dep that exists only for its features or replaces a transitive one can')
$report.Add('textually appear unused but still be required; verify before removal.')
$report.Add('')
$pkgDirRe = [regex]'^(?<dir>.+)[/\\]Cargo\.toml$'
foreach ($p in $meta.packages) {
    if (-not $memberIds.ContainsKey($p.id)) { continue }
    $dir = $pkgDirRe.Match($p.manifest_path).Groups['dir'].Value
    $srcFiles = Get-ChildItem -LiteralPath $dir -Recurse -Filter '*.rs' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '[/\\](target|node_modules|\.git)([/\\]|$)' }
    $srcBlob = New-Object System.Text.StringBuilder
    foreach ($f in $srcFiles) { [void]$srcBlob.Append([System.IO.File]::ReadAllText($f.FullName)) }
    $src = $srcBlob.ToString()
    $unused = @()
    foreach ($d in $p.dependencies) {
        $kind = 'normal'
        if ($null -ne $d.kind) { $kind = $d.kind }
        $token = $d.name -replace '-', '_'
        if ($d.rename) { $token = ($d.rename -replace '-', '_') }
        elseif ($libName.ContainsKey($d.name)) { $token = $libName[$d.name] }
        # Packages outside the workspace (gpui-ce, helio forks, ...) have no
        # lib-name entry; try the plain package id too (gpui-ce ships `gpui`).
        $tokens = @($token)
        if (-not $d.rename) {
            $alt = ($d.name -replace '-ce$', '') -replace '-', '_'
            if ($alt -ne $token) { $tokens += $alt }
        }
        $used = $true
        if ($src) {
            $used = $false
            foreach ($tk in $tokens) {
                if ([regex]::IsMatch($src, ('\b' + [regex]::Escape($tk) + '\b'))) { $used = $true; break }
            }
        }
        if (-not $used) {
            $tag = ''
            if ($deriveOnly.Contains($d.name)) { $tag = ' [derive-only candidate]' }
            if ($d.target) { $tag += ' [target-only]' }
            $optional = if ($d.optional) { ' opt' } else { '' }
            $unused += ("    - {0}{1} ({2}){3}" -f $d.name, $optional, $kind, $tag)
        }
    }
    if ($unused.Count) {
        $rel = $dir.Substring($root.Length + 1)
        $report.Add("## $($p.name)  ($rel)")
        $report.Add($unused)
        $report.Add('')
    }
}
Set-Content -Path (Join-Path $outDir 'unused-deps.md') -Encoding UTF8 -Value $report
Write-Host "done -> $outDir"
