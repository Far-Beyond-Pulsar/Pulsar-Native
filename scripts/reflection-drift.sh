#!/usr/bin/env bash
# Pulsar-Reflection: one source of truth, and a visible delta (#844).
#
# The monorepo builds Pulsar-Reflection from the vendored copy in
# crates/third-party/pulsar-reflection (workspace [patch] path overrides); the
# root manifest's git dependency names the upstream rev the copy is based on.
# Instead of letting the two drift silently, the exact difference between the
# vendored crates and that upstream rev is committed as
# crates/third-party/pulsar-reflection/UPSTREAM.patch. Any change to the
# vendored copy, or to the pinned rev, shows up in review as a change to that
# file, and CI fails until it is regenerated.
#
#   scripts/reflection-drift.sh check    fail if the delta differs from UPSTREAM.patch
#   scripts/reflection-drift.sh update   regenerate UPSTREAM.patch
#   scripts/reflection-drift.sh rev      print the upstream rev in use
#
# Bumping upstream: change the two `rev = "..."` Pulsar-Reflection pins in the
# root Cargo.toml, run `update`, and read the diff of UPSTREAM.patch: what
# upstream gained shows as removed lines, what is still local as added lines.
# Ideal end state: an empty patch, then drop the vendored copy for the git dep.
#
# PULSAR_REFLECTION_REPO overrides the upstream URL (e.g. a local checkout).
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
vendored="crates/third-party/pulsar-reflection"
patch_file="$root/$vendored/UPSTREAM.patch"
upstream_url="${PULSAR_REFLECTION_REPO:-https://github.com/Far-Beyond-Pulsar/Pulsar-Reflection}"

rev="$(grep -m1 -oE 'Pulsar-Reflection", rev = "[0-9a-f]{40}"' "$root/Cargo.toml" | grep -oE '[0-9a-f]{40}')"
[ -n "$rev" ] || { echo "error: no Pulsar-Reflection rev in Cargo.toml" >&2; exit 1; }

delta() {
  local work
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' RETURN
  git init -q "$work/up.git"
  git -C "$work/up.git" fetch -q --depth 1 "$upstream_url" "$rev"
  mkdir "$work/tree" "$work/tree/upstream" "$work/tree/vendored"
  git -C "$work/up.git" archive FETCH_HEAD crates | tar -x -C "$work/tree/upstream" --strip-components=1
  # Same files on both sides: sources and manifests, not build output.
  (cd "$root/$vendored" && tar -c --exclude=target --exclude=Cargo.lock --exclude=UPSTREAM.patch .) | tar -x -C "$work/tree/vendored"
  (cd "$work/tree" && git diff --no-index --no-color --ignore-cr-at-eol --src-prefix=upstream/ --dst-prefix=vendored/ -- upstream vendored || true) \
    | grep -v '^index ' | sed -E 's#(upstream|vendored)/(upstream|vendored)/#\1/#g'
}

case "${1:-}" in
  rev) echo "$rev" ;;
  update)
    { echo "# Vendored crates vs Pulsar-Reflection $rev (scripts/reflection-drift.sh update)"; delta; } > "$patch_file"
    echo "wrote $vendored/UPSTREAM.patch ($(wc -l < "$patch_file") lines) against $rev"
    ;;
  check)
    expected="$(tr -d '\r' < "$patch_file")"
    actual="$({ echo "# Vendored crates vs Pulsar-Reflection $rev (scripts/reflection-drift.sh update)"; delta; })"
    if [ "$expected" != "$actual" ]; then
      echo "error: the vendored Pulsar-Reflection no longer matches $vendored/UPSTREAM.patch." >&2
      echo "       If the change is intended, run scripts/reflection-drift.sh update and commit the patch." >&2
      diff <(echo "$expected") <(echo "$actual") | head -40 >&2 || true
      exit 1
    fi
    echo "pulsar-reflection: vendored copy matches UPSTREAM.patch (upstream $rev)"
    ;;
  *) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
