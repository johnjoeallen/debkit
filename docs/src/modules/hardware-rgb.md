# hardware.rgb

Manages exactly one thing declaratively: the `i2c-dev` kernel module, loaded now and
declared via `/etc/modules-load.d/debkit-i2c-dev.conf` to load on every future boot.
This is the standard, low-risk, reversible (`rmmod`) prerequisite for OpenRGB (or
anything else) to see a motherboard's SMBus-based RGB controller via `/dev/i2c-*` —
without it, an adapter driver like `i2c-piix4` can be loaded with nothing exposing a
usable device node, and RGB control silently just doesn't work with no error message
anywhere.

```yaml
debkit:
  hardware_rgb:
    enabled: false
```

That's the entire config — one field. Deliberately out of scope for `plan()`/`apply()`:

- **Never executes the `openrgb` binary itself.** Its runtime behavior against real
  hardware (bus/device scanning) can't be verified generically, and there's no
  command-timeout primitive in the shared exec layer to safely bound a call that might
  hang scanning an SMBus. `openrgb_installed` in the Observation is a `command -v`
  check only.
- **No device enumeration, color state, or lighting profiles.** There's no reliable
  way to read back "what color is this LED right now" to converge against, unlike a
  config file's content — and re-implementing OpenRGB's own hardware-ID database
  would just duplicate a project that already does this well.
- **A missing OpenRGB udev rule** (needed for non-root USB-RGB-controller access) is
  surfaced as a warning, not auto-fixed — the correct rule/group varies by
  distro/packaging, and guessing wrong risks granting broader device access than
  intended.
