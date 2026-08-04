# Example Configs

The [Full Example](../config-reference.md) page shows every field DebKit understands,
which is useful as a schema reference but isn't what any real host's config actually
looks like — most fields stay at their defaults. The pages in this section are smaller,
realistic configs for specific situations, each validated against the real config loader
before being published here.

| Example | Shape | Shows |
| --- | --- | --- |
| [Minimal Workstation](./minimal-workstation.md) | full base config | A fresh dev machine with no lab-specific features |
| [NIS Master](./nis-master.md) | host override | The lab's NIS/apt-proxy master (`iris`) |
| [NIS Slave](./nis-slave.md) | host override | A normal NIS client host |
| [Laptop with Tailscale](./laptop-tailscale.md) | full base config | Roaming machine: Tailscale, local dnsmasq, no Wake-on-LAN, `s2idle` |
| [Host Override](./host-override.md) | host override | The base+host deep-merge mechanism itself, isolated from any one feature |

"Full base config" means the file is meant to stand alone as
`~/.config/debkit/config.yaml`. "Host override" means it's meant to be layered on top of
a shared base as `~/.config/debkit/hosts/<hostname>.yaml` — see
[Configuration](../configuration.md) for how the merge works.
