# AeroShoot.AI

The project's single source of truth is [`AEROSHOOT_MASTER_PLAN.md`](AEROSHOOT_MASTER_PLAN.md).

The companion video editor has been split out and now lives in its own repository: [`CreaperLost/Automated-Editor`](https://github.com/CreaperLost/Automated-Editor).

## Skills

Agent skills for this project live under `.minimax/skills/` and are **not** committed (the directory is git-ignored). `skills-lock.json` at the repo root is the manifest of what should be installed; it is kept locally and not committed either.

To install the project's skills inside the MiniMax Code runtime, from the repo root:

```sh
npx skills add leonardomso/rust-skills --agent minimax-code --yes
```

After installing, restart MiniMax Code (or reload the workspace) so the new skills are picked up. Re-run the command any time `skills-lock.json` changes.

The Rust skills package is used when writing, reviewing, or refactoring Rust code; invoke it with `/rust-skills` or rely on automatic application in qualifying contexts.
