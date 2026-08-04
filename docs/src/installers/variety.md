# Variety

```bash
debkit install variety
debkit status variety
```

Installs the [Variety](https://github.com/varietywalls/variety) GNOME wallpaper
rotator via apt, points it at the configured wallpapers folder and rotation interval,
and reports status (installed version, whether the wallpapers folder exists, whether
the autostart entry exists). `debkit status variety` runs the same status collection
without installing anything.

```yaml
debkit:
  wallpapers:
    folder: /home/jallen/Pictures/Wallpapers
  variety:
    interval_minutes: 10
```

Configuration runs for the invoking (or `SUDO_USER`-detected) target user, not root —
Variety is a per-user desktop app. On GNOME, if the tray icon is missing, DebKit notes
that's usually a missing AppIndicator extension, not a Variety problem; wallpaper
rotation itself still works without tray support.
