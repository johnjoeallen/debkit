# Codex CLI

```bash
debkit install codex [--node-version <version>]
debkit uninstall codex
```

Installs the [OpenAI Codex CLI](https://www.npmjs.com/package/@openai/codex) as a
global npm package. `install codex` first ensures the `npm` target (above) is
installed at `--node-version` (default `latest`), then runs `npm install -g
@openai/codex` through DebKit's managed npm with a per-user `NPM_CONFIG_PREFIX` —
never a system-wide global install. Re-running when `codex` is already present is a
no-op; `uninstall` runs the matching `npm uninstall -g` and verifies the binary is
actually gone afterward.

No config section of its own — `--node-version`/`foundation`'s `codex` target both
flow through to the same `npm.version`-driven Node.js install described above.
