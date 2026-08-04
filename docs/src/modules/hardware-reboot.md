# hardware.reboot

Board/BIOS identification scoped narrowly to detection + normalization + a
known-affected-BIOS lookup — not a general hardware compatibility matcher. Motivated
by AM5-platform troubleshooting: a BIOS update can change memory training such that a
reboot silently drops to a lower memory speed or fails to detect a DIMM, with no
non-firmware signal that anything changed.

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
    # 16, 32, 64, 96, 128, ...). 0 means "not declared, don't check."
    # Compared against /proc/meminfo's MemTotal (also rounded up) -- a
    # coarse, root-free, no-dmidecode signal that catches a DIMM going
    # undetected after a reboot/BIOS change. This is a capacity check, not
    # a speed check (EXPO/XMP reverting to JEDEC needs `dmidecode --type
    # 17` and root, out of scope).
    expected_memory_gib: 128
```

The known-affected-BIOS registry ships **deliberately empty**: fabricating
compatibility data without verified sourcing would be worse than shipping none. The
mechanism is real — add entries to `~/.config/debkit/boards/registry.yaml`:

```yaml
boards:
  - vendor: Micro-Star International
    name: MAG X870E TOMAHAWK WIFI (MS-7E59)
    # Exact version strings, not a range -- vendor BIOS version strings
    # (MSI's "2.AC3", for example) aren't a consistently orderable scheme.
    affected_bios_versions: ["2.AC3"]
    note: "reverts EXPO profile after flash"
```
