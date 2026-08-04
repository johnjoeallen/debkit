# Git Prompt

```bash
debkit configure git-prompt
```

Writes `~/.git-prompt.sh` (sourcing `/usr/lib/git-core/git-sh-prompt` and setting
`GIT_PS1_SHOWDIRTYSTATE`/`GIT_PS1_SHOWSTASHSTATE`/`GIT_PS1_SHOWUNTRACKEDFILES`/
`GIT_PS1_SHOWUPSTREAM=auto`, plus a `PS1` that shows the current git branch/dirty
state) for the invoking user, then appends a small `if [ -f ~/.git-prompt.sh ]; then . ...; fi`
block to `~/.bashrc` if it isn't already there (checked by substring match on the
prompt file's path). Idempotent: re-running only rewrites `~/.git-prompt.sh` if its
content has actually drifted, and never appends a duplicate block to `~/.bashrc`.

No config section — this is a fixed, opinionated prompt, not a customizable one.
