# identity.nss

Read-only, no config section: local vs. NIS identity collisions, and whether local
recovery access (`root`) still resolves without NIS in the loop. `nsswitch.conf`
*content* is owned by [identity.nis](./identity-nis.md)'s `plan()`/`apply()` — this
module's distinct job is comparing local `/etc/passwd`/`/etc/group` against what NIS
actually serves (`ypcat passwd.byname`/`group.byname`) to catch UID/GID collisions,
plus a `getent -s files passwd root` check that local recovery access survives even if
NIS is unreachable.

There's nothing here to declare or apply — a UID/GID collision isn't something DebKit
should silently resolve on your behalf.

```yaml
# No config section for identity.nss. Just run:
#   debkit diagnose identity.nss
```
