# DebKit

DebKit is a Rust-based CLI tool for bringing a Debian system into a known, reproducible development-ready state.

It is workstation-first and Debian-specific. DebKit installs and configures toolchains and common developer software using deterministic, idempotent steps, and diagnoses/repairs system configuration through a declarative, state-aware engine.

Run it once on a fresh machine.
Run it again safely.
Get the same result.

## Philosophy

DebKit is:

- Deterministic
- Idempotent (safe to re-run)
- Feature-driven
- Profile-based
- Debian-specific
- Explicit and typed (no runtime scripting)
- State-aware: it discovers which subsystem currently owns a setting before proposing a change

DebKit is **not**:

- A dynamic configuration interpreter or scripting language — YAML config is data, not logic
- A general infrastructure orchestrator
- A tool that guesses; it refuses to act when it can't confidently determine what owns a resource

It is a compiled, structured tool for building, maintaining, and troubleshooting Debian developer machines.
