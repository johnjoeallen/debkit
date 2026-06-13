#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

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

if [[ ! "$current_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "Cannot bump non-standard package version: $current_version" >&2
  exit 1
fi

new_version="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.$((BASH_REMATCH[3] + 1))"

awk -v old="$current_version" -v new="$new_version" '
  /^\[package\]$/ { in_package = 1 }
  /^\[/ && $0 != "[package]" && in_package { in_package = 0 }
  in_package && $0 == "version = \"" old "\"" && ! bumped {
    print "version = \"" new "\""
    bumped = 1
    next
  }
  { print }
  END {
    if (!bumped) {
      exit 1
    }
  }
' Cargo.toml > Cargo.toml.tmp
mv Cargo.toml.tmp Cargo.toml

awk -v old="$current_version" -v new="$new_version" '
  /^\[\[package\]\]$/ { in_package = 1; package_name = ""; print; next }
  in_package && $1 == "name" {
    package_name = $3
  }
  in_package && package_name == "\"debkit\"" && $0 == "version = \"" old "\"" {
    print "version = \"" new "\""
    updated = 1
    next
  }
  { print }
  END {
    if (!updated) {
      exit 1
    }
  }
' Cargo.lock > Cargo.lock.tmp
mv Cargo.lock.tmp Cargo.lock

echo "Bumped debkit version: $current_version -> $new_version"
