# AeroShoot.AI

The target architecture is documented in [`AEROSHOOT_MASTER_PLAN.md`](AEROSHOOT_MASTER_PLAN.md). The shorter, current defect list and acceptance-driven execution order live in [`ACTIVE_RECORDING_WORK.md`](ACTIVE_RECORDING_WORK.md).

The companion video editor has been split out and now lives in its own repository: [`CreaperLost/Automated-Editor`](https://github.com/CreaperLost/Automated-Editor).

## Skills

This project pins its agent skills in [`skills-lock.json`](skills-lock.json) — a manifest that records each skill's source, path, and integrity hash. The lockfile is committed; the actual installed files under `.minimax/skills/` are not (the directory is git-ignored).

To install everything the lockfile declares, from the repo root:

```sh
npx skills add leonardomso/rust-skills --agent minimax-code --yes
```

After installing, restart MiniMax Code (or reload the workspace) so the new skills are picked up. Re-run the install command any time `skills-lock.json` changes.

The Rust skills package is used when writing, reviewing, or refactoring Rust code; invoke it with `/rust-skills` or rely on automatic application in qualifying contexts.
