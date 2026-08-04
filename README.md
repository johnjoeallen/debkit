# DebKit (WIP)

DebKit is a Rust-based CLI tool for bringing a Debian system into a known, reproducible development-ready state. It's workstation-first and Debian-specific: idempotent toolchain/package installers, plus a declarative, state-aware engine for diagnosing and repairing system configuration (DNS, firewalls, network managers, identity services, hardware quirks).

Run it once on a fresh machine.
Run it again safely.
Get the same result.

**Full documentation: <https://johnjoeallen.github.io/debkit/>**

## Quick start

```bash
debkit host-config          # create ~/.config/debkit/hosts/<hostname>.yaml
debkit list                 # installable targets + registered lifecycle modules
debkit inspect               # discover observed state for every module
debkit diagnose core.inspect  # discover + compare a single module against config
```

See [`config.example.yaml`](./config.example.yaml) for a fully commented reference
config, or the [docs site](https://johnjoeallen.github.io/debkit/) for the complete
CLI reference, module catalogue, and per-feature guides (NIS, Wake-on-LAN, passwordless
sudo).

## Building the docs locally

```bash
mdbook serve docs --open
```

Docs source lives under `docs/src/`; the built site (`docs/book/`) is gitignored and
published automatically to GitHub Pages on every push to `main` that touches `docs/**`.
