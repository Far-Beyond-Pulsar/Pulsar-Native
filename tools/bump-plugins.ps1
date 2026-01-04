#!/usr/bin/env pwsh
# Plugin Version Bumper
# Updates all plugin repos with new engine hash and triggers releases

param(
    [Parameter(Mandatory=$true)]
    [string]$EngineHash,
    
    [Parameter(Mandatory=$false)]
    [string]$PluginsDir = ".",
    
    [Parameter(Mandatory=$false)]
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

# Colors for output
function Write-Success { Write-Host "✓ $args" -ForegroundColor Green }
function Write-Info { Write-Host "→ $args" -ForegroundColor Cyan }
function Write-Warn { Write-Host "⚠ $args" -ForegroundColor Yellow }
function Write-Err { Write-Host "✗ $args" -ForegroundColor Red }

# Validate hash format
if ($EngineHash -notmatch '^[0-9a-f]{40}$') {
    Write-Err "Invalid git hash format. Expected 40 character hex string."
    exit 1
}

Write-Info "Plugin Update Script"
Write-Info "Engine Hash: $EngineHash"
Write-Info "Plugins Directory: $PluginsDir"
if ($DryRun) {
    Write-Warn "DRY RUN MODE - No changes will be committed"
}
Write-Host ""

# Find all plugin directories
$pluginDirs = Get-ChildItem -Path $PluginsDir -Directory | Where-Object { $_.Name -like "plugin_*" }

if ($pluginDirs.Count -eq 0) {
    Write-Warn "No plugin directories found (looking for folders starting with 'plugin_')"
    exit 0
}

Write-Info "Found $($pluginDirs.Count) plugin(s):"
foreach ($dir in $pluginDirs) {
    Write-Host "  - $($dir.Name)"
}
Write-Host ""

# Function to update Cargo.toml hash
function Update-EngineHash {
    param(
        [string]$Path,
        [string]$NewHash
    )
    
    $content = Get-Content -Path $Path -Raw
    $updated = $false
    
    # Pattern to match Pulsar-Native git dependencies
    $pattern = '(git\s*=\s*"https://github\.com/Far-Beyond-Pulsar/Pulsar-Native"[^}]*rev\s*=\s*")[0-9a-f]{40}(")'
    
    if ($content -match $pattern) {
        $newContent = $content -replace $pattern, "`${1}$NewHash`${2}"
        
        if ($newContent -ne $content) {
            Set-Content -Path $Path -Value $newContent -NoNewline
            $updated = $true
        }
    }
    
    return $updated
}

# Function to bump version in Cargo.toml
function Bump-Version {
    param(
        [string]$Path
    )
    
    $content = Get-Content -Path $Path -Raw
    
    # Find first version = "x.y.z" (the package version)
    if ($content -match 'version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') {
        $major = [int]$matches[1]
        $minor = [int]$matches[2]
        $patch = [int]$matches[3]
        
        # Bump patch version
        $patch++
        $newVersion = "$major.$minor.$patch"
        
        # Replace first occurrence only
        $lines = $content -split "`n"
        $replaced = $false
        
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if (-not $replaced -and $lines[$i] -match '^version\s*=\s*"(\d+\.\d+\.\d+)"') {
                $lines[$i] = $lines[$i] -replace '"(\d+\.\d+\.\d+)"', "`"$newVersion`""
                $replaced = $true
                break
            }
        }
        
        if ($replaced) {
            $newContent = $lines -join "`n"
            Set-Content -Path $Path -Value $newContent -NoNewline
            return $newVersion
        }
    }
    
    return $null
}

# Process each plugin
$successCount = 0
$failCount = 0

foreach ($pluginDir in $pluginDirs) {
    Write-Info "Processing $($pluginDir.Name)..."
    
    $cargoTomlPath = Join-Path $pluginDir.FullName "Cargo.toml"
    
    # Check if Cargo.toml exists
    if (-not (Test-Path $cargoTomlPath)) {
        Write-Warn "  No Cargo.toml found, skipping"
        continue
    }
    
    # Save current directory
    $originalDir = Get-Location
    
    try {
        Set-Location $pluginDir.FullName
        
        # Check if it's a git repo
        if (-not (Test-Path ".git")) {
            Write-Warn "  Not a git repository, skipping"
            continue
        }
        
        # Check if Cargo.toml specifically has uncommitted changes
        $cargoStatus = git status --porcelain Cargo.toml
        if ($cargoStatus -and $cargoStatus -notmatch '^\?\?') {
            Write-Warn "  Cargo.toml has uncommitted changes, skipping"
            Write-Host "    $cargoStatus"
            continue
        }
        
        # STEP 1: Update engine hash
        Write-Info "  Updating engine hash..."
        $hashUpdated = Update-EngineHash -Path $cargoTomlPath -NewHash $EngineHash
        
        if (-not $hashUpdated) {
            Write-Warn "  No Pulsar-Native dependencies found or already up to date"
            # Don't skip - continue to version bump in case that needs doing
        } else {
            Write-Success "  Engine hash updated"
            
            if (-not $DryRun) {
                # Commit and push hash update
                git add Cargo.toml
                git commit -m "bumped engine version"
                git push
                Write-Success "  Committed and pushed 'bumped engine version'"
            } else {
                Write-Info "  [DRY RUN] Would commit 'bumped engine version' and push"
            }
        }
        
        # STEP 2: Bump crate version (always try, even if hash update was skipped)
        Write-Info "  Bumping crate version..."
        $newVersion = Bump-Version -Path $cargoTomlPath
        
        if ($newVersion) {
            Write-Success "  Version bumped to $newVersion"
            
            if (-not $DryRun) {
                # Commit and push version bump (triggers GitHub Actions release)
                git add Cargo.toml
                git commit -m "bump version to $newVersion"
                git push
                Write-Success "  Committed and pushed version bump (triggers release)"
            } else {
                Write-Info "  [DRY RUN] Would commit 'bump version to $newVersion' and push"
                # Restore in dry run
                git checkout Cargo.toml
            }
            
            $successCount++
        } else {
            Write-Warn "  Failed to bump version or already at latest"
            # Check if we at least updated the hash
            if ($hashUpdated) {
                $successCount++
            } else {
                $failCount++
            }
        }
        
        Write-Host ""
        
    } catch {
        Write-Err "  Failed: $_"
        if (-not $DryRun) {
            # Try to rollback on error
            try {
                git reset --hard HEAD~1 2>$null
                git push --force 2>$null
            } catch {
                Write-Warn "  Could not rollback changes"
            }
        }
        $failCount++
        Write-Host ""
    } finally {
        Set-Location $originalDir
    }
}

# Summary
Write-Host "=" * 50
Write-Info "Summary:"
Write-Success "  Successful: $successCount"
if ($failCount -gt 0) {
    Write-Err "  Failed: $failCount"
}

if ($DryRun) {
    Write-Warn "DRY RUN completed - no changes were made"
} else {
    Write-Success "All plugins updated!"
    Write-Info "GitHub Actions should now build fresh releases"
}

exit 0
