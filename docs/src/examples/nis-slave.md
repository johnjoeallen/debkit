# NIS Slave

The recommended shape for every normal client machine — `role: slave` keeps shared NIS
users/groups available even when the master is unreachable, once local maps have been
initialized and synchronized. See [NIS](../nis.md) for the full master/slave mechanics.

```yaml
{{#include ./nis-slave.yaml}}
```

Save as `~/.config/debkit/hosts/<hostname>.yaml`, then:

```bash
debkit install nis
```
