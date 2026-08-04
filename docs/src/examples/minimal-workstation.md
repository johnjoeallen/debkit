# Minimal Workstation

A fresh dev workstation with none of the lab-specific features turned on — no NIS, no
hardware lifecycle modules. This is roughly what you're left with after `debkit
host-config` if you only touch the toggles you actually need.

```yaml
{{#include ./minimal-workstation.yaml}}
```

Apply it with:

```bash
debkit install foundation
```
