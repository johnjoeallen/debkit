# network.tailscale

Deliberately read-only beyond the "installed and running" check. `tailscale status
--json` is a stable, documented interface and is used directly for `discover()`
(backend state, `DNSName`, Tailscale IPs, MagicDNS suffix, and whether
`/etc/resolv.pre-tailscale-backup.conf` exists — the well-documented signal that
`tailscaled` took over `/etc/resolv.conf`, backing up the pre-existing file first).

The CLI surface for changing DNS-acceptance behavior (`tailscale set
--accept-dns=...`) is real, but its interaction with `magic_dns_off_lan`/
`preserve_lan_dns` — an on-LAN vs. off-LAN split Tailscale itself doesn't natively
express — isn't something this module confidently automates without a live Tailscale
install to validate against.

```yaml
debkit:
  tailscale:
    enabled: false
    # Informational only -- surfaced as context in the Observation, not
    # enforced. See the module rationale above for why.
    magic_dns_off_lan: true
    preserve_lan_dns: true
```

`plan()`/`apply()` always stay empty; installing/starting `tailscaled` is left to the
operator.
