# Module Reference

Every module registered for the `inspect`/`diagnose`/`plan`/`apply`/`verify` lifecycle
described in [Lifecycle Modules](../lifecycle-modules.md), in the order `debkit list`
prints them. Five are permanently read-only (no config section, `plan()` always
returns empty) because DebKit isn't positioned to safely decide the answer — a DHCP
server choice, a firewall rule, which systemd unit fix is correct. The other twelve
have a config section; each page below shows every field, annotated.

| Module | Config section | What it manages |
| --- | --- | --- |
| [core.inspect](./core-inspect.md) | none | Read-only baseline evidence: OS, kernel, failed units, watched packages, NICs |
| [network.interfaces](./network-interfaces.md) | `network_interfaces` | Interface inventory, manager-ownership conflicts, stable MAC-based naming |
| [network.dhcp](./network-dhcp.md) | none | Read-only DHCP server ownership conflict + client-backend detection |
| [network.dns](./network-dns.md) | `dns` | Declarative dnsmasq local zones/upstream, resolver-conflict detection |
| [network.firewall](./network-firewall.md) | none | Read-only backend/ruleset diagnostics, real TCP-reachability verification |
| [network.tailscale](./network-tailscale.md) | `tailscale` | Read-only Tailscale backend/DNS status |
| [network.wake_on_lan](./network-wake-on-lan.md) | `wake_on_lan` | Wake-on-LAN via NetworkManager or ethtool |
| [identity.nis](./identity-nis.md) | `nis` | NIS domain, `yp.conf`, `nsswitch.conf`, master-side map lifecycle |
| [identity.nss](./identity-nss.md) | none | Local vs. NIS UID/GID collision detection |
| [identity.pam](./identity-pam.md) | `pam` | `pam_mkhomedir.so` for create-home-on-first-login |
| [identity.sudo](./identity-sudo.md) | `sudo_nopass` | Passwordless-sudo group, NOPASSWD drop-in |
| [systemd.units](./systemd-units.md) | none | Read-only report of currently failed systemd units |
| [developer.git](./developer-git.md) | `git` | Global git credential helper and credential-store permissions |
| [apt.repositories](./apt-repositories.md) | `apt` | apt-cacher-ng proxy config and DIRECT-bypass exceptions |
| [hardware.grub](./hardware-grub.md) | `hardware_grub` | AM5 board/BIOS identification, memory-capacity check, GRUB `reboot=`/`GRUB_GFXMODE`/`GRUB_GFXPAYLOAD_LINUX` boot parameters |
| [hardware.sleep](./hardware-sleep.md) | `hardware_sleep` | Suspend/resume diagnostics, active `mem_sleep` mode |
| [hardware.rgb](./hardware-rgb.md) | `hardware_rgb` | `i2c-dev` prerequisite for motherboard/SMBus RGB control |

Most config-bearing modules follow the same shape: an `enabled: bool` gate (`false` by
default — nothing is enforced until you opt in), plus whatever fields declare the
desired state; `diagnose()`/`plan()` always report compliant/empty when `enabled:
false`, regardless of what `discover()` observes. Two exceptions: `identity.pam` gates
on `create_home_on_first_login` instead of a literal `enabled` field, and
`network.interfaces` has no gate at all — an empty `links` list is naturally a no-op
plan without needing one.
