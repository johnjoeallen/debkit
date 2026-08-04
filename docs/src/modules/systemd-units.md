# systemd.units

Read-only diagnostics: which systemd units are currently in a failed state, parsed
from `systemctl --failed`. There's no declared intent to enforce here (no `systemd`
config section), so `plan()` never produces changes — this module only ever reports.
`diagnose()` is a mismatch (not a hard `compliant: no` in the ownership-conflict
sense) whenever the failed-unit list is non-empty, since "some unit failed" is
inherently not a healthy steady state, but there's no automated fix: what's correct
for one failed unit (`systemctl restart`) is wrong for another (needs a config change
first), and DebKit isn't in a position to guess which.

```yaml
# No config section for systemd.units. Just run:
#   debkit diagnose systemd.units
```
