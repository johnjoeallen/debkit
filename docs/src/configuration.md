# Configuration

```bash
debkit host-config   # create/update ~/.config/debkit/hosts/<hostname>.yaml
```

Config is YAML, split into a shared base and a per-host overlay that's deep-merged on
top — the host file only needs to declare what differs for that host:

```text
~/.config/debkit/config.yaml
~/.config/debkit/hosts/<hostname>.yaml
```

Every field has an in-code default, so a sparse file is fine. See the
[Full Example](./config-reference.md) chapter for every section with inline comments —
the fastest way to see the full schema in one place — or [Example Configs](./examples/README.md)
for smaller, realistic configs for specific situations (a minimal workstation, a NIS
master/slave pair, a roaming laptop).

```yaml
debkit:
  foundation:
    install: [essentials, git, ripgrep, rust, npm, codex, variety, nis, wake-on-lan]
```

## Essentials

`install essentials` installs the baseline Debian packages DebKit expects on a fresh
workstation:

```yaml
debkit:
  essentials:
    packages:
      [curl, wget, zip, unzip, rsync, ca-certificates, gnupg, apt-transport-https, neovim]
```

The target only runs `apt-get update` when one or more configured packages are missing.

The host file supplements that base and only needs host-specific differences. For
example, to disable Wake-on-LAN only on one host:

```yaml
debkit:
  wake_on_lan:
    enabled: false
```

Generated base config uses the current hostname for `wake_on_lan.reference_host`.
