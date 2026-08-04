# NIS

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

## Master and slave mechanics

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

## Troubleshooting

Notes from the live Iris/Spitfire setup:

- `ypcat passwd.byname` working does not prove slave initialization will work.
- `ypcat ypservers` on the master must include each slave hostname.
- The generated `ypservers` map must have non-empty hostname values; blank `ypcat
  ypservers` output means it was built incorrectly.
- Debian `make -B` under `/var/yp` may not rebuild the generated `ypservers` map.
- `ypinit -s` can fail even when direct `ypxfr` works; first transfer may print `Cannot
  open old ... ignored`, which is normal for a new slave map.
- Do not prefer localhost on a slave before local maps exist and local `ypserv` is
  running.
