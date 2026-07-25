#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Working tree has uncommitted changes; commit or stash before tagging." >&2
  exit 1
fi

current_version="$(
  awk '
    /^\[package\]$/ { in_package = 1; next }
    /^\[/ && in_package { exit }
    in_package && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
)"

if [[ -z "$current_version" ]]; then
  echo "Could not read package version from Cargo.toml" >&2
  exit 1
fi

tag="v$current_version"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "Tag $tag already exists." >&2
  exit 1
fi

git tag "$tag"
echo "Created tag $tag"
