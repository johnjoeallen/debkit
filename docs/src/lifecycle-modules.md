# Lifecycle Modules

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
described in the [NIS](./nis.md) chapter — that's genuinely stateful, multi-host
orchestration that doesn't fit the current `Change` primitives yet.
