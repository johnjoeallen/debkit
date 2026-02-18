#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd "$script_dir/.." && pwd)"

cd "$project_dir"

# Pass through any extra arguments, for example:
#   ./scripts/build-package.sh --verbose --reinstall
cargo run -- package deb "$@"
