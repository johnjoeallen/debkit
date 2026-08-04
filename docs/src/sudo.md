# Passwordless Sudo

DebKit can grant passwordless sudo to a specific group and add configured users to that
group. The default group name is `superuser`:

```yaml
debkit:
  sudo_nopass:
    enabled: true
    group: superuser
    add_current_user: true
    users: [alex, alice]
    nis_managed: false
```

Run `debkit install sudo-nopass` or add `sudo-nopass` to `foundation.install`. DebKit
writes a `/etc/sudoers.d/99-<group>-nopass` drop-in, preserves the regular `%sudo` rule
when it is missing, adds the configured users to the group, and validates the result
with `visudo -c`. If the group is managed by NIS instead of `/etc/group`, set
`nis_managed: true`; DebKit will leave membership to NIS and validate that NSS and sudo
policy report the expected no-password access. `debkit diagnose identity.sudo` covers
the same checks read-only.
