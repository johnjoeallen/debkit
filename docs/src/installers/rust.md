# Rust

```bash
debkit install rust [--reinstall]
```

Installs the Rust toolchain. If `rustup` is already present, it installs/updates the
`stable` toolchain and sets it as default; otherwise it bootstraps via the official
`rustup.rs` shell installer (`--profile default --default-toolchain stable`). Also
ensures shell init sources `$HOME/.cargo/env`. Without `--reinstall`, a machine that
already has both `cargo` and `rustc` on `PATH` is a no-op; `--reinstall` forces
`rustup self update` plus a toolchain reinstall even when both are already present.

No `debkit configure rust` subcommand exists today, despite `debkit list` marking this
target `[install, configure]` — that capability flag is currently aspirational/stale
metadata, not a real CLI surface. No config section either; `--reinstall` is the only
knob.
