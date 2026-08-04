# hardware.sleep

Suspend/resume-reliability diagnostics plus one narrow, safe piece of declarative
state: the active `/sys/power/mem_sleep` mode. Motivated by desktop suspend
troubleshooting — platforms that boot with the often power-hungry, sometimes
unreliable-to-resume `s2idle` mode active when `deep` (real S3) is supported and
preferred.

Everything is read without root: `/sys/power/mem_sleep`, `/sys/power/state`, and
`/proc/acpi/wakeup` are world-readable; `busctl get-property`/`call` against
`org.freedesktop.login1` are read-only D-Bus calls any user can make.

```yaml
debkit:
  hardware_sleep:
    enabled: false
    # "s2idle", "deep", or "" (don't enforce). Validated against exactly
    # these three values -- it's embedded directly in a shell command in
    # plan(), so validation closes the injection surface before it's ever
    # reachable. Runtime-only: reverts to the kernel/firmware default on
    # next boot unless a mem_sleep_default= kernel parameter is also set
    # (a bootloader edit, out of scope here).
    desired_mem_sleep: deep
```

Enabled wakeup devices and sleep inhibitors are captured in `discover()`'s
Observation for troubleshooting, but deliberately **never** `diagnose()` findings: a
real desktop routinely has dozens of legitimately-enabled ACPI wakeup sources and
several routine GNOME/NetworkManager/UPower inhibitor locks — flagging any of them as
non-compliant would be false-positive noise on a perfectly healthy system.

`verify()` never triggers an actual suspend/resume cycle (it would kill the SSH
session running `debkit` and require a physical wake) — it re-checks platform
suspend-to-RAM capability and, if declared, that `mem_sleep` actually holds the
declared value.
