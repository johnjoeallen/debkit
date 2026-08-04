# network.dhcp

Read-only, no config section: DHCP server ownership (competing-server detection is the
whole point — DebKit has no business deciding which DHCP server a host should run) and
which client backend is managing lease acquisition. UDP/67 listeners are the ground
truth for "is a DHCP server actually running here," independent of which package
provided it.

`discover()` checks `isc-dhcp-server` and `dnsmasq` (only counted as a DHCP server if
it also has a `dhcp-range` directive configured — a plain DNS-only dnsmasq doesn't
count), plus which client backend (NetworkManager, `systemd-networkd`, `dhclient`) is
handling lease acquisition. `diagnose()` reports a `conflict` — not a mismatch — if
more than one server is active at once; `plan()` stays empty either way.

```yaml
# No config section for network.dhcp. Just run:
#   debkit diagnose network.dhcp
```
