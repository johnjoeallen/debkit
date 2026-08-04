# apt.repositories

Manages `/etc/apt/apt.conf.d/01debkit-proxy` and
`/etc/apt/apt.conf.d/02debkit-proxy-exceptions`, but reads back the **effective**
config via `apt-config dump` rather than just its own two files — apt merges
`apt.conf.d` alphabetically, so another file could easily be overriding or
duplicating what DebKit wrote, and only the effective value actually governs apt's
behavior.

```yaml
debkit:
  apt:
    enabled: false
    # e.g. "http://10.0.0.1:3142" for an apt-cacher-ng proxy. Empty means no
    # proxy is declared/managed.
    proxy: "http://iris:3142"
    # Hosts that bypass `proxy` via a per-host DIRECT override -- anything
    # that shouldn't go through the cache (its own package repo, a service
    # with its own CDN).
    direct_hosts: [pkg.tailscale.com]
```

`verify()` does a live `curl --noproxy` reachability check against a `direct_hosts`
entry — this module was built and validated directly against a real apt-cacher-ng
setup, and caught exactly the gap the requirements doc describes: Tailscale's own apt
repo missing from the DIRECT exceptions, silently going through the proxy instead of
straight to the internet.
