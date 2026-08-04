# developer.git

Global-scope `credential.helper` conflict detection/repair, plus a credential-store
file permission check (must be `0600`, since it can hold plaintext credentials).
"Global scope" means the invoking user's `~/.gitconfig` or `~/.config/git/config`
specifically — a helper set at system scope (`/etc/gitconfig`) or inside a single
repo's `.git/config` is surfaced as a finding but left alone, since fixing either
needs a different privilege level or isn't this module's business to touch.

```yaml
debkit:
  git:
    enabled: false
    # "store" (persists to credential_store_file, plaintext), "cache"
    # (in-memory, times out), "none" (no helper managed -- diagnose only),
    # or an explicit `credential.helper` command string.
    credential_helper: store
    # Only consulted when credential_helper: store. Empty means git's own
    # default (~/.git-credentials).
    credential_store_file: ""
```

Building this module required an engine-level fix: `git config --global`/`chmod` on
the invoking user's own files must run as that user, never under `sudo` — running
privileged would silently operate on root's `$HOME` instead. That's why
`Change::RunCommand` carries a `privileged: bool` flag rather than always escalating.
