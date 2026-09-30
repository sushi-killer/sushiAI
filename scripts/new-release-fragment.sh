#!/bin/sh
set -eu

usage() {
  printf '%s\n' 'Usage: scripts/new-release-fragment.sh <slug> "<Title>"' >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
slug=$1
title=$2
[ -n "$title" ] || usage
case "$slug" in
  ''|[!a-z0-9]*|*[!a-z0-9-]*) usage ;;
esac

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd)
relative_path="docs/releases/unreleased/$slug.md"
target="$repo_root/$relative_path"

if [ -e "$target" ]; then
  printf 'File already exists: %s\n' "$relative_path" >&2
  exit 1
fi

set -C
if ! printf '## %s\n\n- TODO: describe the change for users.\n' "$title" > "$target"; then
  printf 'File already exists: %s\n' "$relative_path" >&2
  exit 1
fi

printf '%s\n' "$relative_path"
printf '%s\n' 'Fragments are user-facing prose, in English, with no attribution trailers.'
