# Laptop with Tailscale

A machine that roams off the home LAN. Contrast with the [Full Example](../config-reference.md)
(a stationary desktop): this one turns on Tailscale and a local dnsmasq resolver, turns
*off* Wake-on-LAN (a laptop in a bag should never wake itself), and declares `s2idle`
instead of `deep` sleep — many laptops only reliably support `s2idle`, unlike the AM5
desktop boards `hardware.sleep`'s other examples target.

```yaml
{{#include ./laptop-tailscale.yaml}}
```

This is a full base config, not a host override — save it as
`~/.config/debkit/config.yaml` directly.
