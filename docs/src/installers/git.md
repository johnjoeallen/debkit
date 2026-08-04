# Git

```bash
debkit install git
```

Installs `git` via apt if it isn't already on `PATH`, then prints `git --version` either
way. That's the entire target — no config, no post-install setup. For the global
`credential.helper` diagnostics, see the `developer.git` lifecycle module
(`debkit diagnose|plan|apply developer.git`) in
[Lifecycle Modules](../lifecycle-modules.md).
