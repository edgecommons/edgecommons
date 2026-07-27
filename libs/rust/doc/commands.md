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
        commands.register("sb/status", CommandScope::Instance, command_handler(|_, _| async { Ok(None) }))
    })
    .build()
    .await?;

assert_eq!(gg.commands().unwrap().startup_status().state, CommandInboxStartupState::Active);
gg.set_ready(true);
```

## Immediate handlers

`CommandInbox::register` takes the verb, its `CommandScope`, and a `CommandHandler`. The handler
receives the request and the **addressed instance**, returns `Result<Option<Value>, CommandError>`,
and the inbox immediately produces the standard success or error wrapper. Fire-and-forget commands
run and discard their result.

```rust
use edgecommons::commands::{CommandScope, command_handler};
use serde_json::json;

commands.register("sb/status", CommandScope::Instance, command_handler(|_request, addressed_instance| async move {
    // addressed_instance: Option<String> - Some("press12"), or None when nothing named an instance.
    Ok(Some(json!({ "instance": addressed_instance, "online": true })))
}))?;
```

Both registration forms share one verb namespace. Built-in verbs (`ping`, `describe`,
`reload-config`, `get-configuration`, `status`) and delegated verbs (`set-config`) cannot be
shadowed, a verb may be registered once, and `unregister` clears the handler, the declared scope,
and any stored availability.

## Declared verb scope

Every verb declares a `CommandScope`, and the inbox enforces the addressing **before dispatch** —
the handler never runs on an addressing error. The `addressed_instance` a handler receives is the
delivery topic's `{instance}` token (`ecv1/{device}/{component}/{instance}/cmd/{verb}`), else the
request body's `instance` field, else `None`.

| Scope | Instance-addressed delivery | Component-addressed delivery |
|---|---|---|
| `CommandScope::Component` | `BAD_ARGS`, `"verb '<verb>' is component-scoped"` | the handler runs with `None` |
| `CommandScope::Instance` | the handler runs with the topic's token | the handler runs with the body's `instance`, else `None` |
| `CommandScope::Both` | the handler runs with the topic's token | the handler runs with the body's `instance`, else `None` — "the whole component" |

A body `instance` that disagrees with the topic's token is `BAD_ARGS`
(`"instance in body conflicts with the addressed instance"`) at every scope, checked first. A
`Component` verb also refuses a body `instance`. A rejected fire-and-forget delivery is logged
rather than replied to, exactly like a handler error.

The library owns *addressing*. Resolving `None` against the component's configuration — the
convention that `instance` is optional when exactly one is configured, and that an unrecognized
name is `NO_SUCH_INSTANCE` — needs configuration knowledge the library does not have and belongs
to the component.

`Both` serves two purposes: scope-indifferent verbs (the built-ins answer identically either way)
and dual-semantics verbs whose handler branches on `None` for component-wide behavior. Widening a
verb from `Instance` to `Both` is additive; narrowing changes that verb's contract.

## Command availability

`set_command_availability(verb, state, reason)` sets a registered verb's availability as surfaced
by `describe`. `state` must be exactly `available`, `disabled`, or `unsupported`; any other token
is an error, as is an unregistered verb. `available` removes the stored entry, so the verb's
describe entry reverts to `{verb, builtIn, scope}`; `disabled`/`unsupported` store
`{"state": …, "reason"?: …}` and the verb's entry becomes `{verb, builtIn, scope, availability}`.
The `reason` is optional, trimmed, truncated to 256 characters, and omitted when empty.
Availability is orthogonal to scope — a verb can be `instance`-scoped and `disabled`. The describe
digest tracks both the stored availability and the declared scope, so a console re-fetches the
descriptor on every transition.

```rust
commands.set_command_availability("sb/write", "disabled", Some("writes.allow[] is empty"))?;
commands.set_command_availability("sb/write", "available", None)?; // back to {verb, builtIn, scope}
```

## Panel manifest default view

`describe`'s panel manifest sets `defaultView` to the `id` of the first registered view whose
`default` property is boolean `true`, falling back to the first view's `id`. Views are emitted
verbatim — the `default` key is not stripped.

## Explicit outcomes

Long-running handlers use the `OutcomeCommandHandler` surface through `outcome_handler` and
`register_outcome`, which takes the same `CommandScope` and delivers the same `addressed_instance`
as the immediate form. They return one of:

- `CommandOutcome::ImmediateSuccess(result)`;
- `CommandOutcome::ImmediateError(CommandError)`; or
- `CommandOutcome::Deferred(token)`; or
- `CommandOutcome::deferred_with_continuation(token, continuation)`.

Immediate and outcome handlers share one verb namespace, one set of built-in/delegated/no-shadowing
rules, and one scope-enforcement path.

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
use edgecommons::commands::{CommandError, CommandOutcome, CommandScope, outcome_handler};
use serde_json::json;
use std::time::Duration;

commands.register_outcome("sb/capture", CommandScope::Instance, outcome_handler(|request, deferred, addressed_instance| async move {
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
            .settle_success(Some(json!({
                "captureId": "cap-1",
                "instance": addressed_instance,
                "state": "SUCCEEDED"
            })))
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
