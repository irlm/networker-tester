# Agent Command Verbs

This module hosts the agent-side command dispatcher introduced by the
command-based orchestration plan
(`docs/superpowers/plans/2026-04-14-command-based-orchestration-plan.md`).

## Pattern

Each verb is a submodule exposing a single async function:

```rust
pub async fn run(
    args: serde_json::Value,
    log_tx: &tokio::sync::mpsc::Sender<AgentCommandLog>,
) -> anyhow::Result<serde_json::Value>;
```

The function returns `Ok(json!(...))` on success, or `Err(...)` on any
failure; the dispatcher (`mod.rs::run_command`) maps that into the
`AgentCommandResult` envelope with the right `CommandStatus`.

Verbs that produce live output stream lines through `log_tx`. Verbs that
don't (like `health`) can ignore the channel.

## Adding a verb

1. Create `src/commands/<verb>.rs` with a `run` function matching the
   signature above.
2. Register it in `mod.rs::run_command`'s match arm.
3. Add a unit test under `#[cfg(test)] mod tests` that constructs an
   `AgentCommand`, calls `run_command`, and asserts on the result shape.

## Token validation

The dispatcher currently trusts `AgentCommand::token`. The dashboard
mints short-lived JWTs and the WebSocket auth gates the channel; a later
plan task will wire per-command token validation in before dispatch.
