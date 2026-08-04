# identity.sudo

Lifecycle-module counterpart to the [Passwordless Sudo](../sudo.md) installer target —
group creation, the standard `%sudo` rule (added if missing, never removed), a secured
(`root:root`, `0440`) NOPASSWD drop-in at `/etc/sudoers.d/99-<group>-nopass`, legacy
drop-in-file cleanup, and user-to-group membership.

```yaml
debkit:
  sudo_nopass:
    enabled: false
    group: superuser
    add_current_user: true
    users: [jallen, alice]
    # When true, group membership is left to NIS instead of local
    # /etc/group -- this module then only validates that NSS/sudo policy
    # report the expected no-password access, and skips local group
    # creation/membership management entirely.
    nis_managed: false
```

`diagnose()` reports which of the configured users are missing entirely vs. present
but not in the group, so a mismatch finding tells you exactly what's wrong rather than
just "not compliant."
