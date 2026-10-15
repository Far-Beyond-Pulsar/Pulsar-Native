#!/usr/bin/env bash
# Check the formatting of Pulsar-Native's own crates (#1375).
#
# `cargo fmt --all` also formats every path dependency, and most of those live
# in submodules (Helio, WGPUI, the vendored plugins): separate repositories
# with their own CI and their own formatting. This checks the workspace
# members that are not inside a submodule. The workspace manifest names
# members in submodules, so they must be checked out for cargo to load it.
#
#   scripts/fmt-check.sh         fail on any formatting difference
#   scripts/fmt-check.sh --fix   format them instead
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

submodules=$(git config --file .gitmodules --get-regexp '\.path$' | awk '{print $2}')
packages=()
while IFS=$'\t' read -r name manifest; do
  relative=${manifest#"$PWD"/}
  owned=1
  for submodule in $submodules; do
    case "$relative" in "$submodule"/*) owned=0 ;; esac
  done
  if [ "$owned" = 1 ]; then packages+=(-p "$name"); fi
done < <(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | [.name, .manifest_path] | @tsv')

echo "rustfmt: $(( ${#packages[@]} / 2 )) Pulsar-Native packages"
if [ "${1:-}" = "--fix" ]; then
  cargo fmt "${packages[@]}"
else
  cargo fmt "${packages[@]}" -- --check
fi
