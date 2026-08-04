# Installers

Every target `debkit install`/`debkit configure`/`debkit uninstall`/`debkit status`
understands — the plain, idempotent half of the CLI described in
[CLI Overview](../cli-overview.md). `debkit list` prints this same set live, alongside
the registered [Lifecycle Modules](../lifecycle-modules.md).

| Target | Commands | What it does |
| --- | --- | --- |
| [essentials](./essentials.md) | `install` | Baseline CLI packages required for provisioning |
| [git](./git.md) | `install` | Git version control via apt |
| [git-prompt](./git-prompt.md) | `configure` | Git-aware Bash prompt for the current user |
| [npm](./npm.md) | `install`, `uninstall` | Node.js and npm from official Node.js binaries |
| [nis](../nis.md) | `install`, `configure` | NIS client and server packages |
| [nis-client](../nis.md) | `install`, `configure` | NIS client packages |
| [nis-server](../nis.md) | `install`, `configure` | NIS server packages |
| [codex](./codex.md) | `install`, `uninstall` | OpenAI Codex CLI via npm |
| [ripgrep](./ripgrep.md) | `install`, `uninstall` | ripgrep recursive search tool |
| [rust](./rust.md) | `install` | Rust toolchain via rustup |
| [sudo-nopass](../sudo.md) | `install` | Passwordless sudo for configured users |
| [variety](./variety.md) | `install` | Variety wallpaper rotator for GNOME |
| [foundation](./foundation.md) | `install` | Installs configured base targets from debkit config |
| [wake-on-lan](../wake-on-lan.md) | `install` | Inspect and enable wired Ethernet Wake-on-LAN |

`nis`/`nis-client`/`nis-server`, `sudo-nopass`, and `wake-on-lan` have their own
full chapters elsewhere in this book (NIS and Wake-on-LAN both have config sections
substantial enough to outgrow a single target page); everything else is documented
here.
