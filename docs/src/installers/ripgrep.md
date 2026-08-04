# ripgrep

```bash
debkit install ripgrep
debkit uninstall ripgrep
```

Installs `ripgrep` via apt if `rg` isn't already on `PATH`, prints `rg --version`
either way. `uninstall` removes the `ripgrep` package via apt and verifies `rg` is no
longer resolvable afterward. No config section.
