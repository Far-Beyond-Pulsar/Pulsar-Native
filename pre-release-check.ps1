#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Pre-release validation script that mirrors the GitHub Actions release pipeline checks.

.DESCRIPTION
    This script runs all the same checks that the release pipeline runs, allowing you to
    validate locally before pushing. It checks:
    - Version bump in crates/engine/Cargo.toml
    - Code formatting (cargo fmt)
    - Clippy lints
    - Unit tests (cargo nextest)
    - Cargo.lock up-to-date
    - Security audit (cargo audit)

.PARAMETER SkipTests
    Skip running the unit tests (useful for faster iteration)

.PARAMETER SkipAudit
    Skip running cargo audit (it's non-blocking in the pipeline anyway)

.EXAMPLE
    .\pre-release-check.ps1
    .\pre-release-check.ps1 -SkipTests
#>

param(
    [switch]$SkipTests,
    [switch]$SkipAudit
)

$ErrorActionPreference = 'Stop'
$OriginalLocation = Get-Location

# ANSI color codes for better output
$Red = "`e[31m"
$Green = "`e[32m"
$Yellow = "`e[33m"
$Blue = "`e[34m"
$Cyan = "`e[36m"
$Reset = "`e[0m"

function Write-Step {
    param([string]$Message)
    Write-Host "${Cyan}==>${Reset} ${Blue}$Message${Reset}"
}

function Write-Success {
    param([string]$Message)
    Write-Host "${Green}✓${Reset} $Message"
}

function Write-Error {
    param([string]$Message)
    Write-Host "${Red}✗${Reset} $Message" -ForegroundColor Red
}

function Write-Warning {
    param([string]$Message)
    Write-Host "${Yellow}⚠${Reset} $Message"
}

function Exit-WithError {
    param([string]$Message)
    Write-Error $Message
    Set-Location $OriginalLocation
    exit 1
}

# Track overall success
$AllChecksPassed = $true
$FailedChecks = @()

Write-Host ""
Write-Host "${Cyan}╔══════════════════════════════════════════════════════════════╗${Reset}"
Write-Host "${Cyan}║${Reset}  ${Blue}Pre-Release Validation Script${Reset}                            ${Cyan}║${Reset}"
Write-Host "${Cyan}║${Reset}  Mirrors GitHub Actions release pipeline requirements     ${Cyan}║${Reset}"
Write-Host "${Cyan}╚══════════════════════════════════════════════════════════════╝${Reset}"
Write-Host ""

# Change to repository root
Set-Location $PSScriptRoot

# ============================================================================
# 1. VERSION CHECK
# ============================================================================
Write-Step "1/6 Checking version bump in crates/engine/Cargo.toml"

$FirstCrate = "crates/engine"
$CargoToml = "$FirstCrate/Cargo.toml"

if (-not (Test-Path $CargoToml)) {
    Exit-WithError "Could not find $CargoToml"
}

# Get current version
$CurrentVersion = (Select-String -Path $CargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value

# Get previous version from HEAD^1
try {
    git show "HEAD^1:$CargoToml" > "$env:TEMP\prev_cargo.toml" 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "Could not get previous version (might be first commit). Current version: $CurrentVersion"
        $ShouldRelease = $false
    } else {
        $PreviousVersion = (Select-String -Path "$env:TEMP\prev_cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
        Remove-Item "$env:TEMP\prev_cargo.toml" -ErrorAction SilentlyContinue

        if ($CurrentVersion -ne $PreviousVersion) {
            Write-Success "Version changed: $PreviousVersion → $CurrentVersion"
            $ShouldRelease = $true
        } else {
            Write-Warning "Version unchanged: $CurrentVersion (release pipeline would skip)"
            $ShouldRelease = $false
        }
    }
} catch {
    Write-Warning "Could not compare versions: $_"
    $ShouldRelease = $false
}

if (-not $ShouldRelease) {
    Write-Host ""
    Write-Host "${Yellow}Note: Release pipeline only runs when version changes in $CargoToml${Reset}"
    Write-Host "${Yellow}Continuing with validation checks anyway...${Reset}"
    Write-Host ""
}

# ============================================================================
# 2. FORMATTING CHECK
# ============================================================================
Write-Step "2/6 Checking code formatting (cargo fmt --all -- --check)"

try {
    cargo fmt --all -- --check 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Write-Success "Code formatting is correct"
    } else {
        Write-Error "Code formatting check failed. Run 'cargo fmt --all' to fix."
        $AllChecksPassed = $false
        $FailedChecks += "Formatting"
    }
} catch {
    Write-Error "Failed to run cargo fmt: $_"
    $AllChecksPassed = $false
    $FailedChecks += "Formatting"
}

# ============================================================================
# 3. CLIPPY
# ============================================================================
Write-Step "3/6 Running clippy with release pipeline configuration"

Write-Host "    Running: cargo clippy --workspace --exclude pulsar_docs --all-targets"

try {
    $ClippyArgs = @(
        'clippy',
        '--workspace',
        '--exclude', 'pulsar_docs',
        '--all-targets',
        '--',
        '-A', 'warnings',
        '-A', 'dead_code',
        '-A', 'clippy::too_many_arguments',
        '-A', 'clippy::type_complexity',
        '-A', 'clippy::match_like_matches_macro',
        '-A', 'clippy::only_used_in_recursion',
        '-A', 'improper_ctypes_definitions',
        '-A', 'clippy::field_reassign_with_default',
        '-A', 'clippy::result_large_err',
        '-A', 'clippy::doc_overindented_list_items'
    )

    & cargo $ClippyArgs

    if ($LASTEXITCODE -eq 0) {
        Write-Success "Clippy checks passed"
    } else {
        Write-Error "Clippy found issues"
        $AllChecksPassed = $false
        $FailedChecks += "Clippy"
    }
} catch {
    Write-Error "Failed to run clippy: $_"
    $AllChecksPassed = $false
    $FailedChecks += "Clippy"
}

# ============================================================================
# 4. UNIT TESTS
# ============================================================================
if (-not $SkipTests) {
    Write-Step "4/6 Running unit tests (cargo nextest run --workspace --locked)"

    # Check if cargo-nextest is installed
    $NextestInstalled = $false
    try {
        cargo nextest --version 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) {
            $NextestInstalled = $true
        }
    } catch {
        $NextestInstalled = $false
    }

    if (-not $NextestInstalled) {
        Write-Host "    Installing cargo-nextest..."
        cargo install cargo-nextest --locked
        if ($LASTEXITCODE -ne 0) {
            Exit-WithError "Failed to install cargo-nextest"
        }
    }

    try {
        cargo nextest run --workspace --locked

        if ($LASTEXITCODE -eq 0) {
            Write-Success "All tests passed"
        } else {
            Write-Error "Tests failed"
            $AllChecksPassed = $false
            $FailedChecks += "Tests"
        }
    } catch {
        Write-Error "Failed to run tests: $_"
        $AllChecksPassed = $false
        $FailedChecks += "Tests"
    }
} else {
    Write-Step "4/6 Skipping unit tests (--SkipTests flag used)"
}

# ============================================================================
# 5. CARGO.LOCK CHECK
# ============================================================================
Write-Step "5/6 Validating Cargo.lock is up to date"

try {
    # Check if Cargo.lock is in sync with Cargo.toml files
    cargo metadata --locked --format-version 1 > $null 2>&1

    if ($LASTEXITCODE -eq 0) {
        # Check if Cargo.lock has uncommitted changes
        $GitStatus = git status --porcelain Cargo.lock 2>&1

        if ([string]::IsNullOrEmpty($GitStatus)) {
            Write-Success "Cargo.lock is up to date and committed"
        } else {
            Write-Error "Cargo.lock has uncommitted changes. Commit Cargo.lock before releasing."
            Write-Host "    Current status: $GitStatus"
            $AllChecksPassed = $false
            $FailedChecks += "Cargo.lock"
        }
    } else {
        Write-Error "Cargo.lock is out of date with Cargo.toml manifests."
        Write-Host "    Run 'cargo update -w' to update the lockfile, then commit."
        $AllChecksPassed = $false
        $FailedChecks += "Cargo.lock"
    }
} catch {
    Write-Error "Failed to check Cargo.lock: $_"
    $AllChecksPassed = $false
    $FailedChecks += "Cargo.lock"
}

# ============================================================================
# 6. CARGO AUDIT (non-blocking like in the pipeline)
# ============================================================================
if (-not $SkipAudit) {
    Write-Step "6/6 Running security audit (cargo audit) - non-blocking"

    # Check if cargo-audit is installed
    $AuditInstalled = $false
    try {
        cargo audit --version 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) {
            $AuditInstalled = $true
        }
    } catch {
        $AuditInstalled = $false
    }

    if (-not $AuditInstalled) {
        Write-Host "    Installing cargo-audit..."
        cargo install cargo-audit --locked 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Failed to install cargo-audit, skipping security audit"
        } else {
            $AuditInstalled = $true
        }
    }

    if ($AuditInstalled) {
        try {
            $AuditOutput = cargo audit 2>&1
            $AuditExitCode = $LASTEXITCODE

            if ($AuditExitCode -eq 0) {
                Write-Success "No security vulnerabilities found"
            } else {
                Write-Warning "Security audit found issues (non-blocking):"
                Write-Host $AuditOutput
                Write-Host ""
                Write-Host "${Yellow}Note: cargo audit failures don't block releases in the pipeline${Reset}"
            }
        } catch {
            Write-Warning "Failed to run cargo audit: $_"
        }
    }
} else {
    Write-Step "6/6 Skipping security audit (--SkipAudit flag used)"
}

# ============================================================================
# SUMMARY
# ============================================================================
Write-Host ""
Write-Host "${Cyan}╔══════════════════════════════════════════════════════════════╗${Reset}"
Write-Host "${Cyan}║${Reset}  ${Blue}Summary${Reset}                                                   ${Cyan}║${Reset}"
Write-Host "${Cyan}╚══════════════════════════════════════════════════════════════╝${Reset}"
Write-Host ""

if ($AllChecksPassed) {
    Write-Host "${Green}✓ All checks passed!${Reset}"
    Write-Host ""

    if ($ShouldRelease) {
        Write-Host "${Green}This release would proceed in the pipeline (version bumped: $CurrentVersion)${Reset}"
    } else {
        Write-Host "${Yellow}Note: Release pipeline would skip (no version change detected)${Reset}"
        Write-Host "${Yellow}To trigger a release, update the version in $CargoToml${Reset}"
    }

    Write-Host ""
    exit 0
} else {
    Write-Host "${Red}✗ Some checks failed:${Reset}"
    foreach ($Check in $FailedChecks) {
        Write-Host "  ${Red}•${Reset} $Check"
    }
    Write-Host ""
    Write-Host "${Red}Fix these issues before pushing for release.${Reset}"
    Write-Host ""
    exit 1
}
