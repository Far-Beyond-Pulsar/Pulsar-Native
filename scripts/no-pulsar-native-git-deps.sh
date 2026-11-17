#!/usr/bin/env bash
# Fail if any manifest takes a git dependency on (or patches) Pulsar-Native.
#
# A `git = ".../Pulsar-Native"` dependency makes cargo fetch the whole engine
# repository with every submodule, some of them private, so anyone without
# access cannot build what depends on it: Helio, a plugin or a generated game
# (Pulsar-Native#1662). Plugins are built inside this workspace and use its
# crates through `workspace = true`; crates Pulsar shares with other
# repositories live in their own public repositories (Pulsar-Profiling,
# Pulsar-Reflection, WGPUI, ...), which this workspace patches onto its
# submodules.
#
#   scripts/no-pulsar-native-git-deps.sh     what CI runs
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
pattern='(git *= *"https://github\.com/+Far-Beyond-Pulsar/Pulsar-Native(\.git)?"|^\[patch\."https://github\.com/+Far-Beyond-Pulsar/Pulsar-Native(\.git)?"\])'

hits="$(find "$root" -name Cargo.toml -not -path '*/target/*' -not -path '*/node_modules/*' -not -path '*/.git/*' \
  -exec grep -HnE "$pattern" {} + 2>/dev/null || true)"
if [ -n "$hits" ]; then
  echo "error: git dependencies on Pulsar-Native (depend on the crate's own repository instead):" >&2
  echo "$hits" | sed "s#^$root/##" >&2
  exit 1
fi
echo "no git dependency on Pulsar-Native"
