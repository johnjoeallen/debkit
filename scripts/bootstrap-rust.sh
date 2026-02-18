#!/usr/bin/env bash
set -euo pipefail

reinstall=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --reinstall)
      reinstall=1
      shift
      ;;
    -h|--help)
      cat <<'EOF'
Usage: ./scripts/bootstrap-rust.sh [--reinstall]

Options:
  --reinstall  Force reinstall/refresh of Rust toolchain and cargo-deb
EOF
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

ensure_shell_init_sources_cargo_env() {
  local line='source "$HOME/.cargo/env"'
  local files=("$HOME/.bashrc" "$HOME/.profile")
  local file

  for file in "${files[@]}"; do
    if [[ ! -f "$file" ]]; then
      touch "$file"
    fi

    if ! grep -Fqx "$line" "$file"; then
      printf '\n%s\n' "$line" >>"$file"
    fi
  done
}

if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
  echo "Do not run as root. Run as a normal user with sudo access." >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd "$script_dir/.." && pwd)"

if [[ "$reinstall" -eq 0 ]] && command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
  echo "Rust already installed:"
  cargo --version
  rustc --version
  ensure_shell_init_sources_cargo_env
else
  if ! command -v sudo >/dev/null 2>&1; then
    echo "sudo is required to install system packages." >&2
    exit 1
  fi

  echo "Installing system prerequisites..."
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    pkg-config \
    libssl-dev

  if [[ "$reinstall" -eq 1 ]]; then
    echo "Reinstalling rustup + stable toolchain..."
  else
    echo "Installing rustup + stable toolchain..."
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default --default-toolchain stable

  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
  ensure_shell_init_sources_cargo_env

  echo "Rust installation complete:"
  cargo --version
  rustc --version
fi

# shellcheck source=/dev/null
source "$HOME/.cargo/env"
cd "$project_dir"

echo "Running: cargo install --locked cargo-deb"
if [[ "$reinstall" -eq 1 ]]; then
  cargo install --locked --force cargo-deb
else
  cargo install --locked cargo-deb
fi

echo "Running: cargo run -- package deb --verbose"
if [[ "$reinstall" -eq 1 ]]; then
  cargo run -- package deb --verbose --reinstall
else
  cargo run -- package deb --verbose
fi

echo
echo "Bootstrap commands complete."
