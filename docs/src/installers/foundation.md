# Foundation

```bash
debkit install foundation
```

Runs every target listed in `foundation.install`, in order, using the same config
each target would read on its own (`essentials.packages`, `npm.version`, `nis.*`,
`sudo_nopass.*`, ...). An empty or unset `foundation.install` prints a note and does
nothing — there's no implicit default set run in its place.

```yaml
debkit:
  foundation:
    install: [essentials, git, ripgrep, rust, npm, codex, variety, nis, wake-on-lan]
```

Recognized target names (unknown names are skipped with a warning, not a hard error —
a typo in this list won't abort the rest of the run):

| Name | Aliases | Runs |
| --- | --- | --- |
| `essentials` | `base`, `dev-base`, `dev_base` | [Essentials](./essentials.md) |
| `git` | | [Git](./git.md) |
| `npm` | | [Node.js / npm](./npm.md) |
| `codex` | | [Codex CLI](./codex.md) |
| `ripgrep` | | [ripgrep](./ripgrep.md) |
| `rust` | | [Rust](./rust.md) (never with `--reinstall`) |
| `variety` | | [Variety](./variety.md) |
| `sudo-nopass` | `sudo_nopass`, `admin-group-nopass`, `admin_group_nopass` | [Passwordless Sudo](../sudo.md) |
| `nis` | | [NIS](../nis.md), configured role |
| `nis-client` | `nis_client` | NIS, forced client role |
| `nis-server` | `nis_server` | NIS, forced server role |
| `wake-on-lan` | `wake_on_lan`, `wol` | [Wake-on-LAN](../wake-on-lan.md) |

`git-prompt` isn't in this table — it's `configure`-only (`debkit configure
git-prompt`), not something `foundation.install` can drive.
