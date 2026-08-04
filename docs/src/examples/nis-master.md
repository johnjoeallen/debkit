# NIS Master

The lab's NIS master (`iris`) also runs the apt-cacher-ng proxy every other host points
at. This is a **host override** file — it only declares what differs from the shared
base config, per the pattern in [NIS](../nis.md).

```yaml
{{#include ./nis-master.yaml}}
```

Save as `~/.config/debkit/hosts/iris.yaml`, then:

```bash
debkit install nis
debkit install sudo-nopass
```
