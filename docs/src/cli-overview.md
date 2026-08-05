# CLI Overview

DebKit has two command families that solve different problems:

1. **Installers** (`install`/`configure`/`uninstall`/`status`) — plain, idempotent
   toolchain and package installers (Rust, npm, essentials, Wake-on-LAN, NIS, ...).
   Each one just does its job; there's no ongoing "compliance" concept.
2. **Lifecycle modules** (`inspect`/`diagnose`/`plan`/`apply`/`verify`/`rollback`) — a
   declarative, state-aware engine for troubleshooting and configuration that changes
   underneath you: DNS resolvers, firewalls, network managers, identity services,
   hardware quirks. Each module *discovers* what's actually running, *diagnoses*
   whether it matches what you declared, *plans* a minimal change, *applies* it
   atomically with a rollback journal, and *verifies* the result functionally (not just
   "the command exited 0").

Run `debkit list` to see both: installable targets, and every registered lifecycle
module with a one-line description. Run `debkit desc <name>` to look up that
description for a single target or module by name (e.g. `debkit desc claude`,
`debkit desc boot.grub`). Run `debkit info <name>` for the full documentation page for
that target or module — the same page content published to GitHub Pages, embedded in
the binary at compile time so it's available offline and regardless of how `debkit` was
installed.
