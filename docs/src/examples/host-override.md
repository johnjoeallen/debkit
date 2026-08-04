# Host Override

The other examples in this section show *what* to declare; this one isolates *how* the
base+host merge itself behaves, independent of any one feature.

Given a shared base `~/.config/debkit/config.yaml`:

```yaml
debkit:
  foundation:
    install: [essentials, git, ripgrep, rust, npm, wake-on-lan]
  wake_on_lan:
    enabled: true
    backend: network_manager
```

...this host override file:

```yaml
{{#include ./host-override.yaml}}
```

...produces an effective config where `wake_on_lan.enabled` is `false` (the host file's
scalar value wins) but `foundation.install` is untouched (the host file never mentioned
it, so the base value falls through unchanged).

The merge is a deep-merge over the YAML structure, not a per-field allowlist: any key
present in the host file overrides the corresponding key in the base, recursively for
nested maps; anything the host file doesn't mention is left exactly as the base
declared it. Lists (like `foundation.install` or `nis.slaves`) are replaced wholesale
when a host file declares them at all — there's no element-level merging of arrays.
