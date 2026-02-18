# DebKit

DebKit is a Rust-based CLI tool for bringing a Debian system into a known, reproducible development-ready state.

It is workstation-first and Debian-specific. DebKit installs and configures toolchains and common developer software using deterministic, idempotent steps.

Run it once on a fresh machine.  
Run it again safely.  
Get the same result.

---

## Philosophy

DebKit is:

- Deterministic
- Idempotent (safe to re-run)
- Feature-driven
- Profile-based
- Debian-specific
- Explicit and typed (no runtime scripting)

DebKit is **not**:

- A YAML DSL engine
- A dynamic configuration interpreter
- A general infrastructure orchestrator

It is a compiled, structured tool for building and maintaining Debian developer machines.

---

## Core Concepts

### Step

The smallest idempotent action.

Examples:

- Install apt packages
- Install rustup
- Ensure a line exists in `~/.bashrc`
- Add a Cargo config entry

Every step implements:

- `check()` — determine if already satisfied
- `apply()` — perform change safely

Steps are internal and dependency-aware.

---

### Feature

A user-facing installable element.

Examples:

- `rust`
- `npm/nodejs`
- `chrome`
- `docker`
- `git`
- `dev-base`

Features expand into one or more steps.

Example:

```bash
debkit install rust
