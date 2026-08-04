# Essentials

```bash
debkit install essentials
```

Installs the baseline Debian packages DebKit expects on a fresh workstation via apt.
The target only runs `apt-get update` when one or more configured packages are
actually missing — re-running it on a machine that already has everything is a no-op
apt-wise.

```yaml
debkit:
  essentials:
    packages:
      [curl, wget, zip, unzip, rsync, ca-certificates, gnupg, apt-transport-https, neovim, ripgrep]
```

`ripgrep` is a default essentials package rather than its own `foundation.install`
entry — see [ripgrep](./ripgrep.md) for the standalone `debkit install/uninstall
ripgrep` target, which still exists for surgical install/removal independent of the
rest of essentials.

An empty or unset `packages` list falls back to DebKit's built-in defaults (shown
above) rather than installing nothing. To skip essentials entirely, leave it out of
`foundation.install` rather than trying to configure an empty package set. See
[Configuration § Essentials](../configuration.md#essentials) for the config walkthrough.
