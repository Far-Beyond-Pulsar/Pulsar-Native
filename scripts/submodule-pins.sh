#!/usr/bin/env bash
# Keep every git submodule pinned to a commit on its upstream default branch.
#
# A Pulsar-Native PR that pins a dependency's feature branch (Helio, PBGC,
# PSGC, a plugin...) and merges before the dependency PR does leaves main
# pinned to a commit that the dependency's main does not contain; if that
# branch is later rebased or deleted, main stops building. The merge order is:
#
#   1. merge the dependency PR into the dependency's default branch;
#   2. re-pin the Pulsar-Native submodule to that default-branch commit
#      (`git -C <path> fetch && git -C <path> checkout origin/main`, commit);
#   3. merge the Pulsar-Native PR.
#
#   scripts/submodule-pins.sh check [--strict]
#                         fail unless each submodule's pinned commit (from the
#                         HEAD tree) is an ancestor of its origin's default
#                         branch, or the pin is listed as an exception;
#                         --strict also fails on stale exceptions
#   scripts/submodule-pins.sh list   print path, pin and default branch
#
# Exceptions live in .github/submodule-pin-exceptions.txt, one
# `<path> <sha> <reason / upstream PR>` per line; a pin that is off its default
# branch and not listed fails the check. An exception whose pin has since
# landed on the default branch (or that no longer matches the pin) is reported
# as stale: delete the line.
#
# Upstream commit graphs are fetched (commits only, no trees or blobs) into
# bare caches under the repository's git dir (override: SUBMODULE_PINS_CACHE),
# so the submodules themselves need not be checked out or deep.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
exceptions_file="$root/.github/submodule-pin-exceptions.txt"
cache="${SUBMODULE_PINS_CACHE:-$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/submodule-pins}"

# `<name> <path>` for each submodule in .gitmodules.
submodules() {
  git -C "$root" config -f .gitmodules --get-regexp '^submodule\..*\.path$' |
    sed -E 's/^submodule\.(.*)\.path (.*)$/\1 \2/'
}

pin_of() {
  git -C "$root" ls-tree HEAD -- "$1" | awk '$2 == "commit" { print $3 }'
}

# Prepare the bare cache for submodule $1 at URL $2 and fetch its branches;
# prints the cache path.
fetch_upstream() {
  local dir="$cache/$(printf '%s' "$1" | tr '/' '_').git"
  if [ ! -d "$dir" ]; then
    git init -q --bare "$dir"
    git -C "$dir" remote add origin "$2"
  else
    git -C "$dir" remote set-url origin "$2"
  fi
  # Commits only: ancestry needs no trees or blobs. Fall back to a full fetch
  # where the server or git lacks partial clone.
  git -C "$dir" fetch -q --prune --no-tags --filter=tree:0 origin '+refs/heads/*:refs/remotes/origin/*' 2>/dev/null ||
    git -C "$dir" fetch -q --prune --no-tags origin '+refs/heads/*:refs/remotes/origin/*'
  echo "$dir"
}

default_branch() {
  git -C "$1" ls-remote --symref origin HEAD | awk '$1 == "ref:" { sub("refs/heads/", "", $2); print $2; exit }'
}

# Prints `<line> <path> <sha> <reason...>` for each exception. A `#` starts a
# comment only at the start of a line or after a space, so `Repo#12` survives.
exceptions() {
  [ -f "$exceptions_file" ] || return 0
  awk '{ sub(/(^|[ \t])#.*/, "") } NF >= 2 { $0 = NR " " $0; print }' "$exceptions_file"
}

warn() {
  # $1 = exceptions-file line, rest = message.
  local line="$1"
  shift
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::warning file=.github/submodule-pin-exceptions.txt,line=$line::$*"
  fi
  echo "warning: $*" >&2
}

fail() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error::$*"
  fi
  echo "error: $*" >&2
}

cmd="${1:-}"
case "$cmd" in
  list)
    while read -r name path; do
      dir="$(fetch_upstream "$name" "$(git -C "$root" config -f .gitmodules "submodule.$name.url")")"
      printf '%-36s %s %s\n' "$path" "$(pin_of "$path")" "$(default_branch "$dir")"
    done < <(submodules)
    ;;
  check)
    strict=0
    [ "${2:-}" = "--strict" ] && strict=1
    mkdir -p "$cache"
    status=0
    stale=0
    checked=0
    excused=0
    declare -A exception_used=()

    while read -r name path; do
      url="$(git -C "$root" config -f .gitmodules "submodule.$name.url")"
      pin="$(pin_of "$path")"
      if [ -z "$pin" ]; then
        fail "$path is in .gitmodules but HEAD has no submodule commit there"
        status=1
        continue
      fi
      dir="$(fetch_upstream "$name" "$url")"
      branch="$(default_branch "$dir")"
      checked=$((checked + 1))

      # The exception for this path, if any: `<line> <sha> <reason...>`.
      exception="$(exceptions | awk -v p="$path" '$2 == p { $2 = ""; print; exit }')"
      ex_line="" ex_sha="" ex_reason=""
      if [ -n "$exception" ]; then
        read -r ex_line ex_sha ex_reason <<< "$exception"
        exception_used[$path]=1
      fi

      # A pin on no branch may still be fetchable by id (e.g. a deleted branch).
      if ! git -C "$dir" cat-file -e "${pin}^{commit}" 2>/dev/null; then
        git -C "$dir" fetch -q --no-tags --filter=tree:0 origin "$pin" 2>/dev/null ||
          git -C "$dir" fetch -q --no-tags origin "$pin" 2>/dev/null || true
      fi
      if ! git -C "$dir" cat-file -e "${pin}^{commit}" 2>/dev/null; then
        fail "$path is pinned to $pin, which $url does not have (unpushed?)"
        status=1
        continue
      fi

      if git -C "$dir" merge-base --is-ancestor "$pin" "refs/remotes/origin/$branch"; then
        if [ -n "$exception" ]; then
          warn "$ex_line" "stale exception: $path $ex_sha is listed, but the pin ($pin) is on $branch now; delete the line"
          stale=1
        fi
        continue
      fi

      contains="$(git -C "$dir" branch -r --contains "$pin" | sed -E 's#^ *origin/##' | head -n 10 | paste -sd ' ' -)"
      if [ -n "$exception" ] && [ "${pin#"$ex_sha"}" != "$pin" ]; then
        echo "allowed: $path $pin is not on $branch (on: ${contains:-no branch}) - $ex_reason"
        excused=$((excused + 1))
        continue
      fi

      fail "$path is pinned to $pin, which is not on $url's $branch"
      echo "  branches containing it: ${contains:-none}" >&2
      if [ -n "$exception" ]; then
        echo "  the exception on line $ex_line names $ex_sha, not this pin" >&2
      fi
      echo "  merge the dependency PR first, re-pin $path to $branch, then merge;" >&2
      echo "  or list '$path $pin <upstream PR>' in .github/submodule-pin-exceptions.txt" >&2
      status=1
    done < <(submodules)

    while read -r line path _; do
      if [ -z "${exception_used[$path]:-}" ]; then
        warn "$line" "stale exception: $path is not a submodule; delete the line"
        stale=1
      fi
    done < <(exceptions)

    if [ $stale -eq 1 ] && [ $strict -eq 1 ]; then
      status=1
    fi
    if [ $status -eq 0 ]; then
      echo "submodule pins: $checked checked, $((checked - excused)) on their default branch, $excused excepted"
    fi
    exit $status
    ;;
  *)
    sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
