# DebKit (WIP)

DebKit is a Rust-based CLI tool for bringing a Debian system into a known, reproducible development-ready state.

It is workstation-first and Debian-specific. DebKit installs and configures toolchains and common developer software using deterministic, idempotent steps.

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

DebKit is **not**:

- A YAML DSL engine
- A dynamic configuration interpreter
- A general infrastructure orchestrator

It is a compiled, structured tool for building and maintaining Debian developer machines.

---

## Core Concepts

### Step

The smallest idempotent action.

Examples:

- Install apt packages
- Install rustup
- Ensure a line exists in `~/.bashrc`
- Add a Cargo config entry

Every step implements:

- `check()` — determine if already satisfied
- `apply()` — perform change safely

Steps are internal and dependency-aware.

---

### Feature

A user-facing installable element.

Examples:

- `rust`
- `npm/nodejs`
- `chrome`
- `docker`
- `git`
- `dev-base`

Features expand into one or more steps.

Example:

```bash
debkit install rust
```

### Host Config

Create the base config and a host-specific override file for the current host:

```bash
debkit configure host-config
```

The host file is written using the current hostname:

```text
~/.config/debkit/hosts/<hostname>.toml
```

The base `~/.config/debkit/config.toml` contains the full default config with DebKit's standard
targets enabled or represented with safe defaults:

```toml
[foundation]
install = ["git", "ripgrep", "rust", "npm", "codex", "variety", "nis", "wake-on-lan"]
```

The host file supplements that base and only needs host-specific differences. For example, to
disable Wake-on-LAN only on one host:

```toml
[wake_on_lan]
enabled = false
```

Generated base config uses the current hostname for `wake_on_lan.reference_host`.

### NIS

DebKit models the home/lab NIS topology as server plus client on every NIS-enabled machine:

- `iris.dublinux.lan`: `master`
- all other machines: `slave`

This keeps shared NIS users and groups available on normal clients when the master is unavailable,
assuming their slave maps have already been initialized and synchronized. NIS is for a trusted LAN;
do not treat it as a secure authentication system.

The base config carries the lab defaults but leaves NIS disabled until a host opts in:

```toml
[nis]
enabled = false
role = "slave"
domain = "dublinux.lan"
admin_user = "jallen"
local_admin_groups = ["sudo"]
master = "iris.dublinux.lan"
server = "iris.dublinux.lan"
prefer_local = true
push_to_slaves = false
force_refresh_maps = false
slaves = []
servers = []
```

On `iris`, put this in `~/.config/debkit/hosts/iris.toml`:

```toml
[nis]
enabled = true
role = "master"
domain = "dublinux.lan"
admin_user = "jallen"
local_admin_groups = ["sudo"]
push_to_slaves = true
slaves = ["spitfire.dublinux.lan", "laptop.dublinux.lan"]
```

On every normal client, put this in that host's override file:

```toml
[nis]
enabled = true
role = "slave"
domain = "dublinux.lan"
master = "iris.dublinux.lan"
prefer_local = true
force_refresh_maps = false
```

Plain NIS clients are supported only for machines that do not need offline NIS account
availability:

```toml
[nis]
enabled = true
role = "client"
domain = "dublinux.lan"
server = "iris.dublinux.lan"
```

`install nis` applies the configured role. `nis-client` and `nis-server` still exist as
compatibility targets, but the recommended role for normal machines is `slave`, not plain
`client`.

DebKit writes `/etc/defaultdomain`, `/etc/yp.conf`, and keeps `/etc/nsswitch.conf` local-first with
`files nis` for `passwd`, `group`, and `shadow`. It does not add or rewrite an explicit
`initgroups:` line; if one already exists, DebKit leaves it alone and warns when NIS supplementary
group lookup appears incomplete. Client-capable NIS installs validate lookup with `getent group`,
`getent initgroups`, and `id` for the configured NIS admin user and local admin groups. It never
removes or rewrites `/etc/passwd`, `/etc/shadow`, or `/etc/group`. Keep a local sudo-capable account
on every machine for recovery.

### Passwordless sudo

DebKit can grant passwordless sudo to a specific group and add configured users to that group. The
default group name is `superuser`:

```toml
[sudo_nopass]
enabled = true
group = "superuser"
add_current_user = true
users = ["jallen", "alice"]
nis_managed = false
```

Run `debkit install sudo-nopass` or add `sudo-nopass` to `foundation.install`. DebKit writes a
`/etc/sudoers.d/99-<group>-nopass` drop-in, preserves the regular `%sudo` rule when it is missing,
adds the configured users to the group, and validates the result with `visudo -c`. If the group is
managed by NIS instead of `/etc/group`, set `nis_managed = true`; DebKit will leave membership to
NIS and validate that NSS and sudo policy report the expected no-password access.

For `master`, DebKit installs the server and client pieces, enables `rpcbind`, `ypserv`, and
`ypbind`, initializes missing maps with `/usr/lib/yp/ypinit -m`, and rebuilds existing maps with
`make -C /var/yp`. Re-running the command is safe: initialization only happens when
`/var/yp/<domain>` does not exist. It also manages `/var/yp/ypservers` from the master hostname and
the configured `slaves` list, then explicitly rebuilds `/var/yp/<domain>/ypservers` with key/value
pairs where both key and value are the server hostname. When `push_to_slaves = true`, DebKit uses
Debian's `/usr/sbin/yppush` to push the known map set to each configured slave. Push failures fail
the run with the map and slave context; the reliable fallback is still to run `debkit configure nis`
on the slave.

For `slave`, DebKit installs the server and client pieces, first configures `/etc/yp.conf` to bind
to `iris.dublinux.lan` for bootstrap, enables `rpcbind`, `ypserv`, and `ypbind`, and runs
`sudo /usr/lib/yp/ypinit -s iris.dublinux.lan` when local maps do not exist yet. If `ypinit -s`
fails, DebKit falls back to direct map transfer with `/usr/lib/yp/ypxfr -h iris.dublinux.lan -d
dublinux.lan <map>`. Once local maps exist, it switches `/etc/yp.conf` to prefer `127.0.0.1` before
the master when `prefer_local = true`, then restarts `ypbind`.

To force-refresh replicated maps on a configured slave, run:

```bash
debkit configure nis
```

This temporarily points the slave at the master, runs direct forced `ypxfr` transfers for the known
Debian NIS map set, then restores the configured local-preferred binding and restarts `ypbind`.
Setting `force_refresh_maps = true` on a slave makes `debkit install nis` use the same forced pull
path during the normal install/configure run.

To add a slave to a master host config from another machine:

```bash
debkit configure nis add-slave --host iris spitfire.dublinux.lan
```

The command edits `~/.config/debkit/hosts/iris.toml`, validates that the selected host is a NIS
master, avoids duplicate entries, and prints the next master/slave commands to run.

Troubleshooting notes from the live Iris/Spitfire setup:

- `ypcat passwd.byname` working does not prove slave initialization will work.
- `ypcat ypservers` on the master must include each slave hostname.
- The generated `ypservers` map must have non-empty hostname values; blank `ypcat ypservers`
  output means it was built incorrectly.
- Debian `make -B` under `/var/yp` may not rebuild the generated `ypservers` map.
- `ypinit -s` can fail even when direct `ypxfr` works; first transfer may print
  `Cannot open old ... ignored`, which is normal for a new slave map.
- Do not prefer localhost on a slave before local maps exist and local `ypserv` is running.

### Wake-on-LAN

DebKit can inspect and enable standard wired Ethernet Wake-on-LAN:

```bash
debkit status wake-on-lan
debkit install wake-on-lan
debkit install wake-on-lan --dry-run
```

Run `debkit status wake-on-lan` on `spitfire` to capture the current NetworkManager state, wired
and wireless interfaces, optional `ethtool` Wake-on-LAN verification, active NetworkManager profile,
and the wake details needed by the TimeVault server.

DebKit defaults to NetworkManager-native Wake-on-LAN because `spitfire` appears to have used
NetworkManager without installing `ethtool`. NetworkManager mode does not install `ethtool`; if
`ethtool` is absent, DebKit reports that low-level NIC verification was skipped.

Default/NetworkManager config in `~/.config/debkit/config.toml`:

```toml
[wake_on_lan]
enabled = true
interfaces = "auto"
mode = "magic"
backend = "network_manager"
reference_host = "<current-hostname>"
```

Explicit `ethtool` config:

```toml
[wake_on_lan]
enabled = true
interfaces = ["enp9s0"]
mode = "magic"
backend = "ethtool"
```

`backend = "auto"` tries NetworkManager first when `nmcli` is available, NetworkManager is running,
the target interface is wired and managed, and an active connection profile exists. Otherwise it
falls back to `ethtool`, installs it using DebKit's apt convention if missing, writes
`/etc/systemd/system/debkit-wol@.service`, and enables `debkit-wol@<interface>.service`.

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
