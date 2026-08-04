# identity.pam

Declarative `create_home_on_first_login` via a `pam_mkhomedir.so` line appended per
configured PAM service — idempotent, never duplicates an existing line, and never
touches a service whose PAM file doesn't exist on this host (not installed, not a
mismatch).

```yaml
debkit:
  pam:
    # The gate for this module -- there's no separate `enabled` field.
    create_home_on_first_login: false
    # Which /etc/pam.d/<service> files get the pam_mkhomedir.so line.
    services: [login, sshd]
    skeleton: /etc/skel
    umask: "0022"
```
