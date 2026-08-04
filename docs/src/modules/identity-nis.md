# identity.nis

Covers the file/service layer for all three roles, plus **master-side** map lifecycle:
`ypinit -m` (when uninitialized), the `ypservers` source file and its rebuilt map,
`make -C /var/yp`, and (when `push_to_slaves: true`) `yppush` to declared slaves.

**Slave-side map lifecycle stays deliberately deferred** — the bootstrap-then-finalize
`yp.conf` dance, `ypinit -s` with its `ypxfr` fallback, and SSH-based master
registration are stateful, multi-host, and touch a live remote host (the master) on
top of the slave itself, a materially different risk profile from the self-contained
master-side steps. That stays the domain of the legacy `debkit install nis`/`debkit
configure nis` commands — see [NIS](../nis.md) for the full mechanics and
troubleshooting notes.

One real bug this module's build caught and fixed: the desired `yp.conf` content is
`maps_initialized`-aware, so a **slave** with no local maps yet is never told to
prefer `127.0.0.1` before local `ypserv` actually has anything to answer with — the
exact failure mode the requirements doc warns against.

```yaml
debkit:
  nis:
    enabled: false
    # master | slave | client. slave is recommended for normal machines
    # (keeps shared accounts available if the master is briefly down, once
    # local maps are initialized); client is for machines that don't need
    # offline account availability.
    role: slave
    domain: dublinux.lan
    # Defaults to $SUDO_USER or $USER if left blank.
    admin_user: jallen
    local_admin_groups: [sudo]
    # Required when role: slave.
    master: iris.dublinux.lan
    # Required when role: client and `servers` is empty.
    server: iris.dublinux.lan
    # Slave only: prefer the local ypserv (127.0.0.1) over the master once
    # local maps exist. See the maps_initialized note above for why this is
    # never applied before local maps are actually ready.
    prefer_local: true
    # Master only: push the map set to `slaves` via yppush after a rebuild.
    push_to_slaves: false
    # Slave only: force a direct ypxfr pull instead of the normal ypinit -s
    # path -- see `debkit configure nis` in the NIS chapter.
    force_refresh_maps: false
    # Master only: FQDNs written into /var/yp/ypservers.
    slaves: []
    # Client only, alternative to `server`: a list of candidate NIS servers.
    servers: []
```

`domain`/`admin_user` are required whenever `enabled: true`; `master` is required for
`role: slave`; `server` or `servers` is required for `role: client` — all validated at
config-load time, before any module runs.
