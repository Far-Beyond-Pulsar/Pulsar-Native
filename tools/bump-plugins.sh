#!/usr/bin/env bash
# Plugin Version Bumper
# Updates all plugin repos with new engine hash and triggers releases

set -e

# Colors for output
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

function write_success() { echo -e "${GREEN}✓ $*${NC}"; }
function write_info() { echo -e "${CYAN}→ $*${NC}"; }
function write_warn() { echo -e "${YELLOW}⚠ $*${NC}"; }
function write_error() { echo -e "${RED}✗ $*${NC}"; }

# Parse arguments
ENGINE_HASH=""
PLUGINS_DIR="."
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--hash)
            ENGINE_HASH="$2"
            shift 2
            ;;
        -d|--dir)
            PLUGINS_DIR="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        *)
            write_error "Unknown option: $1"
            echo "Usage: $0 --hash <engine_hash> [--dir <plugins_dir>] [--dry-run]"
            exit 1
            ;;
    esac
done

# Validate hash
if [[ -z "$ENGINE_HASH" ]]; then
    write_error "Engine hash is required"
    echo "Usage: $0 --hash <engine_hash> [--dir <plugins_dir>] [--dry-run]"
    exit 1
fi

if [[ ! "$ENGINE_HASH" =~ ^[0-9a-f]{40}$ ]]; then
    write_error "Invalid git hash format. Expected 40 character hex string."
    exit 1
fi

write_info "Plugin Update Script"
write_info "Engine Hash: $ENGINE_HASH"
write_info "Plugins Directory: $PLUGINS_DIR"
if $DRY_RUN; then
    write_warn "DRY RUN MODE - No changes will be committed"
fi
echo ""

# Find all plugin directories
shopt -s nullglob
PLUGIN_DIRS=("$PLUGINS_DIR"/plugin_*)

if [[ ${#PLUGIN_DIRS[@]} -eq 0 ]]; then
    write_warn "No plugin directories found (looking for folders starting with 'plugin_')"
    exit 0
fi

write_info "Found ${#PLUGIN_DIRS[@]} plugin(s):"
for dir in "${PLUGIN_DIRS[@]}"; do
    echo "  - $(basename "$dir")"
done
echo ""

# Function to update Cargo.toml hash
update_engine_hash() {
    local cargo_toml="$1"
    local new_hash="$2"
    
    # Check if file contains Pulsar-Native dependencies
    if ! grep -q 'git = "https://github.com/Far-Beyond-Pulsar/Pulsar-Native"' "$cargo_toml"; then
        return 1
    fi
    
    # Use sed to replace hash
    # Pattern matches: rev = "40-char-hash"
    local temp_file="${cargo_toml}.tmp"
    sed -E 's|(git = "https://github.com/Far-Beyond-Pulsar/Pulsar-Native"[^}]*rev = ")[0-9a-f]{40}("|\)|\}|\])|\1'"$new_hash"'\2|g' "$cargo_toml" > "$temp_file"
    
    # Check if anything changed
    if cmp -s "$cargo_toml" "$temp_file"; then
        rm "$temp_file"
        return 1
    fi
    
    mv "$temp_file" "$cargo_toml"
    return 0
}

# Function to bump version in Cargo.toml
bump_version() {
    local cargo_toml="$1"
    
    # Extract current version (first occurrence)
    local current_version=$(grep -m 1 '^version = ' "$cargo_toml" | sed -E 's/version = "([0-9]+\.[0-9]+\.[0-9]+)"/\1/')
    
    if [[ -z "$current_version" ]]; then
        return 1
    fi
    
    # Parse version
    IFS='.' read -r major minor patch <<< "$current_version"
    
    # Bump patch version
    patch=$((patch + 1))
    local new_version="${major}.${minor}.${patch}"
    
    # Replace first occurrence only
    sed -i.bak "0,/^version = \"[0-9]*\.[0-9]*\.[0-9]*\"/s//version = \"$new_version\"/" "$cargo_toml"
    rm "${cargo_toml}.bak"
    
    echo "$new_version"
}

# Process each plugin
success_count=0
fail_count=0

for plugin_dir in "${PLUGIN_DIRS[@]}"; do
    plugin_name=$(basename "$plugin_dir")
    write_info "Processing $plugin_name..."
    
    cargo_toml="$plugin_dir/Cargo.toml"
    
    # Check if Cargo.toml exists
    if [[ ! -f "$cargo_toml" ]]; then
        write_warn "  No Cargo.toml found, skipping"
        continue
    fi
    
    # Save current directory
    original_dir=$(pwd)
    
    # Change to plugin directory
    cd "$plugin_dir" || continue
    
    # Check if it's a git repo
    if [[ ! -d ".git" ]]; then
        write_warn "  Not a git repository, skipping"
        cd "$original_dir"
        continue
    fi
    
    # Check if Cargo.toml has uncommitted changes
    cargo_status=$(git status --porcelain Cargo.toml)
    if [[ -n "$cargo_status" ]] && [[ ! "$cargo_status" =~ ^\?\? ]]; then
        write_warn "  Cargo.toml has uncommitted changes, skipping"
        echo "    $cargo_status"
        cd "$original_dir"
        continue
    fi
    
    hash_updated=false
    
    # STEP 1: Update engine hash
    write_info "  Updating engine hash..."
    if update_engine_hash "$cargo_toml" "$ENGINE_HASH"; then
        write_success "  Engine hash updated"
        hash_updated=true
        
        if ! $DRY_RUN; then
            # Commit and push hash update
            git add Cargo.toml
            git commit -m "bumped engine version"
            git push
            write_success "  Committed and pushed 'bumped engine version'"
        else
            write_info "  [DRY RUN] Would commit 'bumped engine version' and push"
        fi
    else
        write_warn "  No Pulsar-Native dependencies found or already up to date"
        # Don't skip - continue to version bump in case that needs doing
    fi
    
    # STEP 2: Bump crate version (always try, even if hash update was skipped)
    write_info "  Bumping crate version..."
    new_version=$(bump_version "$cargo_toml")
    
    if [[ -n "$new_version" ]]; then
        write_success "  Version bumped to $new_version"
        
        if ! $DRY_RUN; then
            # Commit and push version bump (triggers GitHub Actions release)
            git add Cargo.toml
            git commit -m "bump version to $new_version"
            git push
            write_success "  Committed and pushed version bump (triggers release)"
        else
            write_info "  [DRY RUN] Would commit 'bump version to $new_version' and push"
            # Restore in dry run
            git checkout Cargo.toml
        fi
        
        ((success_count++))
    else
        write_warn "  Failed to bump version or already at latest"
        # Check if we at least updated the hash
        if $hash_updated; then
            ((success_count++))
        else
            ((fail_count++))
        fi
    fi
    
    echo ""
    cd "$original_dir"
done

# Summary
echo "=================================================="
write_info "Summary:"
write_success "  Successful: $success_count"
if [[ $fail_count -gt 0 ]]; then
    write_error "  Failed: $fail_count"
fi

if $DRY_RUN; then
    write_warn "DRY RUN completed - no changes were made"
else
    write_success "All plugins updated!"
    write_info "GitHub Actions should now build fresh releases"
fi

exit 0
