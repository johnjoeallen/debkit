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

See the [Module Reference](./modules/README.md) for all 17 registered modules — each
with a description, its `plan()`/`apply()` scope, and (for the twelve with one) a
fully annotated config section. `debkit list` prints the same set live.

Some deliberately stop at diagnostics: `network.dhcp`/`network.firewall`/`systemd.units`
never write config (DebKit isn't positioned to choose a DHCP server or safely rewrite
firewall rules over the same connection you'd use to fix a mistake). `network.tailscale`
diagnoses but doesn't enforce DNS-acceptance behavior. `identity.nis` manages the
file/service layer and master-side map lifecycle, but NIS slave-side map bootstrapping
still goes through the legacy `debkit install nis`/`debkit configure nis` commands
described in the [NIS](./nis.md) chapter — that's genuinely stateful, multi-host
orchestration that doesn't fit the current `Change` primitives yet.
