# Design assessment — deployments interrupted by a control-plane restart (#764)

**Status:** assessment only. No code change lands with this note; the Retry
button (#766) is the shipped mitigation. This documents the options for a
durable fix so a future PR can pick one deliberately.

## Problem

`Provisioning/DeployRunner.cs` shells out to `install.sh` (via
`ShellInstallAsync`) as a **child process of the control plane**. When the
control plane restarts — which happens on **every deploy** of the control plane
itself, plus on host restarts and crashes — the OS SIGTERMs the process group,
so any in-flight `install.sh` dies with exit **143** (SIGTERM) or **137**
(SIGKILL). The deployment fails, and because `install.sh` may have already
created a cloud VM, that VM is briefly orphaned (the orphan sweep reaps it).

`DeployRunner.cs:233` already documents this and, since v0.28.213, emits a clear
error ("Deployment interrupted … the control plane restarted or was shut down
while deploying … retry the deployment") instead of a bare `exit 143`.

The blast radius is bounded: it only bites a deployment that is mid-flight
*during* a control-plane restart. But since the control plane restarts on its
own deploys, a deploy that ships during someone else's endpoint deployment
takes that deployment down with it.

## Why not just "fix it" here

Every real fix is architecturally significant and cannot be safely validated on
the local Docker lab (which does not exercise the prod deploy/restart cutover).
So this is scoped as an assessment; the recommendation below is deliberately the
*smallest* durable step.

## Options

### A. Detach / daemonize `install.sh` so it survives a restart

Spawn `install.sh` in its **own session/process group** (e.g. `setsid`, or
`Process` with a new process group and no kill-on-parent-exit), so a
control-plane SIGTERM no longer propagates to it. The control plane then
re-attaches to the still-running job's output/exit on restart.

- **Pros:** directly removes the SIGTERM kill; relatively small spawn change.
- **Cons:** the control plane currently reads stdout/stderr over an in-process
  pipe (`DeployOutput`) and calls `FinishAsync` from the same async flow — all
  of that is lost when the parent dies. To recover the result you need the
  detached job to write its log + exit code somewhere durable (file or DB) and a
  reconciler on startup to read it back. So "detach" is not a one-liner; it
  drags in out-of-process result capture. Also leaves a running installer with
  no live supervisor mid-restart (progress logs stall until re-attach).

### B. Durable / resumable deployment job

Persist deployment state as a proper job (status, step, cloud resource ids,
log offset) and drive `install.sh` from a **worker that reconciles on startup**:
on boot, find deployments in `running` with a live/last-known child and either
resume streaming or re-drive from the last durable checkpoint. Pair with
idempotent installer steps so a re-run doesn't double-create.

- **Pros:** the robust, correct answer — survives restart *and* crash, gives
  real progress/resume, and generalizes to other long-running provisioning.
- **Cons:** the largest change. Needs a job table + schema migration (owned
  `schema.sql`, see `docs/schema-ownership.md`), a reconciler background
  service, and idempotency work in `install.sh` itself. High blast radius; must
  be soaked in the lab + canary, not shippable as a quick fix.

### C. Readiness-gated cutover to shrink the window

Leave the child-process model, but make the control plane's own deploy
**drain**: stop accepting new deployments, wait for in-flight `install.sh`
children to finish (bounded) before the old process exits, and only then cut
over. Shrinks — does not eliminate — the kill window.

- **Pros:** smallest surface; no schema change; no installer idempotency work.
  Meaningfully cuts the failure rate for the common case (a deploy shipping
  during a short endpoint deployment).
- **Cons:** does not help host crash / SIGKILL / a deployment longer than the
  drain budget. A long `install.sh` (multi-minute) either blocks the cutover or
  still gets killed at the timeout. It's mitigation, not a cure.

## Recommendation

**Ship C now (drain-on-cutover) as the near-term reliability win, and treat B
(durable job) as the real fix** when there's appetite for the schema + reconciler
+ installer-idempotency investment. Prefer C over A: A's "detach" looks cheap but
forces most of B's out-of-process result-capture machinery anyway to recover the
outcome, for a strictly weaker guarantee (no crash resilience). The already-shipped
Retry button (#766) plus the clear #764 error message remain the backstop under
all three, and are sufficient until one of these lands.

Whichever is chosen must be validated on a path that actually restarts the
control plane mid-deploy — the local lab does not, so this needs a canary/soak
step, not just `lab.sh validate`.
