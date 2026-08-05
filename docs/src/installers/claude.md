# Claude Code CLI

```bash
debkit install claude [--node-version <version>]
debkit uninstall claude
```

Installs the [Claude Code CLI](https://www.npmjs.com/package/@anthropic-ai/claude-code)
entirely as the invoking user — **root is never involved**, neither for the initial
install nor for updating it later. `install claude` first ensures the `npm` target
(above) is installed at `--node-version` (default `latest`), then runs `npm install -g
@anthropic-ai/claude-code` through DebKit's managed npm with `NPM_CONFIG_PREFIX`
pointed at a per-user directory. `-g` here means "global to that per-user prefix," not
"global to the system" — it's the same mechanism `nvm`/`fnm`-style per-user Node
installs use, not the classic `sudo npm install -g` that leaves files root-owned and
makes every future `npm update`/`npm install -g` need `sudo` again. Re-running `debkit
install claude` to pick up a new version is the identical unprivileged command;
`uninstall` runs the matching `npm uninstall -g` and verifies the binary is actually
gone afterward.

No config section of its own — `--node-version`/`foundation`'s `claude` target both
flow through to the same `npm.version`-driven Node.js install described above.
