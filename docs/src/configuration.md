# Configuration

```bash
debkit host-config          # create/update ~/.config/debkit/hosts/<hostname>.yaml
debkit enable hardware.grub # set hardware_grub.enabled: true in ~/.config/debkit/config.yaml
```

`debkit enable <module>` is a shortcut for the common case of turning a module's gate
on without hand-editing YAML: it adds or updates the module's section in the per-user
base config (`~/.config/debkit/config.yaml`), setting `enabled: true`, creating the
file and the section if either is missing. It only works for modules with a plain
`enabled: bool` gate — most of them (`hardware.grub`, `network.dns`,
`network.wake_on_lan`, `identity.sudo`, `identity.nis`, `developer.git`,
`apt.repositories`, `network.tailscale`, `hardware.sleep`, `hardware.rgb`). A few
modules are gated differently (`identity.pam` gates on
`create_home_on_first_login`) or have no config section at all (purely-diagnostic
modules like `core.inspect`, `network.dhcp`) — `debkit enable` refuses with an error
for those, since there'd be nothing sensible for it to set. It only ever touches the
per-user base config, never the global or host-overlay tiers — run `debkit
host-config` first if you want the same flag scoped to one host instead.

Config is YAML, deep-merged across up to three optional tiers, lowest to highest
priority:

```text
/etc/debkit/config.yaml              # global -- packaged by the .deb, machine-wide
~/.config/debkit/config.yaml         # per-user base
~/.config/debkit/hosts/<hostname>.yaml   # per-user host overlay
```

Any key present in a higher tier wins; anything it doesn't mention falls through to
the tier below. All three are optional — a missing file just means "nothing declared
at this tier," never an error, and every field has an in-code default regardless, so
even a fully empty config tree is a normal, supported state.

`/etc/debkit/config.yaml` is provisioned by the `.deb` package itself (from
`config.example.yaml`, as a dpkg conffile — `apt install`/upgrade creates it and
preserves any local edits across upgrades). DebKit is mostly not a per-user tool: most
lifecycle modules (`hardware_grub`, `network_*`, `identity_*`, `apt`, `pam`,
`systemd_units`) describe the *machine's* state, not an individual operator's
preferences, so that's the natural home for them — the board registry is the
sharpest example (see [hardware.grub](./modules/hardware-grub.md)): which BIOS
versions are known-bad on a board has nothing to do with which Unix user runs
`debkit`. The per-user files remain the right place for genuinely personal
preferences instead — toolchain/session settings like `npm.version`, `wallpapers`,
`variety`, and `git.credential_helper`.

See the [Full Example](./config-reference.md) chapter for every section with inline
comments — the fastest way to see the full schema in one place, and literally the
content packaged to `/etc/debkit/config.yaml` — or [Example Configs](./examples/README.md)
for smaller, realistic configs for specific situations (a minimal workstation, a NIS
master/slave pair, a roaming laptop).

```yaml
debkit:
  foundation:
    install: [essentials, git, rust, npm, codex, variety, nis, wake-on-lan]
```

## Essentials

`install essentials` installs the baseline Debian packages DebKit expects on a fresh
workstation:

```yaml
debkit:
  essentials:
    packages:
      [curl, wget, zip, unzip, rsync, ca-certificates, gnupg, apt-transport-https, neovim, ripgrep]
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
