# Claude Code CLI

```bash
debkit install claude [--version <version-or-channel>]
debkit uninstall claude
```

Installs the [Claude Code CLI](https://claude.com/product/claude-code) using
Anthropic's own native installer — **root is never involved**, and this target does
*not* go through npm or [Node.js](./npm.md) at all: `install claude` runs the
equivalent of `curl -fsSL https://claude.ai/install.sh | bash`, which downloads a
prebuilt binary straight to `$HOME/.local/bin/claude` (symlinked from
`$HOME/.local/share/claude/versions/`). `--version` (default `latest`) is passed
through to the installer script and accepts either a release channel (`latest` or
`stable`) or a specific version number, e.g. `--version stable` or `--version 2.1.89`.

Re-running `debkit install claude` is a no-op once `claude` is present — the native
installer manages its own background auto-updates from then on (run `claude update` to
update it manually, or see [Anthropic's docs](https://code.claude.com/docs/en/setup) for
disabling auto-update or pinning a version). `uninstall` removes
`$HOME/.local/bin/claude` and `$HOME/.local/share/claude` (the installed binary
versions), but deliberately leaves `~/.claude` and `~/.claude.json` (your settings, MCP
config, and session history) untouched — those aren't part of the install this target
manages.

No config section of its own, and no `npm`/Node.js prerequisite — `--version`/
`foundation`'s `claude` target both flow straight into the native installer script.
