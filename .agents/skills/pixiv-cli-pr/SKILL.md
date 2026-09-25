---
name: pixiv-cli-pr
description: Prepare, update, or verify a pixiv-cli pull request using its current template, reviewed diff, and head-specific check results. Use for contributor handoff or PR readiness; merging and releases are separate actions.
---

# Prepare a pixiv-cli PR

## Verify scope

Read `AGENTS.md`, the request/issue, branch status, and the diff against the intended base. Preserve unrelated changes and verify the remote repository before pushing. Use [the review workflow](../pixiv-cli-review/SKILL.md) and [test workflow](../pixiv-cli-test/SKILL.md); record actual outcomes and blockers.

## Write the body

This fork has no PR metadata parser and no PR template file. Write a concise body in English, or the language explicitly requested for the PR, covering:

- **Changes** — what actually changed and why.
- **Verification** — the commands that ran and their outcomes; separate unrun, native, and live checks.
- **Notes** — unmet conditions, known risks, or decisions a reviewer must confirm.

State facts only. Do not invent release-note fields, version decisions, historical problems, or unchanged non-features. Keep secrets, private targets, and state-changing account commands out of a public PR.

## Publish and inspect

When authorized, push the dedicated branch and create/update the PR against the verified base. Do not force-push, merge, change protections, or release implicitly. Read checks on the **current head SHA**, not an earlier green revision. Use [the CI workflow](../pixiv-cli-ci/SKILL.md) for failures.

`ci.yml` runs the full `Quality gate` on every pull request; there is no per-path skip, no smoke coordinator, and no `/test` command in this fork. `platform-smoke.yml`, `container-smoke.yml`, and `browser-evidence.yml` are manual workers that nothing dispatches automatically, so their absence from a PR is expected rather than missing evidence.

Deliver the PR URL, commit, actual validation, and pending blockers. Distinguish a review-ready draft from a merge-ready PR; do not mark required checks successful because their workflow exists.
