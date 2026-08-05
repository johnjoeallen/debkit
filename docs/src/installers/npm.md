# Node.js / npm

```bash
debkit install npm [--version <version>]
debkit uninstall npm
```

Installs Node.js (and the npm it bundles) from the official upstream Node.js binary
tarballs — not from apt or nvm — into a DebKit-managed per-user location owned by the
invoking user, and ensures the shell init sources `$HOME/.local/bin` onto `PATH`.
Nothing here ever runs as root, so nothing it installs ends up root-owned either —
unlike a system apt/`sudo npm install -g` Node, every later `npm install -g <pkg>`
(including [Claude Code](./claude.md) and [Codex](./codex.md), below) stays
unprivileged too. Re-running with the
same version is a no-op. Each version gets its own directory (nothing is deleted when
you install a different one), but a `current` symlink always points at whichever
version was installed most recently — that symlink is what ends up on `PATH`, so
installing a second version does switch the active one, even though the first is
still on disk.

```yaml
debkit:
  npm:
    version: latest
```

`foundation.install: [npm, ...]` uses `npm.version` from config. `claude`/`codex`
(below) and `foundation`'s `claude`/`codex` targets all install npm as a prerequisite
using `config.npm.version`, via the same managed install path.
