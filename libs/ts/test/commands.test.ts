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
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

import { Config } from "../src/config/model";
import {
  CommandInbox,
  CommandInboxState,
  CommandException,
  CommandOutcomes,
  DeferredReply,
  DeferredReplyState,
  SettlementResult,
} from "../src/commands";
import { Message, MessageBuilder } from "../src/message";
import { UnsValidationError } from "../src/uns";
import { RecordingMessagingService, tick } from "./_fakes";

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

async function waitFor(predicate: () => boolean, timeoutMs = 1_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error("condition was not satisfied before timeout");
    await tick(5);
  }
}

function expectCommandExceptionCode(action: () => unknown, code: string): void {
  try {
    action();
    throw new Error(`expected CommandException(${code})`);
  } catch (error) {
    expect(error).toBeInstanceOf(CommandException);
    expect((error as CommandException).code).toBe(code);
  }
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
    expect(inbox.state()).toBe(CommandInboxState.Stopped);
    await inbox.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([INBOX_FILTER]));
    expect(inbox.state()).toBe(CommandInboxState.Active);
  });

  it("reports a stable FAILED state and tears down the exact filter after a subscription failure", async () => {
    messaging.subscribe = async () => {
      throw new Error("broker rejected subscription with sensitive detail");
    };
    await inbox.start();
    expect(inbox.state()).toBe(CommandInboxState.Failed);
    expect(inbox.startupError()).toBe("COMMAND_INBOX_SUBSCRIPTION_FAILED");
    expect(messaging.unsubscribed).toEqual([INBOX_FILTER]);
    await inbox.close();
    expect(inbox.state()).toBe(CommandInboxState.Stopped);
    expect(inbox.startupError()).toBeUndefined();
  });

  it("notifies lifecycle observers in the STARTING, ACTIVE, STOPPED order", async () => {
    const states: CommandInboxState[] = [];
    const observed = new CommandInbox(
      config,
      messaging,
      () => uptime,
      () => reloadResult,
      () => redactedConfig,
      (state) => states.push(state),
    );
    await observed.start();
    await observed.close();
    expect(states).toEqual([CommandInboxState.Starting, CommandInboxState.Active, CommandInboxState.Stopped]);
  });

  it("start() is idempotent", async () => {
    await inbox.start();
    await inbox.start();
    expect(new Set(messaging.subscriptions.keys())).toEqual(new Set([INBOX_FILTER]));
  });

  it("shares one bounded startup attempt across concurrent callers", async () => {
    let releaseSubscription!: () => void;
    let subscribeCalls = 0;
    messaging.subscribe = async (filter, handler) => {
      subscribeCalls++;
      messaging.subscriptions.set(filter, handler);
      await new Promise<void>((resolve) => {
        releaseSubscription = resolve;
      });
    };

    const first = inbox.start(100);
    const second = inbox.start(100);
    await Promise.resolve();
    expect(subscribeCalls).toBe(1);
    expect(inbox.state()).toBe(CommandInboxState.Starting);

    releaseSubscription();
    await Promise.all([first, second]);
    expect(inbox.state()).toBe(CommandInboxState.Active);
    expect(subscribeCalls).toBe(1);
  });

  it("retains a delivery racing subscription acknowledgement until ACTIVE", async () => {
    let releaseSubscription!: () => void;
    let handlerRan = false;
    inbox.register("startup-race", () => {
      handlerRan = true;
      return {};
    });
    messaging.subscribe = async (filter, handler) => {
      messaging.subscriptions.set(filter, handler);
      await new Promise<void>((resolve) => {
        releaseSubscription = resolve;
      });
    };

    const starting = inbox.start(100);
    await Promise.resolve();
    await deliver(messaging, topic("startup-race"), notification("startup-race"));
    expect(inbox.state()).toBe(CommandInboxState.Starting);
    expect(handlerRan).toBe(false);

    releaseSubscription();
    await starting;
    expect(inbox.state()).toBe(CommandInboxState.Active);
    await waitFor(() => handlerRan);
  });

  it("times out with a stable error and removes a late successful subscription", async () => {
    let releaseSubscription!: () => void;
    let lateHandler!: (deliveredTopic: string, message: Message) => void | Promise<void>;
    messaging.subscribe = (filter, handler) => new Promise<void>((resolve) => {
      lateHandler = handler;
      releaseSubscription = () => {
        messaging.subscriptions.set(filter, handler);
        resolve();
      };
    });

    await inbox.start(10);
    expect(inbox.state()).toBe(CommandInboxState.Failed);
    expect(inbox.startupError()).toBe("COMMAND_INBOX_SUBSCRIPTION_FAILED");
    expect(messaging.unsubscribed).toContain(INBOX_FILTER);

    releaseSubscription();
    await waitFor(() => messaging.unsubscribed.filter((filter) => filter === INBOX_FILTER).length >= 2);
    expect(inbox.state()).toBe(CommandInboxState.Failed);
    await lateHandler(topic(CommandInbox.PING), request(CommandInbox.PING));
    expect(messaging.published).toHaveLength(0);
    expect(messaging.subscriptions.has(INBOX_FILTER)).toBe(false);
  });

  it("close racing startup remains STOPPED after a late acknowledgement", async () => {
    let releaseSubscription!: () => void;
    const states: CommandInboxState[] = [];
    const observed = new CommandInbox(
      config,
      messaging,
      () => uptime,
      () => reloadResult,
      () => redactedConfig,
      (state) => states.push(state),
    );
    messaging.subscribe = async (filter, handler) => {
      messaging.subscriptions.set(filter, handler);
      await new Promise<void>((resolve) => {
        releaseSubscription = resolve;
      });
    };

    const starting = observed.start(100);
    await Promise.resolve();
    await observed.close();
    expect(observed.state()).toBe(CommandInboxState.Stopped);

    releaseSubscription();
    await starting;
    expect(observed.state()).toBe(CommandInboxState.Stopped);
    expect(states).toEqual([CommandInboxState.Starting, CommandInboxState.Stopped]);
    expect(messaging.subscriptions.has(INBOX_FILTER)).toBe(false);
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

  it("describe includes component identity, built-ins, custom verbs, panels, and a stable digest", async () => {
    inbox.register("sb/browse", () => ({ nodes: [] }));
    const overview = {
      id: "overview",
      title: "Overview",
      order: 10,
      scope: "component",
      widgets: [{ kind: "summary", id: "summary", title: "Summary", rows: [{ label: "Endpoint", value: "opc.tcp" }] }],
    };
    inbox.registerPanel(overview);

    await inbox.start();
    await deliver(messaging, topic(CommandInbox.DESCRIBE), request(CommandInbox.DESCRIBE));
    const body = onlyReplyBody(messaging);
    expect(body.ok).toBe(true);
    const result = body.result as Record<string, unknown>;
    expect(result.schemaVersion).toBe("edgecommons.component.describe.v1");
    expect(result.digest).toMatch(/^sha256:[0-9a-f]{64}$/);
    expect(result.component).toEqual({
      hier: [{ level: "device", value: "test-thing" }],
      path: "test-thing",
      component: "TestComponent",
      instance: "main",
    });
    expect(result.commands).toEqual([
      { verb: CommandInbox.DESCRIBE, builtIn: true },
      { verb: CommandInbox.GET_CONFIGURATION, builtIn: true },
      { verb: CommandInbox.PING, builtIn: true },
      { verb: CommandInbox.RELOAD_CONFIG, builtIn: true },
      { verb: "sb/browse", builtIn: false },
    ]);
    expect(result.panels).toEqual({
      schemaVersion: "edgecommons.panels.v2",
      provider: "TestComponent",
      renderer: "descriptor",
      defaultView: "overview",
      views: [overview],
    });

    messaging.published.length = 0;
    await deliver(messaging, topic(CommandInbox.DESCRIBE), request(CommandInbox.DESCRIBE));
    const secondBody = onlyReplyBody(messaging);
    expect((secondBody.result as Record<string, unknown>).digest).toBe(result.digest);
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
    expect(() => inbox.register(CommandInbox.DESCRIBE, () => null), "describe cannot be shadowed").toThrow(
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
    expect(() => inbox.unregister(CommandInbox.DESCRIBE)).toThrow(/built-in/);
    // The unregistered verb now gets the unknown-verb error.
    await inbox.start();
    await deliver(messaging, topic("mine"), request("mine"));
    expect((onlyReplyBody(messaging).error as Record<string, unknown>).code).toBe(CommandInbox.ERR_UNKNOWN_VERB);
  });

  it("verbs() snapshot contains built-ins and customs", () => {
    inbox.register("mine", () => null);
    expect(inbox.verbs()).toEqual(
      new Set([
        CommandInbox.PING,
        CommandInbox.DESCRIBE,
        CommandInbox.GET_CONFIGURATION,
        CommandInbox.RELOAD_CONFIG,
        "mine",
      ]),
    );
  });

  afterEach(async () => {
    await inbox.close();
  });

  it("registerPanel rejects invalid panels and duplicate ids", () => {
    expect(() => inbox.registerPanel(null as unknown as Record<string, unknown>)).toThrow(/JSON object/);
    expect(() => inbox.registerPanel([] as unknown as Record<string, unknown>)).toThrow(/JSON object/);
    expect(() => inbox.registerPanel({ title: "Overview" })).toThrow(/id/);
    expect(() => inbox.registerPanel({ id: "", title: "Overview" })).toThrow(/id/);
    expect(() => inbox.registerPanel({ id: "overview" })).toThrow(/title/);
    expect(() => inbox.registerPanel({ id: "overview", title: "" })).toThrow(/title/);

    inbox.registerPanel({ id: "overview", title: "Overview" });
    expect(inbox.panels()).toEqual([{ id: "overview", title: "Overview" }]);
    expect(() => inbox.registerPanel({ id: "overview", title: "Other" })).toThrow(/already registered/);
  });

  // ===================== explicit outcomes + deferred replies =====================

  it("explicit immediate outcomes preserve the standard success and coded-error bodies", async () => {
    inbox.registerOutcome("accepted", () => CommandOutcomes.success({ durable: true }));
    inbox.registerOutcome("rejected", () => CommandOutcomes.error("NOT_ACCEPTED", "queue is draining"));
    await inbox.start();

    await deliver(messaging, topic("accepted"), request("accepted"));
    expect(onlyReplyBody(messaging)).toEqual({ ok: true, result: { durable: true } });

    messaging.published.length = 0;
    await deliver(messaging, topic("rejected"), request("rejected"));
    expect(onlyReplyBody(messaging)).toEqual({
      ok: false,
      error: { code: "NOT_ACCEPTED", message: "queue is draining" },
    });
  });

  it("legacy and explicit-outcome registrations share one no-shadowing namespace", () => {
    inbox.registerOutcome("job", () => CommandOutcomes.success());
    expect(() => inbox.register("job", () => null)).toThrow(/already registered/);
    expect(inbox.verbs()).toContain("job");
    inbox.unregister("job");
    expect(inbox.verbs()).not.toContain("job");
    inbox.register("job", () => null);
    expect(() => inbox.registerOutcome("job", () => CommandOutcomes.success())).toThrow(/already registered/);
  });

  it("provisions before durable commit, activates after commit, and confirms exactly one reply", async () => {
    let token: DeferredReply | undefined;
    inbox.registerOutcome("capture", (req) => {
      token = inbox.defer(req, 2_000);
      expect(token.state()).toBe(DeferredReplyState.Provisional);
      // This line represents the application durable-acceptance commit boundary.
      expect(token.activate()).toBe(true);
      return CommandOutcomes.deferred(token);
    });
    await inbox.start();
    const original = request("capture");
    await deliver(messaging, topic("capture"), original);

    expect(messaging.published).toHaveLength(0);
    expect(token?.state()).toBe(DeferredReplyState.Open);
    expect(inbox.deferredReplySnapshot().active).toBe(1);
    expect(token?.settleSuccess({ imageId: "frame-7" })).toBe(SettlementResult.Accepted);
    expect(token?.state()).toBe(DeferredReplyState.Settling);
    await waitFor(() => token?.state() === DeferredReplyState.Settled);

    expect(token?.settleError("TOO_LATE", "second settler")).toBe(SettlementResult.AlreadySettled);
    expect(messaging.published).toHaveLength(1);
    const confirmed = messaging.published[0];
    expect(confirmed.kind).toBe("replyConfirmed");
    expect(confirmed.topic).toBe(REPLY_TO);
    expect(confirmed.message?.getCorrelationId()).toBe(original.getCorrelationId());
    expect(confirmed.message?.getBody()).toEqual({ verb: "capture", ok: true, result: { imageId: "frame-7" } });
    expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, provisioned: 1, settled: 1 });
  });

  it("starts a post-accept continuation only after the inbox accepts an open token", async () => {
    let token: DeferredReply | undefined;
    let continuationRan = false;
    inbox.registerOutcome("post-accept", (req) => {
      token = inbox.defer(req, 2_000);
      expect(token.activate()).toBe(true);
      const settlement = token;
      return CommandOutcomes.deferredWithContinuation(token, async () => {
        continuationRan = true;
        expect(settlement.settleSuccess({ imageId: "frame-post-accept" })).toBe(SettlementResult.Accepted);
      });
    });
    await inbox.start();

    await deliver(messaging, topic("post-accept"), request("post-accept"));

    await waitFor(() => continuationRan && token?.state() === DeferredReplyState.Settled);
    expect(onlyReplyBody(messaging)).toEqual({
      verb: "post-accept",
      ok: true,
      result: { imageId: "frame-post-accept" },
    });
  });

  it("does not start a post-accept continuation for an invalid token", async () => {
    let continuationRan = false;
    inbox.registerOutcome("post-accept-invalid", (req) => {
      const token = inbox.defer(req, 1_000);
      // Deliberately leave this token PROVISIONAL.
      return CommandOutcomes.deferredWithContinuation(token, () => {
        continuationRan = true;
      });
    });
    await inbox.start();

    await deliver(messaging, topic("post-accept-invalid"), request("post-accept-invalid"));
    await tick();

    expect(continuationRan).toBe(false);
    expect((onlyReplyBody(messaging).error as Record<string, unknown>).code)
      .toBe(CommandInbox.ERR_HANDLER_ERROR);
  });

  it("settles a failed post-accept continuation through the guarded error path", async () => {
    let token: DeferredReply | undefined;
    inbox.registerOutcome("post-accept-failure", (req) => {
      token = inbox.defer(req, 2_000);
      expect(token.activate()).toBe(true);
      return CommandOutcomes.deferredWithContinuation(token, async () => {
        throw new Error("simulated camera worker failure");
      });
    });
    await inbox.start();

    await deliver(messaging, topic("post-accept-failure"), request("post-accept-failure"));

    await waitFor(() => token?.state() === DeferredReplyState.Settled);
    expect((onlyReplyBody(messaging).error as Record<string, unknown>).code)
      .toBe(CommandInbox.ERR_HANDLER_ERROR);
  });

  it("discards a provisional token when a handler returns it without durable activation", async () => {
    let token: DeferredReply | undefined;
    inbox.registerOutcome("bad-defer", (req) => {
      token = inbox.defer(req, 1_000);
      return CommandOutcomes.deferred(token);
    });
    await inbox.start();
    await deliver(messaging, topic("bad-defer"), request("bad-defer"));

    expect(token?.state()).toBe(DeferredReplyState.Discarded);
    expect((onlyReplyBody(messaging).error as Record<string, unknown>).code).toBe(CommandInbox.ERR_HANDLER_ERROR);
    expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, discarded: 1 });
  });

  it("rejects unsafe or incomplete deferred requests without consuming registry capacity", () => {
    expectCommandExceptionCode(
      () => inbox.defer(notification("job"), 1_000),
      CommandInbox.ERR_REPLY_REQUIRED,
    );
    const hostile = MessageBuilder.create("job", "1.0")
      .withPayload({})
      .withReplyTo("ecv1/test-thing/TestComponent/main/state")
      .build();
    expect(() => inbox.defer(hostile, 1_000)).toThrow(/reserved/);
    expect(() => inbox.defer(request("job"), 0)).toThrow(/lifetimeMs/);
    expect(() => inbox.defer(request("job"), CommandInbox.MAX_DEFERRED_REPLY_LIFETIME_MS + 1)).toThrow(
      /lifetimeMs/,
    );
    expect(inbox.deferredReplySnapshot().active).toBe(0);
  });

  it("retries failed confirmed replies with the same settlement until one is confirmed", async () => {
    const originalConfirmed = messaging.replyConfirmed.bind(messaging);
    let attempts = 0;
    messaging.replyConfirmed = async (...args) => {
      attempts++;
      if (attempts < 3) throw new Error("ambiguous disconnect");
      await originalConfirmed(...args);
    };
    const token = inbox.defer(request("retry-job"), 2_000);
    expect(token.activate()).toBe(true);
    expect(token.settleError("CAMERA_BUSY", "retry later")).toBe(SettlementResult.Accepted);

    await waitFor(() => token.state() === DeferredReplyState.Settled);
    expect(attempts).toBe(3);
    expect(messaging.published).toHaveLength(1);
    expect(messaging.published[0].message?.getBody()).toEqual({
      verb: "retry-job",
      ok: false,
      error: { code: "CAMERA_BUSY", message: "retry later" },
    });
  });

  it("floors a fractional remaining deferred lifetime before strict confirmation", async () => {
    const now = vi.spyOn(performance, "now");
    now.mockReturnValueOnce(1_000.25).mockReturnValue(1_000.75);
    try {
      const originalConfirmed = messaging.replyConfirmed.bind(messaging);
      let observedTimeout: number | undefined;
      messaging.replyConfirmed = async (...args) => {
        observedTimeout = args[2];
        expect(Number.isInteger(observedTimeout)).toBe(true);
        await originalConfirmed(...args);
      };

      const token = inbox.defer(request("fractional-timeout"), 2_000);
      expect(token.activate()).toBe(true);
      expect(token.settleSuccess({ accepted: true })).toBe(SettlementResult.Accepted);

      await waitFor(() => token.state() === DeferredReplyState.Settled);
      expect(observedTimeout).toBe(1_999);
      expect(messaging.published).toHaveLength(1);
    } finally {
      now.mockRestore();
    }
  });

  it("expires a deferred settlement when less than one whole millisecond remains", async () => {
    const now = vi.spyOn(performance, "now");
    now.mockReturnValueOnce(1_000).mockReturnValue(1_000.75);
    try {
      const token = inbox.defer(request("sub-millisecond-timeout"), 1);
      expect(token.activate()).toBe(true);
      expect(token.settleSuccess({ mustNotPublish: true })).toBe(SettlementResult.Accepted);

      await waitFor(() => token.state() === DeferredReplyState.Expired);
      expect(messaging.published).toHaveLength(0);
      expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, expired: 1 });
    } finally {
      now.mockRestore();
    }
  });

  it("floors a fractional remaining deferred lifetime during shutdown confirmation", async () => {
    const now = vi.spyOn(performance, "now");
    now.mockReturnValueOnce(2_000.25).mockReturnValue(2_000.75);
    try {
      const originalConfirmed = messaging.replyConfirmed.bind(messaging);
      let observedTimeout: number | undefined;
      messaging.replyConfirmed = async (...args) => {
        observedTimeout = args[2];
        expect(Number.isInteger(observedTimeout)).toBe(true);
        await originalConfirmed(...args);
      };

      const token = inbox.defer(request("fractional-shutdown"), 500);
      expect(token.activate()).toBe(true);
      await inbox.close();

      expect(observedTimeout).toBe(499);
      expect(messaging.published).toHaveLength(1);
      expect((messaging.published[0].message?.getBody() as Record<string, unknown>).error)
        .toMatchObject({ code: CommandInbox.ERR_COMPONENT_STOPPING });
    } finally {
      now.mockRestore();
    }
  });

  it("does not extend a shutdown confirmation beyond a sub-millisecond deferred lifetime", async () => {
    const now = vi.spyOn(performance, "now");
    now.mockReturnValueOnce(2_000).mockReturnValue(2_000.75);
    try {
      const token = inbox.defer(request("sub-millisecond-shutdown"), 1);
      expect(token.activate()).toBe(true);
      await inbox.close();

      expect(token.state()).toBe(DeferredReplyState.CancelledOnShutdown);
      expect(messaging.published).toHaveLength(0);
      expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, cancelledOnShutdown: 1 });
    } finally {
      now.mockRestore();
    }
  });

  it("expires an open deferred reply with stable counters and terminal settlement result", async () => {
    const token = inbox.defer(request("expires"), 25);
    expect(token.activate()).toBe(true);
    await waitFor(() => token.state() === DeferredReplyState.Expired);

    expect(token.settleSuccess()).toBe(SettlementResult.Expired);
    expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, expired: 1, openExpired: 1 });
  });

  it("gives exactly one winner to concurrent settlers", async () => {
    const token = inbox.defer(request("race"), 1_000);
    token.activate();
    const results = await Promise.all([
      Promise.resolve().then(() => token.settleSuccess({ winner: "success" })),
      Promise.resolve().then(() => token.settleError("LOSER", "must not publish")),
    ]);

    expect(results.filter((r) => r === SettlementResult.Accepted)).toHaveLength(1);
    expect(results.filter((r) => r === SettlementResult.AlreadySettled)).toHaveLength(1);
    await waitFor(() => token.state() === DeferredReplyState.Settled);
    expect(messaging.published).toHaveLength(1);
  });

  it("uses one atomic winner when settlement and expiration become runnable together", async () => {
    vi.useFakeTimers();
    try {
      // The expiration timer is registered first; a settler registered for the same deadline
      // therefore observes the already-terminal state and cannot publish.
      const expirationWins = inbox.defer(request("expiry-race"), 10);
      expirationWins.activate();
      let lateResult: SettlementResult | undefined;
      setTimeout(() => {
        lateResult = expirationWins.settleSuccess({ mustNotPublish: true });
      }, 10);
      await vi.advanceTimersByTimeAsync(10);
      expect(expirationWins.state()).toBe(DeferredReplyState.Expired);
      expect(lateResult).toBe(SettlementResult.Expired);
      expect(messaging.published).toHaveLength(0);

      // Conversely, a synchronous OPEN -> SETTLING winner owns the result; advancing through
      // the same deadline cannot overwrite the confirmed SETTLED terminal state.
      const settlementWins = inbox.defer(request("settlement-race"), 10);
      settlementWins.activate();
      expect(settlementWins.settleSuccess({ winner: true })).toBe(SettlementResult.Accepted);
      await vi.advanceTimersByTimeAsync(10);
      expect(settlementWins.state()).toBe(DeferredReplyState.Settled);
      expect(messaging.published).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("attempts COMPONENT_STOPPING for open work, cancels all tokens, and rejects new deferrals", async () => {
    const open = inbox.defer(request("open-job"), 5_000);
    const provisional = inbox.defer(request("provisional-job"), 5_000);
    open.activate();
    await inbox.close();

    expect(open.state()).toBe(DeferredReplyState.CancelledOnShutdown);
    expect(provisional.state()).toBe(DeferredReplyState.CancelledOnShutdown);
    expect(messaging.published).toHaveLength(1);
    expect(messaging.published[0].kind).toBe("replyConfirmed");
    expect((messaging.published[0].message?.getBody() as Record<string, unknown>).error).toEqual({
      code: CommandInbox.ERR_COMPONENT_STOPPING,
      message: "the component stopped before the deferred command could reply",
    });
    expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, cancelledOnShutdown: 2 });
    expectCommandExceptionCode(
      () => inbox.defer(request("late"), 100),
      CommandInbox.ERR_COMPONENT_STOPPING,
    );
  });

  it("bounds the deferred registry at 1024 entries and reports capacity rejection", async () => {
    const req = request("capacity");
    for (let i = 0; i < CommandInbox.MAX_DEFERRED_REPLIES; i++) inbox.defer(req, 30_000);
    expect(inbox.deferredReplySnapshot().active).toBe(CommandInbox.MAX_DEFERRED_REPLIES);
    expectCommandExceptionCode(
      () => inbox.defer(req, 30_000),
      CommandInbox.ERR_DEFERRED_REPLY_CAPACITY,
    );
    expect(inbox.deferredReplySnapshot().capacityRejected).toBe(1);
    await inbox.close();
    expect(inbox.deferredReplySnapshot()).toMatchObject({ active: 0, cancelledOnShutdown: 1024 });
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
