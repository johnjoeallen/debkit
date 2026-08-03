# DebKit (WIP)

DebKit is a Rust-based CLI tool for bringing a Debian system into a known, reproducible development-ready state.

It is workstation-first and Debian-specific. DebKit installs and configures toolchains and common developer software using deterministic, idempotent steps, and diagnoses/repairs system configuration through a declarative, state-aware engine.

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
- State-aware: it discovers which subsystem currently owns a setting before proposing a change

DebKit is **not**:

- A dynamic configuration interpreter or scripting language — YAML config is data, not logic
- A general infrastructure orchestrator
- A tool that guesses; it refuses to act when it can't confidently determine what owns a resource

It is a compiled, structured tool for building, maintaining, and troubleshooting Debian developer machines.

---

## Two halves of the CLI

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
module with a one-line description.

---

## Lifecycle modules

```bash
debkit inspect   [module]           # discover() only — what's actually there
debkit diagnose  [module]           # discover() + compare against declared config
debkit plan      [module]           # dry-run of what apply() would do
debkit apply     [module]           # apply the plan, then verify; auto-rollback on verify failure
debkit verify    [module]           # functional checks only, e.g. after a reboot
debkit rollback  <module> [--journal <path>]   # reverse the most recent applied plan
debkit status    <module>           # discover()+diagnose(), human-readable (no apply)
debkit history   [module]           # read back the evidence apply/verify already recorded
```

Omit `[module]` on `inspect`/`diagnose`/`plan`/`apply`/`verify` to run every registered
module. Every apply/verify run writes structured evidence to
`/var/lib/debkit/<module>/<hostname>.json`.

A module's `plan()` returns an empty plan when it's already compliant — re-running
`apply` on a healthy system is a safe no-op. If more than one subsystem is actively
competing to own the same resource (e.g. two DNS resolvers both listening on :53),
`diagnose()` reports a conflict and `plan`/`apply` refuse to act until it's resolved.

Currently registered modules (see `debkit list` for the live, authoritative list):

| Module | What it manages |
| --- | --- |
| `core.inspect` | Read-only baseline evidence: OS, kernel, failed units, watched packages, NICs |
| `network.interfaces` | Interface inventory, manager-ownership conflicts, stable MAC-based naming |
| `network.dhcp` | Read-only DHCP server ownership conflict + client-backend detection |
| `network.dns` | Declarative dnsmasq local zones/upstream, resolver-conflict detection, `dig` verify |
| `network.firewall` | Read-only backend/ruleset diagnostics, real TCP-reachability verification |
| `network.tailscale` | Read-only Tailscale backend/DNS status |
| `network.wake_on_lan` | Wake-on-LAN via NetworkManager or ethtool, with ownership conflict detection |
| `identity.nis` | NIS domain, `yp.conf`, `nsswitch.conf`, master-side map lifecycle |
| `identity.nss` | Local vs. NIS UID/GID collision detection, local-recovery-access check |
| `identity.pam` | `pam_mkhomedir.so` for create-home-on-first-login |
| `identity.sudo` | Passwordless-sudo group, NOPASSWD drop-in, membership |
| `systemd.units` | Read-only report of currently failed systemd units |
| `developer.git` | Global `git` credential helper and credential-store file permissions |
| `apt.repositories` | apt-cacher-ng proxy config and DIRECT-bypass exceptions |
| `hardware.reboot` | AM5 board/BIOS identification, known-affected-BIOS registry, memory-capacity check |
| `hardware.sleep` | Suspend/resume diagnostics, the active `/sys/power/mem_sleep` mode |
| `hardware.rgb` | `i2c-dev` kernel module prerequisite for motherboard/SMBus RGB control |

Some deliberately stop at diagnostics: `network.dhcp`/`network.firewall`/`systemd.units`
never write config (DebKit isn't positioned to choose a DHCP server or safely rewrite
firewall rules over the same connection you'd use to fix a mistake). `network.tailscale`
diagnoses but doesn't enforce DNS-acceptance behavior. `identity.nis` manages the
file/service layer and master-side map lifecycle, but NIS slave-side map bootstrapping
still goes through the legacy `debkit install nis`/`debkit configure nis` commands
described below — that's genuinely stateful, multi-host orchestration that doesn't fit
the current `Change` primitives yet.

---

## Configuration

```bash
debkit host-config   # create/update ~/.config/debkit/hosts/<hostname>.yaml
```

Config is YAML, split into a shared base and a per-host overlay that's deep-merged on
top — the host file only needs to declare what differs for that host:

```text
~/.config/debkit/config.yaml
~/.config/debkit/hosts/<hostname>.yaml
```

Every field has an in-code default, so a sparse file is fine. See
[`config.example.yaml`](./config.example.yaml) at the repo root for every section with
inline comments — the fastest way to see the full schema in one place.

```yaml
debkit:
  foundation:
    install: [essentials, git, ripgrep, rust, npm, codex, variety, nis, wake-on-lan]
```

### Essentials

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

### NIS

DebKit models the home/lab NIS topology as server plus client on every NIS-enabled
machine:

- `iris.dublinux.lan`: `master`
- all other machines: `slave`

This keeps shared NIS users and groups available on normal clients when the master is
unavailable, assuming their slave maps have already been initialized and synchronized.
NIS is for a trusted LAN; do not treat it as a secure authentication system.

The base config carries the lab defaults but leaves NIS disabled until a host opts in:

```yaml
debkit:
  nis:
    enabled: false
    role: slave
    domain: dublinux.lan
    admin_user: jallen
    local_admin_groups: [sudo]
    master: iris.dublinux.lan
    server: iris.dublinux.lan
    prefer_local: true
    push_to_slaves: false
    force_refresh_maps: false
    slaves: []
    servers: []
```

On `iris`, put this in `~/.config/debkit/hosts/iris.yaml`:

```yaml
debkit:
  nis:
    enabled: true
    role: master
    domain: dublinux.lan
    admin_user: jallen
    local_admin_groups: [sudo]
    push_to_slaves: true
    slaves: [spitfire.dublinux.lan, laptop.dublinux.lan]
```

On every normal client, put this in that host's override file:

```yaml
debkit:
  nis:
    enabled: true
    role: slave
    domain: dublinux.lan
    master: iris.dublinux.lan
    prefer_local: true
    force_refresh_maps: false
```

Plain NIS clients are supported only for machines that do not need offline NIS account
availability:

```yaml
debkit:
  nis:
    enabled: true
    role: client
    domain: dublinux.lan
    server: iris.dublinux.lan
```

`install nis` applies the configured role. `nis-client` and `nis-server` still exist as
compatibility targets, but the recommended role for normal machines is `slave`, not
plain `client`. `debkit diagnose identity.nis` / `debkit plan identity.nis` cover the
domain/`yp.conf`/`nsswitch.conf`/package/service layer read-only or dry-run without
touching maps.

DebKit writes `/etc/defaultdomain`, `/etc/yp.conf`, and keeps `/etc/nsswitch.conf`
local-first with `files nis` for `passwd`, `group`, and `shadow`. It does not add or
rewrite an explicit `initgroups:` line; if one already exists, DebKit leaves it alone
and warns when NIS supplementary group lookup appears incomplete. Client-capable NIS
installs validate lookup with `getent group`, `getent initgroups`, and `id` for the
configured NIS admin user and local admin groups. It never removes or rewrites
`/etc/passwd`, `/etc/shadow`, or `/etc/group`. Keep a local sudo-capable account on
every machine for recovery.

### Passwordless sudo

DebKit can grant passwordless sudo to a specific group and add configured users to that
group. The default group name is `superuser`:

```yaml
debkit:
  sudo_nopass:
    enabled: true
    group: superuser
    add_current_user: true
    users: [jallen, alice]
    nis_managed: false
```

Run `debkit install sudo-nopass` or add `sudo-nopass` to `foundation.install`. DebKit
writes a `/etc/sudoers.d/99-<group>-nopass` drop-in, preserves the regular `%sudo` rule
when it is missing, adds the configured users to the group, and validates the result
with `visudo -c`. If the group is managed by NIS instead of `/etc/group`, set
`nis_managed: true`; DebKit will leave membership to NIS and validate that NSS and sudo
policy report the expected no-password access. `debkit diagnose identity.sudo` covers
the same checks read-only.

For `master`, DebKit installs the server and client pieces, enables `rpcbind`,
`ypserv`, and `ypbind`, initializes missing maps with `/usr/lib/yp/ypinit -m`, and
rebuilds existing maps with `make -C /var/yp`. Re-running the command is safe:
initialization only happens when `/var/yp/<domain>` does not exist. It also manages
`/var/yp/ypservers` from the master hostname and the configured `slaves` list, then
explicitly rebuilds `/var/yp/<domain>/ypservers` with key/value pairs where both key
and value are the server hostname. When `push_to_slaves: true`, DebKit uses Debian's
`/usr/sbin/yppush` to push the known map set to each configured slave. Push failures
fail the run with the map and slave context; the reliable fallback is still to run
`debkit configure nis` on the slave.

For `slave`, DebKit installs the server and client pieces, first configures
`/etc/yp.conf` to bind to `iris.dublinux.lan` for bootstrap, enables `rpcbind`,
`ypserv`, and `ypbind`, and runs `sudo /usr/lib/yp/ypinit -s iris.dublinux.lan` when
local maps do not exist yet. If `ypinit -s` fails, DebKit falls back to direct map
transfer with `/usr/lib/yp/ypxfr -h iris.dublinux.lan -d dublinux.lan <map>`. Once local
maps exist, it switches `/etc/yp.conf` to prefer `127.0.0.1` before the master when
`prefer_local: true`, then restarts `ypbind`.

To force-refresh replicated maps on a configured slave, run:

```bash
debkit configure nis
```

This temporarily points the slave at the master, runs direct forced `ypxfr` transfers
for the known Debian NIS map set, then restores the configured local-preferred binding
and restarts `ypbind`. Setting `force_refresh_maps: true` on a slave makes `debkit
install nis` use the same forced pull path during the normal install/configure run.

To add a slave to a master host config from another machine:

```bash
debkit configure nis add-slave --host iris spitfire.dublinux.lan
```

The command edits `~/.config/debkit/hosts/iris.yaml`, validates that the selected host
is a NIS master, avoids duplicate entries, and prints the next master/slave commands to
run.

Troubleshooting notes from the live Iris/Spitfire setup:

- `ypcat passwd.byname` working does not prove slave initialization will work.
- `ypcat ypservers` on the master must include each slave hostname.
- The generated `ypservers` map must have non-empty hostname values; blank `ypcat
  ypservers` output means it was built incorrectly.
- Debian `make -B` under `/var/yp` may not rebuild the generated `ypservers` map.
- `ypinit -s` can fail even when direct `ypxfr` works; first transfer may print `Cannot
  open old ... ignored`, which is normal for a new slave map.
- Do not prefer localhost on a slave before local maps exist and local `ypserv` is
  running.

### Wake-on-LAN

DebKit can inspect and enable standard wired Ethernet Wake-on-LAN:

```bash
debkit status wake-on-lan
debkit install wake-on-lan
debkit install wake-on-lan --dry-run
```

The same functionality is also available as the `network.wake_on_lan` lifecycle module
(`debkit diagnose|plan|apply network.wake_on_lan`), which adds ownership-conflict
detection between NetworkManager and a `debkit-wol@` systemd unit both claiming the
same interface.

Run `debkit status wake-on-lan` on `spitfire` to capture the current NetworkManager
state, wired and wireless interfaces, optional `ethtool` Wake-on-LAN verification,
active NetworkManager profile, and the wake details needed by the TimeVault server.

DebKit defaults to NetworkManager-native Wake-on-LAN because `spitfire` appears to have
used NetworkManager without installing `ethtool`. NetworkManager mode does not install
`ethtool`; if `ethtool` is absent, DebKit reports that low-level NIC verification was
skipped.

Default/NetworkManager config in `~/.config/debkit/config.yaml`:

```yaml
debkit:
  wake_on_lan:
    enabled: true
    interfaces: auto
    mode: magic
    backend: network_manager
    reference_host: <current-hostname>
```

Explicit `ethtool` config:

```yaml
debkit:
  wake_on_lan:
    enabled: true
    interfaces: [enp9s0]
    mode: magic
    backend: ethtool
```

`backend: auto` tries NetworkManager first when `nmcli` is available, NetworkManager is
running, the target interface is wired and managed, and an active connection profile
exists. Otherwise it falls back to `ethtool`, installs it using DebKit's apt convention
if missing, writes `/etc/systemd/system/debkit-wol@.service`, and enables
`debkit-wol@<interface>.service`.

Wake info is written after configuration:

```text
/var/lib/debkit/wake-on-lan/<hostname>.txt
/var/lib/debkit/wake-on-lan/<hostname>.json
```

From TimeVault:

```bash
wakeonlan <mac>
sudo etherwake -i <timevault-interface> <mac>
```

Troubleshooting checks:

- BIOS/UEFI Wake-on-LAN or PCIe wake is disabled.
- The selected interface is Wi-Fi or the wrong wired NIC.
- NetworkManager is not managing the interface.
- `ethtool` is missing when `backend = "ethtool"` is requested and apt cannot install it.
- Wake-on-LAN is not persistent after reboot.
- The machine loses standby power when shut down.
- VLAN, subnet, or broadcast routing prevents the magic packet from reaching the target.
