# network.wake_on_lan

Lifecycle-module counterpart to the [Wake-on-LAN](../wake-on-lan.md) installer target
and `debkit install wake-on-lan` — same underlying behavior, with ownership-conflict
detection added: `diagnose()` reports a conflict when both NetworkManager and a
`debkit-wol@` systemd unit appear to be actively claiming the same interface.

```yaml
debkit:
  wake_on_lan:
    enabled: true
    # "auto" (autodetect wired interfaces) or an explicit list: [enp5s0].
    interfaces: auto
    # Only "magic" is currently supported (validated at config load).
    mode: magic
    # network_manager | ethtool | auto. "auto" tries NetworkManager first
    # when nmcli is available, NetworkManager is running, the interface is
    # wired and managed, and an active connection profile exists; otherwise
    # falls back to ethtool (installing it if missing) via a
    # debkit-wol@<interface>.service systemd unit.
    backend: network_manager
    # Used in the wake-info evidence written after apply -- defaults to the
    # current hostname if left blank.
    reference_host: tornado
```

`apply()` writes `/var/lib/debkit/wake-on-lan/<hostname>.{txt,json}` with the wake
details (MAC, interface, backend) needed by an external waker like TimeVault.
