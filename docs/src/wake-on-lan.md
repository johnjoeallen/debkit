# Wake-on-LAN

DebKit can inspect and enable standard wired Ethernet Wake-on-LAN:

```bash
debkit status wake-on-lan
debkit install wake-on-lan
debkit install wake-on-lan --dry-run
```

The same functionality is also available as the `network.wake_on_lan` lifecycle module
(`debkit diagnose|plan|apply network.wake_on_lan`), which adds ownership-conflict
detection between NetworkManager and a `debkit-wol@` systemd unit both claiming the
same interface.

Run `debkit status wake-on-lan` on `spitfire` to capture the current NetworkManager
state, wired and wireless interfaces, optional `ethtool` Wake-on-LAN verification,
active NetworkManager profile, and the wake details needed by the TimeVault server.

DebKit defaults to NetworkManager-native Wake-on-LAN because `spitfire` appears to have
used NetworkManager without installing `ethtool`. NetworkManager mode does not install
`ethtool`; if `ethtool` is absent, DebKit reports that low-level NIC verification was
skipped.

Default/NetworkManager config in `~/.config/debkit/config.yaml`:

```yaml
debkit:
  wake_on_lan:
    enabled: true
    interfaces: auto
    mode: magic
    backend: network_manager
    reference_host: <current-hostname>
```

Explicit `ethtool` config:

```yaml
debkit:
  wake_on_lan:
    enabled: true
    interfaces: [enp9s0]
    mode: magic
    backend: ethtool
```

`backend: auto` tries NetworkManager first when `nmcli` is available, NetworkManager is
running, the target interface is wired and managed, and an active connection profile
exists. Otherwise it falls back to `ethtool`, installs it using DebKit's apt convention
if missing, writes `/etc/systemd/system/debkit-wol@.service`, and enables
`debkit-wol@<interface>.service`.

Wake info is written after configuration:

```text
/var/lib/debkit/wake-on-lan/<hostname>.txt
/var/lib/debkit/wake-on-lan/<hostname>.json
```

From TimeVault:

```bash
wakeonlan <mac>
sudo etherwake -i <timevault-interface> <mac>
```

Troubleshooting checks:

- BIOS/UEFI Wake-on-LAN or PCIe wake is disabled.
- The selected interface is Wi-Fi or the wrong wired NIC.
- NetworkManager is not managing the interface.
- `ethtool` is missing when `backend = "ethtool"` is requested and apt cannot install it.
- Wake-on-LAN is not persistent after reboot.
- The machine loses standby power when shut down.
- VLAN, subnet, or broadcast routing prevents the magic packet from reaching the target.
