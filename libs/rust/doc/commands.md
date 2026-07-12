# Commands and deferred replies

`CommandInbox` owns the component's `main/cmd/#` subscription, built-in verbs, handler dispatch, guarded
reply construction, and deferred-reply registry. Obtain it with `gg.commands()`.

## Startup lifecycle and registration ordering

Install application verbs with `EdgeCommonsBuilder::configure_commands`; the builder invokes every
configurer before subscription starts. `CommandInbox` reports `Starting`, `Active`, `Failed`, or
`Stopped`. `Active` requires a successful MQTT SUBACK or Greengrass subscription-operation completion,
not local enqueueing. Startup failure retains a bounded sanitized error and cleans partial subscription
state; `stop().await` followed by `start().await` is retryable.

Deliveries racing acknowledgement are retained in arrival order behind a strict 256-message activation
gate. The 257th and later delivery is dropped newest. Runtime readiness requires both the application's
`initial_ready`/`set_ready` gate and an `Active` command plane.

```rust
let gg = EdgeCommonsBuilder::new("com.example.Camera")
    .initial_ready(false)
    .configure_commands(|commands| {
        commands.register("sb/status", command_handler(|_| async { Ok(None) }))
    })
    .build()
    .await?;

assert_eq!(gg.commands().unwrap().startup_status().state, CommandInboxStartupState::Active);
gg.set_ready(true);
```

## Legacy handlers remain unchanged

Existing `CommandHandler`, `command_handler`, and `CommandInbox::register` behavior is unchanged. A legacy
handler returns `Result<Option<Value>, CommandError>` and the inbox immediately produces the standard
success or error wrapper. Fire-and-forget commands still run and discard their result.

```rust
use edgecommons::commands::command_handler;
use serde_json::json;

commands.register("sb/status", command_handler(|_request| async move {
    Ok(Some(json!({ "online": true })))
}))?;
```

## Explicit outcomes

Long-running handlers use the parallel `OutcomeCommandHandler` surface through `outcome_handler` and
`register_outcome`. They return one of:

- `CommandOutcome::ImmediateSuccess(result)`;
- `CommandOutcome::ImmediateError(CommandError)`; or
- `CommandOutcome::Deferred(token)`; or
- `CommandOutcome::deferred_with_continuation(token, continuation)`.

Legacy and outcome handlers share one verb namespace. Built-in/delegated/no-shadowing rules apply to both.

## Deferred lifecycle

The inbox owns a registry with a hard capacity of 1,024 entries. A handler receives a cloneable registry
handle but never receives an unguarded reply publisher. The required acceptance sequence is:

1. call `deferred.defer(&request, lifetime)` to create a `PROVISIONAL` opaque token;
2. durably commit the application job or operation;
3. call `token.activate()` to make it `OPEN`;
4. return `CommandOutcome::Deferred(token)`, or return the post-accept continuation form below;
   and
5. later call `settle_success`, `settle_error`, or `settle_command_error`.

If durable acceptance fails, call `token.discard()` while it is still provisional and return an immediate
error. `defer` rejects a missing/empty `reply_to` with `REPLY_REQUIRED`; fire-and-forget work cannot promise
a later direct reply.

```rust
use edgecommons::commands::{CommandError, CommandOutcome, outcome_handler};
use serde_json::json;
use std::time::Duration;

commands.register_outcome("sb/capture", outcome_handler(|request, deferred| async move {
    let token = match deferred.defer(&request, Duration::from_secs(95)) {
        Ok(token) => token,
        Err(error) => return CommandOutcome::ImmediateError(error),
    };

    // Insert and commit the durable job here. On failure:
    // let _ = token.discard();
    // return CommandOutcome::ImmediateError(CommandError::new("PERSISTENCE_FAILED", "..."));

    if let Err(error) = token.activate() {
        return CommandOutcome::ImmediateError(CommandError::handler_error(error));
    }

    let completion = token.clone();
    CommandOutcome::deferred_with_continuation(token, async move {
        let _ = completion
            .settle_success(Some(json!({ "captureId": "cap-1", "state": "SUCCEEDED" })))
            .await;
        Ok(())
    })
}))?;
```

Returning `Deferred` suppresses the automatic reply and releases the ordinary command-dispatch permit.
The token is validated against the exact request UUID, verb, correlation id, and guarded `reply_to` before
the dispatcher accepts it. `deferred_with_continuation` is the race-free handoff for asynchronous
application work: the inbox validates an `OPEN` token for the exact delivery, then starts its bounded
continuation. The continuation is never invoked for a provisional, foreign, expired, or otherwise invalid
token. At most 256 post-accept continuations may be in flight; rejection settles the accepted token through
the standard guarded error path. A continuation returns `Result<(), CommandError>`; an `Err` is settled
through the same guarded reply path.

## Settlement guarantees

- `OPEN -> SETTLING` is an atomic compare-and-set. Cloned tokens can race, but at most one caller settles.
- The registry builds one responder-identity-stamped command reply and uses strict confirmed reply.
- Transient publish/confirmation errors retry with bounded exponential backoff until token expiry.
- The retained request is never exposed as a raw publish capability; the messaging reply guard validates
  its `reply_to` on every attempt.
- A timer expires provisional, open, or settling tokens at their explicit lifetime and logs a stable
  diagnostic for an open/settling expiry.
- `shutdown_deferred().await` stops new token creation, attempts a confirmed `COMPONENT_STOPPING` reply for
  each open token, and transitions it to `CANCELLED_ON_SHUTDOWN`. `CommandInbox` drop schedules the same
  bounded cleanup as a fallback.
- Provider enqueue success is never treated as settlement confirmation. MQTT requires the matching PUBACK;
  Greengrass requires successful IPC operation completion.

Deferred reply paths are intentionally ephemeral across process restart. Durable job status and terminal
application messages are the recovery contract.
