#!/usr/bin/env bash
# Keep the vendored plugins' Pulsar-Native git pins in step with this repo (#847).
#
# Each plugin under plugins/vendor/ depends on Pulsar-Native crates through
# `git = ".../Pulsar-Native", rev = "<sha>"`. An API change here means every
# one of those revs has to move; this does it in one step and checks it.
#
#   scripts/plugin-pins.sh bump [REV]   rewrite every pin to REV (default HEAD),
#                                       and refresh each plugin's Cargo.lock
#   scripts/plugin-pins.sh check        fail unless every pin is an ancestor of
#                                       HEAD (needs full history: fetch-depth 0)
#   scripts/plugin-pins.sh list         print the distinct pins in use
#
# Commit the plugin submodule change first; the pins must name a commit that
# is already published, or standalone plugin builds cannot fetch it.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
repo_url='github.com/Far-Beyond-Pulsar/Pulsar-Native'
pin_re="git = \"https://${repo_url}(\.git)?\", rev = \"[0-9a-f]{7,40}\""

manifests() {
  find "$root/plugins/vendor" -name Cargo.toml -not -path '*/target/*' -not -path '*/node_modules/*' \
    -exec grep -lE "$pin_re" {} + 2>/dev/null | sort
}

pins() {
  manifests | xargs -r grep -hoE "$pin_re" | grep -oE '[0-9a-f]{7,40}"$' | tr -d '"' | sort -u
}

cmd="${1:-}"
case "$cmd" in
  list)
    pins
    ;;
  bump)
    rev="$(git -C "$root" rev-parse "${2:-HEAD}")"
    for m in $(manifests); do
      sed -i -E "s#(git = \"https://${repo_url}(\.git)?\", rev = \")[0-9a-f]{7,40}\"#\1${rev}\"#g" "$m"
      echo "pinned $(realpath --relative-to="$root" "$m") -> $rev"
    done
    # A changed rev is a new source to cargo, so resolving re-pins the lockfile
    # (`cargo update -p` is ambiguous while a lock still holds two generations).
    for m in $(manifests); do
      dir="$(dirname "$m")"
      [ -f "$dir/Cargo.lock" ] || continue
      (cd "$dir" && cargo metadata --format-version 1 > /dev/null) || echo "warning: could not refresh $dir/Cargo.lock (network?); run cargo metadata there" >&2
    done
    ;;
  check)
    head="$(git -C "$root" rev-parse HEAD)"
    status=0
    for pin in $(pins); do
      if ! git -C "$root" cat-file -e "${pin}^{commit}" 2>/dev/null; then
        echo "error: plugin pin $pin is not a commit in this repository's history" >&2
        status=1
      elif ! git -C "$root" merge-base --is-ancestor "$pin" "$head"; then
        echo "error: plugin pin $pin is not an ancestor of HEAD ($head)" >&2
        status=1
      fi
    done
    [ $status -eq 0 ] && echo "plugin pins: $(pins | wc -l) distinct, all ancestors of HEAD"
    exit $status
    ;;
  *)
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
