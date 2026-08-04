# network.firewall

Read-only diagnostics, no config section, no `plan()`/`apply()` — DebKit does not
write firewall rules; a bad `nft`/`iptables` change can lock out the very SSH session
used to fix it. `discover()` reports the active backend (`iptables-nft`,
`iptables-legacy`, or `nftables`), ruleset table names when readable (`nft list
tables` needs root even to list an empty ruleset — when that fails, the reason is
surfaced honestly via a warning instead of silently reporting an empty ruleset), IPv4/
IPv6 forwarding sysctls, and Docker-published ports.

The centerpiece is `verify()`'s reachability check: a real `TcpStream` connect (no
external `nc`/`curl`) to each Docker-published TCP port via both loopback and the
host's LAN IP. Reachable via loopback but not the LAN IP is flagged explicitly — that
exact split is the doc's recurring "port is up but nothing can reach it externally"
failure signature, usually a DNAT/forwarding/firewall-rule problem on the LAN-facing
interface.

```yaml
# No config section for network.firewall. Just run:
#   debkit verify network.firewall
```
