/**
 * The TS loader for the cross-language `uns-test-vectors/commands.json` conformance suite
 * (DESIGN-uns §9.5, the minimal `commands()` facade — edge-console slice S2), mirroring the
 * `bcast_vectors.test.ts` / `uns_vectors.test.ts` loader pattern. See
 * `uns-test-vectors/README.md` "commands.json command-inbox contract" for the normative shape.
 *
 * Two kinds of checks, per the vector document's own description ("request/reply envelopes
 * structural (D-U22); reply bodies must equal a live inbox dispatch's output"):
 * - **Static reconstruction** — the inbox filter and every verb/error topic are rebuilt
 *   byte-for-byte through the real `Uns` builder; the golden request/reply envelopes are
 *   rebuilt through `MessageBuilder` (pinned uuid/timestamp/correlation_id/identity) and
 *   compared structurally — this proves wire-format compliance independent of dispatch.
 * - **Live dispatch** — a real `CommandInbox`, wired with injected built-in actions that
 *   reproduce the vector's pinned values (`uptimeSecs: 42`, a successful reload, and the
 *   exact `get-configuration` golden's redacted config), is started and fed each golden
 *   request (and the `unknown-verb` error case); the resulting reply's `header.name`,
 *   `header.version`, `header.correlation_id` (= the request's), and — the normative bit —
 *   **body** must equal the golden reply's, byte for byte. `uuid`/`timestamp` are NOT compared
 *   on the live reply (they are freshly minted per dispatch, unlike the golden's pinned
 *   example values).
 *
 * Existence-guarded like the other vector loaders: skips when the vector file has not been
 * generated yet (the file is committed, so CI always exercises this).
 */
import { existsSync, readFileSync } from "fs";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { Config } from "../src/config/model";
import { CommandInbox } from "../src/commands";
import { Message, MessageBuilder, MessageIdentity } from "../src/message";
import type { HierLevel } from "../src/message";
import { Uns, UnsClass, unsClassFromToken } from "../src/uns";
import { RecordingMessagingService } from "./_fakes";

const VECTORS = join(__dirname, "..", "..", "..", "uns-test-vectors");
const COMMANDS_PATH = join(VECTORS, "commands.json");
const present = existsSync(COMMANDS_PATH);

interface VectorHeader {
  name: string;
  version: string;
  timestamp: string;
  uuid: string;
  correlation_id: string;
  reply_to?: string;
}

interface VectorIdentity {
  hier: HierLevel[];
  path: string;
  component: string;
  instance: string;
}

interface VectorEnvelope {
  header: VectorHeader;
  identity?: VectorIdentity;
  body: Record<string, unknown>;
}

interface VerbVector {
  name: string;
  verb: string;
  topic: string;
  request: VectorEnvelope;
  reply: VectorEnvelope;
}

interface CommandsDocument {
  description: string;
  inbox: {
    filter: string;
    componentFilter: string;
    input: { device: string; component: string; instance: string; includeRoot: boolean; class: string };
  };
  verbs: VerbVector[];
  errors: VerbVector[];
  behavior: {
    verbIsTopicChannel: boolean;
    headerNameMustEqualVerb: boolean;
    fireAndForgetWithoutReplyTo: boolean;
    malformedIgnoredWithoutReply: boolean;
    builtInVerbs: string[];
    delegatedVerbs: string[];
    errorCodes: string[];
  };
}

function load(): CommandsDocument {
  return JSON.parse(readFileSync(COMMANDS_PATH, "utf8")) as CommandsDocument;
}

/**
 * The config the live inbox binds to. commands.json is generated from an identity carrying the
 * explicit instance token `main` (Java `SINGLE_IDENTITY`), so its inbox filters are the instance
 * filter `.../main/cmd/#` plus the component filter `.../cmd/#`, and its replies/describe body carry
 * `instance: "main"`. Bind the same identity so the live output matches byte-for-byte. Under D-U28
 * `Config.fromValue` resolves component scope, so the bound instance is applied to the identity here.
 */
function boundConfig(doc: CommandsDocument): () => Config {
  const base = Config.fromValue(doc.inbox.input.component, doc.inbox.input.device, {});
  const bound = { ...base, componentIdentity: base.componentIdentity.withInstance(doc.inbox.input.instance) } as Config;
  return () => bound;
}

/** Rebuilds a golden envelope through `MessageBuilder` with every pinned setter (D-U22). */
function rebuildEnvelope(env: VectorEnvelope): Record<string, unknown> {
  const builder = MessageBuilder.create(env.header.name, env.header.version)
    .withUuid(env.header.uuid)
    .withTimestamp(env.header.timestamp)
    .withCorrelationId(env.header.correlation_id)
    .withPayload(env.body);
  if (env.header.reply_to) {
    builder.withReplyTo(env.header.reply_to);
  }
  if (env.identity) {
    builder.withIdentity(
      new MessageIdentity(env.identity.hier, env.identity.component, env.identity.instance, env.identity.path),
    );
  }
  return builder.build().toObject();
}

describe.skipIf(!present)("uns-test-vectors/commands.json — CommandInbox conformance", () => {
  it("pins the built-in verb goldens, in order, plus one unknown-verb error case", () => {
    const doc = load();
    expect(doc.verbs.map((v) => v.name)).toEqual([
      CommandInbox.PING,
      CommandInbox.DESCRIBE,
      CommandInbox.RELOAD_CONFIG,
      CommandInbox.GET_CONFIGURATION,
      CommandInbox.STATUS,
    ]);
    expect(doc.errors).toHaveLength(1);
    expect(doc.errors[0].name).toBe("unknown-verb");
  });

  it("the inbox filter is reproduced byte-for-byte through the Uns filter builder", () => {
    const doc = load();
    const identity = new MessageIdentity(
      [{ level: "device", value: doc.inbox.input.device }],
      doc.inbox.input.component,
      doc.inbox.input.instance,
    );
    const cls = unsClassFromToken(doc.inbox.input.class);
    expect(cls, "inbox input class token").toBeDefined();
    const uns = new Uns(identity, doc.inbox.input.includeRoot);
    const scope = {
      device: doc.inbox.input.device,
      component: doc.inbox.input.component,
      instance: doc.inbox.input.instance,
    };
    // Pinning the explicit instance reproduces the instance-scoped filter byte-for-byte.
    expect(uns.filter(cls!, scope)).toBe(doc.inbox.filter);
    // D-U28: omitting the instance slot reproduces the component-scope filter byte-for-byte.
    expect(uns.filter(cls!, scope, false)).toBe(doc.inbox.componentFilter);
  });

  it("every verb/error topic is reproduced byte-for-byte (the verb is the cmd channel)", () => {
    const doc = load();
    const identity = new MessageIdentity(
      [{ level: "device", value: doc.inbox.input.device }],
      doc.inbox.input.component,
      doc.inbox.input.instance,
    );
    const uns = new Uns(identity, doc.inbox.input.includeRoot);
    for (const v of [...doc.verbs, ...doc.errors]) {
      expect(v.request.header.name, `'${v.name}' header.name must equal the topic's verb`).toBe(v.verb);
      const topic = uns.topic(UnsClass.Cmd, v.verb);
      expect(topic, `'${v.name}' topic`).toBe(v.topic);
    }
  });

  it("every verb's request/reply envelope is reproduced structurally (D-U22)", () => {
    const doc = load();
    for (const v of [...doc.verbs, ...doc.errors]) {
      expect(rebuildEnvelope(v.request), `'${v.name}' request`).toEqual(v.request as unknown as Record<string, unknown>);
      expect(rebuildEnvelope(v.reply), `'${v.name}' reply`).toEqual(v.reply as unknown as Record<string, unknown>);
      // The reply's correlation_id is always the REQUEST's, never a fresh one.
      expect(v.reply.header.correlation_id, `'${v.name}' reply correlation_id = request correlation_id`).toBe(
        v.request.header.correlation_id,
      );
      // Replies never carry a reply_to (terminal - no reply-to-a-reply).
      expect(v.reply.header.reply_to, `'${v.name}' reply carries no reply_to`).toBeUndefined();
    }
  });

  it("the verb goldens, replayed through a LIVE inbox, produce the golden reply bodies", async () => {
    const doc = load();
    const getConfigVerb = doc.verbs.find((v) => v.name === CommandInbox.GET_CONFIGURATION);
    expect(getConfigVerb, "get-configuration golden must be present").toBeDefined();
    const goldenRedactedConfig = (getConfigVerb!.reply.body.result as Record<string, unknown>).config as Record<
      string,
      unknown
    >;

    const config = boundConfig(doc);
    const messaging = new RecordingMessagingService();
    const inbox = new CommandInbox(
      config,
      messaging,
      () => 42, // matches the ping golden's pinned uptimeSecs
      () => true, // matches the reload-config golden's {reloaded: true}
      () => goldenRedactedConfig, // matches the get-configuration golden byte-for-byte
    );
    await inbox.start();
    // D-U28: the live component-scoped inbox subscribes the component-scope filter and the
    // instance-scope wildcard. The verb goldens carry an explicit `.../main/cmd/...` topic, so the
    // dispatcher (verb located via the `/cmd/` marker) handles either scope; drive it through the
    // subscribed component-scope handler.
    expect(
      messaging.subscriptions.has(doc.inbox.componentFilter),
      "the live inbox must subscribe the component-scope filter (D-U28)",
    ).toBe(true);
    const handler = messaging.subscriptions.get(doc.inbox.componentFilter)!;

    for (const v of doc.verbs) {
      const request = rebuiltRequestMessage(v);
      await handler(v.topic, request);
    }

    expect(messaging.published, "one reply per verb golden").toHaveLength(doc.verbs.length);
    doc.verbs.forEach((v, i) => {
      const rec = messaging.published[i];
      expect(rec.topic, `'${v.name}' reply topic = request reply_to`).toBe(v.request.header.reply_to);
      expect(rec.message!.header.name, `'${v.name}' reply header.name`).toBe(v.reply.header.name);
      expect(rec.message!.header.version, `'${v.name}' reply header.version`).toBe(v.reply.header.version);
      expect(rec.message!.header.correlation_id, `'${v.name}' reply correlation_id`).toBe(
        v.reply.header.correlation_id,
      );
      expect(rec.message!.getBody(), `'${v.name}' reply body must equal the live inbox dispatch's output`).toEqual(
        v.reply.body,
      );
    });
  });

  it("the unknown-verb golden, replayed through a LIVE inbox, produces the golden UNKNOWN_VERB reply", async () => {
    const doc = load();
    const errorVector = doc.errors[0];

    const config = boundConfig(doc);
    const messaging = new RecordingMessagingService();
    const inbox = new CommandInbox(config, messaging, () => 42, () => true, () => undefined);
    await inbox.start();
    const handler = messaging.subscriptions.get(doc.inbox.componentFilter)!;

    const request = rebuiltRequestMessage(errorVector);
    await handler(errorVector.topic, request);

    expect(messaging.published).toHaveLength(1);
    const rec = messaging.published[0];
    expect(rec.topic).toBe(errorVector.request.header.reply_to);
    expect(rec.message!.header.correlation_id).toBe(errorVector.reply.header.correlation_id);
    expect(rec.message!.getBody(), "the UNKNOWN_VERB reply body must equal the golden's byte for byte").toEqual(
      errorVector.reply.body,
    );
  });

  it("the normative behavior constants and sets match CommandInbox exactly", () => {
    const doc = load();
    expect(doc.behavior.verbIsTopicChannel).toBe(true);
    expect(doc.behavior.headerNameMustEqualVerb).toBe(true);
    expect(doc.behavior.fireAndForgetWithoutReplyTo).toBe(true);
    expect(doc.behavior.malformedIgnoredWithoutReply).toBe(true);
    expect(doc.behavior.builtInVerbs, "builtInVerbs pins the canonical built-in set").toEqual([
      CommandInbox.PING,
      CommandInbox.DESCRIBE,
      CommandInbox.RELOAD_CONFIG,
      CommandInbox.GET_CONFIGURATION,
      CommandInbox.STATUS,
    ]);
    expect(new Set(doc.behavior.builtInVerbs)).toEqual(CommandInbox.BUILT_IN_VERBS);
    expect(new Set(doc.behavior.delegatedVerbs)).toEqual(CommandInbox.DELEGATED_VERBS);
    expect(new Set(doc.behavior.errorCodes)).toEqual(
      new Set([
        CommandInbox.ERR_UNKNOWN_VERB,
        CommandInbox.ERR_HANDLER_ERROR,
        CommandInbox.ERR_RELOAD_FAILED,
        CommandInbox.ERR_NO_CONFIG,
      ]),
    );
  });
});

/** Rebuilds a golden vector's REQUEST as a live `Message` (pinned uuid/timestamp/correlation_id/reply_to). */
function rebuiltRequestMessage(v: VerbVector): Message {
  const builder = MessageBuilder.create(v.request.header.name, v.request.header.version)
    .withUuid(v.request.header.uuid)
    .withTimestamp(v.request.header.timestamp)
    .withCorrelationId(v.request.header.correlation_id)
    .withPayload(v.request.body);
  if (v.request.header.reply_to) {
    builder.withReplyTo(v.request.header.reply_to);
  }
  return builder.build();
}
