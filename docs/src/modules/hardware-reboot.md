# hardware.reboot

Board/BIOS identification scoped narrowly to detection + normalization + a
known-affected-BIOS lookup — not a general hardware compatibility matcher. Motivated
by AM5-platform troubleshooting: a BIOS update can change memory training such that a
reboot silently drops to a lower memory speed or fails to detect a DIMM, with no
non-firmware signal that anything changed.

**The memory-capacity and known-affected-BIOS findings themselves have no automated
fix.** DebKit can't reseat a DIMM or safely choose to flash a BIOS on your behalf —
reseating is physical, and a BIOS flash carries real bricking risk this tool isn't
going to take on your behalf. Both stay pure `diagnose()` findings for a human to act
on.

**What `plan()`/`apply()` do manage** is the shared underlying mitigation for both:
persisting `reboot_mode`/`reboot_type` into `/etc/default/grub`'s
`GRUB_CMDLINE_LINUX_DEFAULT` and regenerating `/boot/grub/grub.cfg` via `update-grub`.
`reboot_mode: cold` sets the kernel's `reboot=` parameter to force the BIOS cold-boot
flag on every future reboot — both findings above stem from a *warm* reboot skipping
full memory retraining, so declaring this is a real fix for the reboot-time symptom
even though it can't touch the DIMM or BIOS version directly. This is genuinely the
highest-risk write in DebKit today — a bad `/etc/default/grub` edit can leave a system
unbootable — so both changes are `Risk::High` in `debkit plan` output, and the
parsing that produces them is deliberately conservative (see below). This mitigation
is only ever proposed when there's an actual signal for it: a confirmed memory
mismatch, the current BIOS matching a known-affected registry entry, or the board
itself matching a registry entry that carries a `recommended_reboot_mode` — a
recognized board the registry already has a known-good answer for wins outright,
without needing its own separate confirmed-bad finding first. Enabling the module
and declaring `reboot_mode` isn't, by itself, reason enough to touch the bootloader.

Reads straight from `/sys/class/dmi/id/*` — no `dmidecode`, no root required. Missing
individual fields, or no DMI support on the platform at all, is a normal `None`
outcome, never a discovery error. `board_vendor`/`board_name` are case-folded,
whitespace-collapsed, and stripped of common vendor-suffix noise (`Co., Ltd.`,
`Corporation`, ...) before matching against the registry.

```yaml
debkit:
  hardware_reboot:
    enabled: false
    # Declared installed RAM, in GiB, rounded to the nearest common size (8,
    # 16, 32, 64, 96, 128, ...). 0 (the default) means "not declared, don't
    # check." Compared against /proc/meminfo's MemTotal (also rounded up) --
    # a coarse, root-free, no-dmidecode signal that catches a DIMM going
    # undetected after a reboot/BIOS change. This is a capacity check, not
    # a speed check (EXPO/XMP reverting to JEDEC needs `dmidecode --type
    # 17` and root, out of scope). A mismatch is diagnose()-only -- see
    # above -- but does contribute to whether the grub mitigation below is
    # considered needed.
    expected_memory_gib: 0
    # "cold" or "warm" -- the first component of the kernel's reboot=
    # parameter (man 2 reboot, LINUX_REBOOT_CMD_RESTART2). "cold" sets the
    # BIOS cold-boot flag, forcing a full memory retrain/POST on every
    # reboot. Empty (the default) defers to the matched board registry
    # entry's recommended_reboot_mode below, if any, and only then falls
    # back to "cold" -- an explicit value here always wins over the
    # registry.
    reboot_mode: ""
    # Optional second component of the same reboot= syntax: "bios", "acpi",
    # "kbd", "triple", "efi", "pci", or empty (default -- let the kernel
    # pick). Rendered together as "<reboot_mode>,<reboot_type>" when both
    # are set, or just reboot_mode alone when reboot_type is empty.
    reboot_type: ""
```

Both `reboot_mode` and `reboot_type` are validated at config load against exactly
those literal values — this file gets written into `/etc/default/grub`, which
`update-grub` sources as a shell script, so closing that injection surface matters
more here than almost anywhere else in this codebase.

The known-affected-BIOS registry is deliberately conservative about what ships in
the .deb-packaged system copy (`/usr/share/debkit/boards/registry.yaml`):
fabricating compatibility data without verified sourcing would be worse than
shipping none, so it only ever grows entries with real, verified sourcing. It
merges across three tiers — the compiled-in (empty) default, the
.deb-packaged system registry, then `~/.config/debkit/boards/registry.yaml` — with
each later tier overriding an earlier one on a matching `vendor`+`name`. A match
against the current BIOS version produces a `diagnose()` finding quoting the entry's
`note`, and — like the memory-capacity finding — contributes to whether the grub
mitigation is needed, but never triggers a flash directly:

```yaml
boards:
  - vendor: Micro-Star International
    name: MAG X870E TOMAHAWK WIFI (MS-7E59)
    # Exact version strings, not a range -- vendor BIOS version strings
    # (MSI's "2.AC3", for example) aren't a consistently orderable scheme.
    affected_bios_versions: ["2.AC3"]
    note: "reverts EXPO profile after flash"
    # Optional. Used as the effective reboot_mode when config leaves it
    # empty -- lets a recognized board "just know" the right mitigation
    # without the user having to declare reboot_mode themselves.
    recommended_reboot_mode: cold
```

## The grub write, precisely

`plan()` checks two things independently, and only proceeds if `update-grub` is
actually resolvable on the host (checked against `PATH` and the well-known
`/usr/sbin`/`/sbin` locations, since it's rarely on a regular user's `PATH`):

1. **Source**: does `/etc/default/grub`'s `GRUB_CMDLINE_LINUX_DEFAULT="..."` line
   already contain a `reboot=<mode>[,<type>]` token *anywhere* in its value (not just
   at the end)? If not, a `WriteFile` change patches just that one line — every other
   line, including a separate active `GRUB_CMDLINE_LINUX="..."` line (no `_DEFAULT`,
   a different variable entirely, and a real shape this parser was built and tested
   against), is left untouched. If the file doesn't contain a recognizable
   `GRUB_CMDLINE_LINUX_DEFAULT="..."` line at all, this module refuses to guess and
   surfaces a warning instead.
2. **Effective**: does the already-generated `/boot/grub/grub.cfg` reflect the
   desired token? This catches the case where someone hand-edited `/etc/default/grub`
   without running `update-grub` afterward. `/boot/grub/grub.cfg` is commonly
   root-only (`0600`) — when it can't be read, this check is skipped rather than
   treated as a mismatch, and `debkit verify`/`apply` (which do run privileged) are
   what actually confirm it.

Whenever either is stale, `plan()` includes a `RunCommand` for `update-grub` to
regenerate the effective config — writing the source without ever regenerating it
would leave a misleading half-applied state.

Rollback reverses the `WriteFile` automatically (the prior `/etc/default/grub`
content is restored). The `update-grub` regeneration itself is recorded as `Manual` —
the engine can't generically reverse an arbitrary command, so a rollback restores the
source file but doesn't re-run `update-grub` for you afterward; `debkit rollback`
reports that as a step you'd need to do by hand.
