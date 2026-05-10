#!/usr/bin/env bash

set -u

SKIP_TESTS=false
SKIP_AUDIT=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-tests)
      SKIP_TESTS=true
      shift
      ;;
    --skip-audit)
      SKIP_AUDIT=true
      shift
      ;;
    -h|--help)
      cat <<'EOF'
Pre-release validation script that mirrors the GitHub Actions release pipeline checks.

Usage:
  ./pre-release-check.sh [--skip-tests] [--skip-audit]

Options:
  --skip-tests   Skip running unit tests
  --skip-audit   Skip running cargo audit (non-blocking in pipeline)
  -h, --help     Show this help message
EOF
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      echo "Use --help for usage."
      exit 1
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORIGINAL_DIR="$PWD"

RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
BLUE='\033[34m'
CYAN='\033[36m'
RESET='\033[0m'

write_step() {
  echo -e "${CYAN}==>${RESET} ${BLUE}$1${RESET}"
}

write_success() {
  echo -e "${GREEN}✓${RESET} $1"
}

write_error() {
  echo -e "${RED}✗${RESET} $1"
}

write_warning() {
  echo -e "${YELLOW}⚠${RESET} $1"
}

exit_with_error() {
  write_error "$1"
  cd "$ORIGINAL_DIR" || true
  exit 1
}

run_quiet() {
  "$@" >/dev/null 2>&1
}

ALL_CHECKS_PASSED=true
FAILED_CHECKS=()
SHOULD_RELEASE=false
CURRENT_VERSION=""
PREVIOUS_VERSION=""

printf "\n"
echo -e "${CYAN}╔══════════════════════════════════════════════════════════════╗${RESET}"
echo -e "${CYAN}║${RESET}  ${BLUE}Pre-Release Validation Script${RESET}                            ${CYAN}║${RESET}"
echo -e "${CYAN}║${RESET}  Mirrors GitHub Actions release pipeline requirements     ${CYAN}║${RESET}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════╝${RESET}"
printf "\n"

cd "$SCRIPT_DIR" || exit_with_error "Failed to change to repository root"

# ============================================================================
# 1. VERSION CHECK
# ============================================================================
write_step "1/6 Checking version bump in crates/engine/Cargo.toml"

FIRST_CRATE="crates/engine"
CARGO_TOML="$FIRST_CRATE/Cargo.toml"

if [[ ! -f "$CARGO_TOML" ]]; then
  exit_with_error "Could not find $CARGO_TOML"
fi

CURRENT_VERSION="$(grep -E '^version\s*=\s*"[^"]+"' "$CARGO_TOML" | head -n 1 | sed -E 's/^version\s*=\s*"([^"]+)".*/\1/')"

if [[ -z "$CURRENT_VERSION" ]]; then
  exit_with_error "Could not parse current version from $CARGO_TOML"
fi

PREV_FILE="$(mktemp 2>/dev/null || echo "/tmp/prev_cargo.toml.$$")"
if git show "HEAD^1:$CARGO_TOML" >"$PREV_FILE" 2>/dev/null; then
  PREVIOUS_VERSION="$(grep -E '^version\s*=\s*"[^"]+"' "$PREV_FILE" | head -n 1 | sed -E 's/^version\s*=\s*"([^"]+)".*/\1/')"
  rm -f "$PREV_FILE"

  if [[ "$CURRENT_VERSION" != "$PREVIOUS_VERSION" ]]; then
    write_success "Version changed: $PREVIOUS_VERSION -> $CURRENT_VERSION"
    SHOULD_RELEASE=true
  else
    write_warning "Version unchanged: $CURRENT_VERSION (release pipeline would skip)"
    SHOULD_RELEASE=false
  fi
else
  rm -f "$PREV_FILE"
  write_warning "Could not get previous version (might be first commit). Current version: $CURRENT_VERSION"
  SHOULD_RELEASE=false
fi

if [[ "$SHOULD_RELEASE" != true ]]; then
  printf "\n"
  echo -e "${YELLOW}Note: Release pipeline only runs when version changes in $CARGO_TOML${RESET}"
  echo -e "${YELLOW}Continuing with validation checks anyway...${RESET}"
  printf "\n"
fi

# ============================================================================
# 2. FORMATTING CHECK
# ============================================================================
write_step "2/6 Checking code formatting (cargo fmt --all -- --check)"

if run_quiet cargo fmt --all -- --check; then
  write_success "Code formatting is correct"
else
  write_error "Code formatting check failed. Run 'cargo fmt --all' to fix."
  ALL_CHECKS_PASSED=false
  FAILED_CHECKS+=("Formatting")
fi

# ============================================================================
# 3. CLIPPY
# ============================================================================
write_step "3/6 Running clippy with release pipeline configuration"

echo "    Running: cargo clippy --workspace --exclude pulsar_docs --all-targets"

if cargo clippy \
  --workspace \
  --exclude pulsar_docs \
  --all-targets \
  -- \
  -A warnings \
  -A dead_code \
  -A clippy::too_many_arguments \
  -A clippy::type_complexity \
  -A clippy::match_like_matches_macro \
  -A clippy::only_used_in_recursion \
  -A improper_ctypes_definitions \
  -A clippy::field_reassign_with_default \
  -A clippy::result_large_err \
  -A clippy::doc_overindented_list_items; then
  write_success "Clippy checks passed"
else
  write_error "Clippy found issues"
  ALL_CHECKS_PASSED=false
  FAILED_CHECKS+=("Clippy")
fi

# ============================================================================
# 4. UNIT TESTS
# ============================================================================
if [[ "$SKIP_TESTS" != true ]]; then
  write_step "4/6 Running pre-test cleanup (cargo clean + cache removal) and unit tests"

  CLEANUP_FAILED=false

  # Force a clean baseline for test execution.
  if cargo clean; then
    write_success "cargo clean completed"
  else
    write_error "cargo clean failed"
    CLEANUP_FAILED=true
  fi

  CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
  CACHE_PATHS=(
    "$CARGO_HOME_DIR/registry/cache"
    "$CARGO_HOME_DIR/registry/index"
    "$CARGO_HOME_DIR/registry/src"
    "$CARGO_HOME_DIR/git/db"
    "$CARGO_HOME_DIR/git/checkouts"
  )

  echo "    Removing Cargo caches in: $CARGO_HOME_DIR"
  for cache_path in "${CACHE_PATHS[@]}"; do
    if [[ -e "$cache_path" ]]; then
      if rm -rf "$cache_path"; then
        write_success "Removed cache path: $cache_path"
      else
        write_warning "Could not remove cache path: $cache_path"
        CLEANUP_FAILED=true
      fi
    fi
  done

  if [[ "$CLEANUP_FAILED" == true ]]; then
    write_error "Pre-test cleanup encountered errors"
    ALL_CHECKS_PASSED=false
    FAILED_CHECKS+=("Pre-test cleanup")
  fi

  if ! run_quiet cargo nextest --version; then
    echo "    Installing cargo-nextest..."
    if ! cargo install cargo-nextest --locked; then
      exit_with_error "Failed to install cargo-nextest"
    fi
  fi

  if cargo nextest run --workspace --locked; then
    write_success "All tests passed"
  else
    write_error "Tests failed"
    ALL_CHECKS_PASSED=false
    FAILED_CHECKS+=("Tests")
  fi
else
  write_step "4/6 Skipping unit tests (--skip-tests flag used)"
fi

# ============================================================================
# 5. CARGO.LOCK CHECK
# ============================================================================
write_step "5/6 Validating Cargo.lock is up to date"

if run_quiet cargo metadata --locked --format-version 1; then
  GIT_STATUS="$(git status --porcelain Cargo.lock 2>&1 || true)"
  if [[ -z "$GIT_STATUS" ]]; then
    write_success "Cargo.lock is up to date and committed"
  else
    write_error "Cargo.lock has uncommitted changes. Commit Cargo.lock before releasing."
    echo "    Current status: $GIT_STATUS"
    ALL_CHECKS_PASSED=false
    FAILED_CHECKS+=("Cargo.lock")
  fi
else
  write_error "Cargo.lock is out of date with Cargo.toml manifests."
  echo "    Run 'cargo update -w' to update the lockfile, then commit."
  ALL_CHECKS_PASSED=false
  FAILED_CHECKS+=("Cargo.lock")
fi

# ============================================================================
# 6. CARGO AUDIT (non-blocking like in the pipeline)
# ============================================================================
if [[ "$SKIP_AUDIT" != true ]]; then
  write_step "6/6 Running security audit (cargo audit) - non-blocking"

  AUDIT_INSTALLED=true
  if ! run_quiet cargo audit --version; then
    AUDIT_INSTALLED=false
  fi

  if [[ "$AUDIT_INSTALLED" != true ]]; then
    echo "    Installing cargo-audit..."
    if run_quiet cargo install cargo-audit --locked; then
      AUDIT_INSTALLED=true
    else
      write_warning "Failed to install cargo-audit, skipping security audit"
    fi
  fi

  if [[ "$AUDIT_INSTALLED" == true ]]; then
    AUDIT_OUTPUT=""
    if AUDIT_OUTPUT="$(cargo audit 2>&1)"; then
      write_success "No security vulnerabilities found"
    else
      write_warning "Security audit found issues (non-blocking):"
      echo "$AUDIT_OUTPUT"
      printf "\n"
      echo -e "${YELLOW}Note: cargo audit failures don't block releases in the pipeline${RESET}"
    fi
  fi
else
  write_step "6/6 Skipping security audit (--skip-audit flag used)"
fi

# ============================================================================
# SUMMARY
# ============================================================================
printf "\n"
echo -e "${CYAN}╔══════════════════════════════════════════════════════════════╗${RESET}"
echo -e "${CYAN}║${RESET}  ${BLUE}Summary${RESET}                                                   ${CYAN}║${RESET}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════════╝${RESET}"
printf "\n"

if [[ "$ALL_CHECKS_PASSED" == true ]]; then
  echo -e "${GREEN}✓ All checks passed!${RESET}"
  printf "\n"

  if [[ "$SHOULD_RELEASE" == true ]]; then
    echo -e "${GREEN}This release would proceed in the pipeline (version bumped: $CURRENT_VERSION)${RESET}"
  else
    echo -e "${YELLOW}Note: Release pipeline would skip (no version change detected)${RESET}"
    echo -e "${YELLOW}To trigger a release, update the version in $CARGO_TOML${RESET}"
  fi

  printf "\n"
  cd "$ORIGINAL_DIR" || true
  exit 0
else
  echo -e "${RED}✗ Some checks failed:${RESET}"
  for check in "${FAILED_CHECKS[@]}"; do
    echo -e "  ${RED}•${RESET} $check"
  done
  printf "\n"
  echo -e "${RED}Fix these issues before pushing for release.${RESET}"
  printf "\n"
  cd "$ORIGINAL_DIR" || true
  exit 1
fi
