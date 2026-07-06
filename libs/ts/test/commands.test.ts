/**
 * Deterministic unit tests for {@link CommandInbox} (DESIGN-uns §9.5, the minimal `commands()`
 * facade — edge-console slice S2) over the {@link RecordingMessagingService} fake. Mirrors the
 * Java `CommandInboxTest`, adapted to the TS async handler/action seams:
 *
 * - `start()` subscribes exactly the own-inbox wildcard (`ecv1/{device}/{component}/main/cmd/#`)
 *   on the primary connection;
 * - each built-in verb dispatches and replies with the pinned body shape — `ping`
 *   (status + uptime), `reload-config` (ack / `RELOAD_FAILED`), `get-configuration`
 *   (redacted config / `NO_CONFIG`);
 * - replies go to the request's `reply_to` with the request's `correlation_id` and the
 *   responder's identity;
 * - custom verbs register/dispatch (namespaced verbs included), cannot shadow built-ins or each
 *   other, and unregister; coded (`CommandException`) vs uncoded (`HANDLER_ERROR`) failures;
 * - unknown verbs get an `UNKNOWN_VERB` error reply (requests) or are ignored
 *   (fire-and-forget); no-`reply_to` commands run the handler without a reply;
 * - malformed payloads (name mismatch, headerless, null) and the delegated `set-config` verb
 *   are ignored — never replied to, never a crash;
 * - `close()` unsubscribes the inbox and stops dispatch; lifecycle is idempotent; a subscribe
 *   failure disables the inbox (best-effort start, never throws).
 *
 * There is no TS mirror of the Java `missingIdentityDisablesTheInbox` test: the TS `Config`
 * model resolves `componentIdentity` eagerly and fails fast at construction, so a `Config`
 * snapshot always carries a resolved identity (the same divergence already documented on
 * `RepublishListener` / `republish_listener.test.ts`).
 */
import { describe, it, expect, beforeEach } from "vitest";

import { Config } from "../src/config/model";
import { CommandInbox, CommandException } from "../src/commands";
import { Message, MessageBuilder } from "../src/message";
import { UnsValidationError } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

/** The default test identity: device `test-thing`, component `TestComponent` (single level). */
const INBOX_FILTER = "ecv1/test-thing/TestComponent/main/cmd/#";
const REPLY_TO = "edgecommons/reply-test-1";

const config = (): Config => Config.fromValue("com.example.TestComponent", "test-thing", {});

function topic(verb: string): string {
  return `ecv1/test-thing/TestComponent/main/cmd/${verb}`;
}

/** A well-formed request for a verb: `header.name` = verb, pinned `reply_to`. */
function request(verb: string): Message {
  return MessageBuilder.create(verb, "1.0").withPayload({}).withReplyTo(REPLY_TO).build();
}

/** A well-formed fire-and-forget command (no `reply_to`). */
function notification(verb: string): Message {
  return MessageBuilder.create(verb, "1.0").withPayload({}).build();
}

/** Fetches the inbox's registered handler (the single own-inbox wildcard subscription). */
function handlerFor(svc: RecordingMessagingService): (topic: string, message: Message) => void | Promise<void> {
  const h = svc.subscriptions.get(INBOX_FILTER);
  if (!h) throw new Error(`no handler registered for '${INBOX_FILTER}'`);
  return h;
}

/** Delivers one message to the inbox's handler and awaits dispatch completion. */
async function deliver(svc: RecordingMessagingService, deliveredTopic: string, message: Message | null): Promise<void> {
  await handlerFor(svc)(deliveredTopic, message as unknown as Message);
}

/** The single recorded reply (topic must be the request's `reply_to`). */
function onlyReplyBody(svc: RecordingMessagingService): Record<string, unknown> {
  expect(svc.published, "exactly one reply expected").toHaveLength(1);
  const rec = svc.published[0];
  expect(rec.topic, "the reply must go to the request's reply_to").toBe(REPLY_TO);
  return rec.message!.getBody() as Record<string, unknown>;
}

describe("CommandInbox", () => {
  let messaging: RecordingMessagingService;
  let uptime: number;
  let reloadResult: boolean;
  let redactedConfig: Record<string, unknown> | undefined;
  let inbox: CommandInbox;

  beforeEach(() => {
    messaging = new RecordingMessagingService();
    uptime = 42;
    reloadResult = true;
    redactedConfig = { component: { global: { v: 1 } } };
    inbox = new CommandInbox(
      config,
      messaging,
      () => uptime,
      () => reloadResult,
      () => redactedConfig,
    );
  });

  // ===================== subscription lifecycle =====================

  it("start() subscribes the own-inbox wildcard", async () => {
    await inbox.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([INBOX_FILTER]));
  });

  it("start() is idempotent", async () => {
    await inbox.start();
    await inbox.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([INBOX_FILTER]));
  });

  it("close() unsubscribes and stops dispatch", async () => {
    await inbox.start();
    const handler = handlerFor(messaging); // captured before close() removes the subscription
    await inbox.close();
    expect(messaging.subscriptions.has(INBOX_FILTER)).toBe(false);
    // A late (stray) delivery after close is ignored (the closed flag guards dispatch).
    await handler(topic(CommandInbox.PING), request(CommandInbox.PING));
    expect(messaging.published).toHaveLength(0);
  });

  it("close() is idempotent and start() after close() is a no-op", async () => {
    await inbox.start();
    await inbox.close();
    await expect(inbox.close()).resolves.toBeUndefined();
    await inbox.start(); // closed -> must not resubscribe
    expect(messaging.subscriptions.size).toBe(0);
  });

  it("a subscribe failure disables the inbox (best-effort start, never throws)", async () => {
    messaging.subscribe = async () => {
      throw new Error("broker unavailable");
    };
    await expect(inbox.start()).resolves.toBeUndefined();
    expect(messaging.subscriptions.size).toBe(0);
    await expect(inbox.close()).resolves.toBeUndefined();
  });

  // ===================== built-in verbs =====================

  it("ping replies status and uptime", async () => {
    uptime = 1234;
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.PING), request(CommandInbox.PING));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    const result = body.result as Record<string, unknown>;
    expect(result.status).toBe("RUNNING");
    expect(result.uptimeSecs).toBe(1234);
  });

  it("reply carries the request's correlation_id, verb name, and responder identity", async () => {
    await inbox.start();
    const ping = request(CommandInbox.PING);
    await deliver(messaging, topic(CommandInbox.PING), ping);
    expect(messaging.published).toHaveLength(1);
    const rec = messaging.published[0];
    expect(rec.message!.getCorrelationId(), "the reply must carry the request's correlation_id").toBe(
      ping.getCorrelationId(),
    );
    expect(rec.message!.header.name, "the reply header.name is the verb").toBe(CommandInbox.PING);
    expect(rec.message!.header.version).toBe(CommandInbox.CMD_MESSAGE_VERSION);
    expect(rec.message!.getIdentity(), "the reply is config-stamped with the responder's identity").toBeDefined();
  });

  it("reload-config replies ack on success", async () => {
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.RELOAD_CONFIG), request(CommandInbox.RELOAD_CONFIG));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    expect((body.result as Record<string, unknown>).reloaded).toBe(true);
  });

  it("reload-config replies RELOAD_FAILED on failure", async () => {
    reloadResult = false;
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.RELOAD_CONFIG), request(CommandInbox.RELOAD_CONFIG));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    const error = body.error as Record<string, unknown>;
    expect(error.code).toBe(CommandInbox.ERR_RELOAD_FAILED);
    expect(error.message).not.toBe("");
  });

  it("reload-config supports an async (Promise-returning) reload action", async () => {
    const asyncInbox = new CommandInbox(config, messaging, () => uptime, async () => true, () => redactedConfig);
    await asyncInbox.start();
    await deliver(messaging, topic(CommandInbox.RELOAD_CONFIG), request(CommandInbox.RELOAD_CONFIG));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
  });

  it("get-configuration replies the redacted effective config", async () => {
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.GET_CONFIGURATION), request(CommandInbox.GET_CONFIGURATION));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    expect((body.result as Record<string, unknown>).config, "get-configuration must return the redacted effective config (Flow B)").toEqual(
      redactedConfig,
    );
  });

  it("get-configuration replies NO_CONFIG when unavailable", async () => {
    redactedConfig = undefined;
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.GET_CONFIGURATION), request(CommandInbox.GET_CONFIGURATION));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    expect((body.error as Record<string, unknown>).code).toBe(CommandInbox.ERR_NO_CONFIG);
  });

  // ===================== custom verbs (the registration seam) =====================

  it("a custom verb registers and dispatches", async () => {
    await inbox.start(); // registration after start needs no new subscription
    inbox.register("restart-pipeline", () => ({ restarted: true }));
    await deliver(messaging, topic("restart-pipeline"), request("restart-pipeline"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    expect((body.result as Record<string, unknown>).restarted).toBe(true);
  });

  it("a namespaced custom verb dispatches", async () => {
    inbox.register("sb/status", () => null); // null result -> empty ack
    await inbox.start();
    await deliver(messaging, topic("sb/status"), request("sb/status"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    expect(body.result, "a null handler result must reply an empty result object").toEqual({});
  });

  it("a handler's CommandException keeps its code", async () => {
    inbox.register("guarded", () => {
      throw new CommandException("NOT_ALLOWED", "operator role required");
    });
    await inbox.start();
    await deliver(messaging, topic("guarded"), request("guarded"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    const error = body.error as Record<string, unknown>;
    expect(error.code).toBe("NOT_ALLOWED");
    expect(error.message).toBe("operator role required");
  });

  it("a handler's uncoded exception maps to HANDLER_ERROR", async () => {
    inbox.register("boomy", () => {
      throw new Error("boom");
    });
    await inbox.start();
    await deliver(messaging, topic("boomy"), request("boomy"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    expect((body.error as Record<string, unknown>).code).toBe(CommandInbox.ERR_HANDLER_ERROR);
  });

  it("an async handler's rejected promise maps to HANDLER_ERROR", async () => {
    inbox.register("async-boomy", async () => {
      throw new Error("async boom");
    });
    await inbox.start();
    await deliver(messaging, topic("async-boomy"), request("async-boomy"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    expect((body.error as Record<string, unknown>).code).toBe(CommandInbox.ERR_HANDLER_ERROR);
  });

  it("register rejects shadowing and invalid verbs", () => {
    expect(() => inbox.register(CommandInbox.PING, () => null), "a built-in verb cannot be shadowed").toThrow(
      /built-in/,
    );
    expect(
      () => inbox.register(CommandInbox.SET_CONFIG_VERB, () => null),
      "a delegated verb cannot be registered",
    ).toThrow(/owned by another/);
    inbox.register("mine", () => null);
    expect(() => inbox.register("mine", () => null), "an already-registered verb cannot be re-registered").toThrow(
      /already registered/,
    );
    expect(() => inbox.register("bad+verb", () => null), "verb tokens must pass the topic token rule").toThrow(
      UnsValidationError,
    );
    expect(() => inbox.register("sb//x", () => null), "empty namespace tokens are rejected").toThrow(
      UnsValidationError,
    );
  });

  it("unregister removes custom verbs but never built-ins", async () => {
    inbox.register("mine", () => null);
    expect(inbox.verbs().has("mine")).toBe(true);
    inbox.unregister("mine");
    expect(inbox.verbs().has("mine")).toBe(false);
    expect(() => inbox.unregister("mine")).not.toThrow(); // unknown -> no-op
    expect(() => inbox.unregister(CommandInbox.RELOAD_CONFIG)).toThrow(/built-in/);
    // The unregistered verb now gets the unknown-verb error.
    await inbox.start();
    await deliver(messaging, topic("mine"), request("mine"));
    expect((onlyReplyBody(messaging).error as Record<string, unknown>).code).toBe(CommandInbox.ERR_UNKNOWN_VERB);
  });

  it("verbs() snapshot contains built-ins and customs", () => {
    inbox.register("mine", () => null);
    expect(inbox.verbs()).toEqual(
      new Set([CommandInbox.PING, CommandInbox.RELOAD_CONFIG, CommandInbox.GET_CONFIGURATION, "mine"]),
    );
  });

  // ===================== unknown / fire-and-forget / malformed =====================

  it("an unknown verb request gets an UNKNOWN_VERB error reply", async () => {
    await inbox.start();
    await deliver(messaging, topic("no-such-verb"), request("no-such-verb"));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(false);
    expect((body.error as Record<string, unknown>).code).toBe(CommandInbox.ERR_UNKNOWN_VERB);
  });

  it("an unknown fire-and-forget verb is ignored", async () => {
    await inbox.start();
    await deliver(messaging, topic("no-such-verb"), notification("no-such-verb"));
    expect(messaging.published, "an unknown fire-and-forget verb must not be replied to").toHaveLength(0);
  });

  it("no reply_to runs the handler without replying", async () => {
    let ran = false;
    inbox.register("do-it", () => {
      ran = true;
      return null;
    });
    await inbox.start();
    await deliver(messaging, topic("do-it"), notification("do-it"));
    expect(ran, "a fire-and-forget command must still run the handler").toBe(true);
    expect(messaging.published, "...but never reply").toHaveLength(0);
  });

  it("a fire-and-forget handler failure is logged only", async () => {
    inbox.register("do-it", () => {
      throw new CommandException("NOPE", "nope");
    });
    await inbox.start();
    await expect(deliver(messaging, topic("do-it"), notification("do-it"))).resolves.toBeUndefined();
    expect(messaging.published).toHaveLength(0);
  });

  it("malformed payloads are ignored without reply and never crash", async () => {
    await inbox.start();
    // header.name does not equal the topic verb (foreign convention on a cmd topic).
    await deliver(messaging, topic(CommandInbox.PING), request("something-else"));
    // A raw (headerless) envelope - junk JSON on the inbox.
    await deliver(messaging, topic(CommandInbox.PING), Message.fromObject({}));
    // A null message must not crash the callback either.
    await expect(deliver(messaging, topic(CommandInbox.PING), null)).resolves.toBeUndefined();
    expect(messaging.published, "malformed/foreign payloads must never be replied to").toHaveLength(0);
  });

  it("the delegated set-config verb is ignored even as a request", async () => {
    await inbox.start();
    await deliver(messaging, topic(CommandInbox.SET_CONFIG_VERB), request(CommandInbox.SET_CONFIG_VERB));
    expect(
      messaging.published,
      "set-config is owned by the CONFIG_COMPONENT subscription - never dispatched or replied to here",
    ).toHaveLength(0);
  });

  it("a bare cmd parent-level delivery is ignored", async () => {
    await inbox.start();
    // MQTT "#" also matches the parent level (".../cmd") - nothing to dispatch there.
    await deliver(messaging, "ecv1/test-thing/TestComponent/main/cmd", request(CommandInbox.PING));
    expect(messaging.published).toHaveLength(0);
  });

  it("a failing reply publish is swallowed", async () => {
    const failing = new RecordingMessagingService();
    failing.reply = async () => {
      throw new Error("broker down");
    };
    const failingInbox = new CommandInbox(config, failing, () => uptime, () => reloadResult, () => redactedConfig);
    await failingInbox.start();
    await expect(
      deliver(failing, topic(CommandInbox.PING), request(CommandInbox.PING)),
    ).resolves.toBeUndefined();
    await failingInbox.close();
  });

  it("an unsubscribe failure during close() is swallowed", async () => {
    await inbox.start();
    messaging.unsubscribe = async () => {
      throw new Error("already disconnected");
    };
    await expect(inbox.close()).resolves.toBeUndefined();
  });

  it("a delivery landing exactly on the inbox prefix (empty verb) is ignored", async () => {
    await inbox.start();
    // ".../cmd/#" also matches the bare ".../cmd/" prefix itself - an empty verb, nothing to
    // dispatch (distinct from the parent-level ".../cmd" case, which fails the prefix check).
    await deliver(messaging, "ecv1/test-thing/TestComponent/main/cmd/", request(CommandInbox.PING));
    expect(messaging.published).toHaveLength(0);
  });

  it("an exception while extracting/validating the verb is caught and logged (never crashes)", async () => {
    await inbox.start();
    const throwsOnHeaderAccess = {
      get header(): never {
        throw new Error("boom");
      },
    } as unknown as Message;
    await expect(deliver(messaging, topic(CommandInbox.PING), throwsOnHeaderAccess)).resolves.toBeUndefined();
    expect(messaging.published).toHaveLength(0);
  });

  it("a fire-and-forget handler's uncoded exception is logged only (never replied)", async () => {
    inbox.register("boomy", () => {
      throw new Error("boom");
    });
    await inbox.start();
    await expect(deliver(messaging, topic("boomy"), notification("boomy"))).resolves.toBeUndefined();
    expect(messaging.published).toHaveLength(0);
  });

  it("CommandException rejects an empty code", () => {
    expect(() => new CommandException("", "message")).toThrow(/code must not be empty/);
  });
});
