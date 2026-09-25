---
name: pixiv-cli-ci
description: Diagnose pixiv-cli GitHub Actions failures and verify current-head quality, platform, native/browser, and release evidence. Use for CI readiness, failed or stalled runs, or an explicitly authorized rerun/cancel/dispatch; default to read-only evidence gathering.
---

# Diagnose pixiv-cli CI

Read `AGENTS.md`, the exact changed workflow, and its invoked script/tool. Workflow YAML owns ordering, inputs, permissions, and conditions; `ci/platforms.json` plus `tools/platformmatrix` own platform selection. Do not infer executable policy from a historical plan or a skill's old command example.

## Identify the failure

Use the connected GitHub tools or an already available authenticated `gh`. Establish repository, PR/head SHA, workflow, run ID, attempt, event, job, and failing step. Read the first causal error and relevant surrounding log, not just the final failure line. Never display tokens, account stores, signed URLs, or raw secret-bearing responses.

Useful read-only commands, with verified placeholders, are:

```bash
gh pr checks PR_NUMBER
gh run view RUN_ID --json name,event,headBranch,headSha,status,conclusion,jobs,url
gh run view RUN_ID --log-failed
gh run view --job JOB_ID --log
```

Classify deterministic code/contract failure, missing environment, upstream/network failure, cancellation, or genuine infrastructure flakiness. A long-running job is not automatically hung. Gather bounded relevant output, but preserve complete logs where needed instead of truncating source evidence.

## Select the owning checks

Use [pixiv-cli-test](../pixiv-cli-test/SKILL.md) for local reproduction.

`ci.yml` owns the read-only `Quality gate`, triggered by `pull_request` on `main` and by manual dispatch. This fork deliberately carries no change-scope classifier and no trusted PR-smoke coordinator, so the gate is a single job that always runs the full suite with `contents: read`; there is no per-path skip decision to audit and no `Skipped` Quality job to expect.

`platform-smoke.yml` and `container-smoke.yml` are manual workers; nothing dispatches them automatically here. `platform-smoke.yml` also owns the manual `native_evidence` stage (dispatch with `evidence: true`), which is the only entry point for the six-platform native evidence matrix. The two stages are mutually exclusive within a run: `native_evidence` requires `inputs.evidence`, and the smoke `worker` job requires its negation.

`browser-evidence.yml` and the `native_evidence` stage are explicit manual, credential-free evidence providers, not automatic push gates; synthetic browser data does not prove real user-profile access. Native linking and real API tests remain different evidence classes.

## Operate only the authorized action

Before rerun, cancel, dispatch, or repair, state the exact run/workflow/target and why. A changed source or documented infrastructure hypothesis may justify a rerun; repeated deterministic failure does not. Preserve the original failure and attempt identity even after recovery succeeds.

`release.yml` is tag-triggered and has no manual dispatch input. Its verified immutable artifacts precede the single `release-approval` boundary. Publisher workflows for Homebrew, Docker Hub, SkillHub, and ClawHub are not present in this fork; if a run references them, it is a stale expectation rather than a workflow to repair here. Do not invent a publisher run.

Do not move a tag, rebuild different bytes under an old release identity, bypass approval, invent a `deploy` input, or enable production secrets to rescue a check. Use [release preparation](../pixiv-cli-release-notes/SKILL.md) for release-specific work.

## Report

State the causal finding, exact run/attempt/SHA, evidence, relevant local results, and remaining checks. Distinguish API/tool access failure from workflow failure. Never call a skipped matrix, stale-head run, fixture, or pending publisher a completed release.
