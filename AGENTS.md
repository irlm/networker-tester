# AGENTS.md — rules for coding agents working in this repo

Read by every agent that edits this repo (Claude Code imports this file from
`CLAUDE.md`; Codex, Cursor, Copilot and friends pick it up by name). Humans
are welcome to follow it too. The point: an agent produces code fast, so the
only thing standing between "fast" and "slop" is how strictly it checks its
own output before calling the work done.

## 1. Lint as strictly as the tree allows — before you say "done"

One command covers every language in the repo and is the same command CI
runs (CI's lint jobs call it, so green here is green there):

```bash
scripts/lint-all.sh --no-build            # the ~1-2 min pre-push pass; MUST be clean
scripts/lint-all.sh                       # full pass incl. clippy / rustdoc / dotnet build — run before opening a PR
scripts/lint-all.sh --fix --only <group>  # let the tool fix what it can, then re-check
scripts/lint-all.sh --list                # sections, groups, tiers, required tools
```

Rules:

- **`block` sections must pass.** Never finish a task, open a PR, or report
  "done" with a failing `block` section. Not "CI will tell us" — you tell CI.
- **`info` sections: leave them better than you found them.** They carry a
  known baseline (`shellcheck-all`, `psa-all`, `hadolint`, `actionlint-sc`,
  `dotnet-format`, …). When you touch a file those sections cover, run the
  group with `--strict` (`scripts/lint-all.sh --strict --no-build --only shell`),
  fix every finding in the code you wrote or changed, and take the cheap
  pre-existing ones in the same file while you are there. The file's count
  must never be higher after your change than before it.
- **Promote, don't tolerate.** When you clean a section's baseline, flip its
  tier from `info` to `block` in the `SECTIONS` table of
  `scripts/lint-all.sh` in the same PR. CI enforces the promotion
  automatically because it runs the same script.
- **A missing tool is not a pass.** The script prints the install command for
  anything it skips; install it (or set `LINT_DOCKER=1` for shellcheck /
  actionlint / hadolint) rather than reporting a skipped section as clean.
- **Suppressions need a reason, in the code, next to the suppression.**
  `#[allow(...)]`, `// eslint-disable-next-line`, `# shellcheck disable=`,
  `#pragma warning disable`, `// nolint`: every one carries a one-line "why"
  comment. A suppression without a reason is a finding.
- **Tests are part of the lint bar.** `cargo test --workspace --lib`,
  `dotnet test Networker.sln`, `bats tests/installer.bats`, and
  `cd dashboard && npm test` for whatever you touched — and the repo's rule
  from `CLAUDE.md` § Quality Checks: test the real end-to-end path, not just
  the unit seam.

## 2. What "slop" looks like here (and what to do instead)

| Smell | Do instead |
|---|---|
| A lint disabled to make the bar go green | Fix the code; if the rule is genuinely wrong for this site, suppress with a reason |
| `unwrap()` / `expect()` / `!` / `as any` in non-test code | `anyhow::Result` + `.context()`, typed nulls, a real error path (`CLAUDE.md` § Rust / Cargo) |
| A new helper that duplicates one that exists | `grep` first — `CliComputeProvisioner.BuildGcloudEnv`, `SecretFile`, `shared/*.json` loaders, `_gcp_*` in install.sh exist for a reason |
| Version bumped in four of the five files | All five (`CLAUDE.md` § Version Sync); `scripts/lint-all.sh --only version` checks the agreement |
| A bash change that uses `declare -A`, `readarray`, `[[ -v ]]` | install.sh is Bash 3.2 (`CLAUDE.md` § Installer Constraints); dev tooling under `scripts/` may use bash 4 |
| Documentation that asserts a flag/env value nobody ran | Run it (`CLAUDE.md` § Documentation) |
| "Tests pass" with one class run in isolation | Run the suite you touched; tests that mutate process-global state belong in an xUnit collection |

## 3. Scope discipline

- Do the task that was asked; don't widen it into a refactor. If you find a
  real adjacent defect, fix it when it is small and clearly related (say so
  in the PR), otherwise open an issue.
- Every PR bumps the version (five files + `Cargo.lock` via
  `cargo update --workspace --offline` — never `cargo generate-lockfile`,
  which re-resolves the whole tree) and adds a `CHANGELOG.md` section.
- Never commit to `main`; branch → PR → merge (`CLAUDE.md` § Git Workflow).
