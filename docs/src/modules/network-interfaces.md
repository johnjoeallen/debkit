# network.interfaces

Mostly read-only: interface inventory, active network-manager ownership conflicts,
forwarding, and `rp_filter`. The one piece of declared state is stable MAC-based
interface naming via systemd `.link` files — self-contained because systemd-udevd
applies it independent of which manager configures the interface afterward. It only
takes effect on the next boot/udev reload, **never live**. Full role-based WAN/LAN
configuration (addresses, forwarding, per-interface manager selection) is deferred —
that requires choosing/switching network managers, real Phase-2-shaped complexity.

```yaml
debkit:
  network_interfaces:
    # Zero or more MAC -> stable-name declarations. Each entry renders a
    # /etc/systemd/network/10-debkit-<name>.link file pinning that MAC to that
    # interface name. Nothing here is applied live -- takes effect on next
    # boot or `udevadm trigger`/reload.
    links:
      - mac: "aa:bb:cc:dd:ee:ff"
        name: lan0
```

`diagnose()` also reports (informationally, not as a `links`-driven finding) when more
than one network manager appears to actively own the same interface — the classic
NetworkManager-vs-systemd-networkd-vs-ifupdown conflict.
