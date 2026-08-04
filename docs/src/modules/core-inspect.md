# core.inspect

Read-only baseline evidence, not an enforced resource — it has no desired state to
compare against, so `diagnose()` never reports a mismatch and `plan()` is always
empty. `discover()` collects:

- Hostname, `/etc/os-release` (pretty name, ID, version), `uname -r`, boot ID
- Currently-failed systemd units (`systemctl --failed`)
- Installed versions of a fixed **watched package list** — subsystems that repeatedly
  turned out to be the actual owner of a setting during troubleshooting: DNS
  resolvers, network managers, Tailscale, NIS, firewalling. Absence from this list is
  not itself a finding; it's just not queried
- Network interface inventory (name, MAC, wired/wireless/other), excluding loopback

No config section — `core.inspect` always runs, and always succeeds with whatever it
can observe.

```yaml
# No config section for core.inspect. Just run:
#   debkit inspect core.inspect
```
