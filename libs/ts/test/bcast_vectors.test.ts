/**
 * The TS loader for the cross-language `uns-test-vectors/bcast.json` conformance suite
 * (DESIGN-uns §9.4, the `_bcast` republish listener's wire contract), mirroring the Java
 * `UnsTestVectors.assertBcastDocument` and the `uns_vectors.test.ts` loader pattern.
 *
 * Replays every command case through the LIVE implementation:
 * - the topic is rebuilt byte-for-byte through the real `Uns` builder with the reserved
 *   `_bcast` pseudo-component identity (single-level -> rootless by D-U25);
 * - the envelope is rebuilt through `MessageBuilder` (pinned uuid/timestamp/correlation_id) and
 *   compared structurally — a notification-style `cmd` envelope: no `identity`, no `tags`, no
 *   `reply_to`, empty body, `header.name` = the verb;
 * - the document's `behavior` block is asserted equal to `RepublishListener`'s normative
 *   constants (`JITTER_WINDOW_MS`/`COOLDOWN_MS`) and `replyTo: false`.
 *
 * Existence-guarded like the `uns_vectors.test.ts` / vault loaders: skips when the vector file
 * has not been generated yet (the file is committed, so CI always exercises this).
 */
import { existsSync, readFileSync } from "fs";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { MessageBuilder, MessageIdentity } from "../src/message";
import { RepublishListener } from "../src/republish_listener";
import { Uns, unsClassFromToken } from "../src/uns";

const VECTORS = join(__dirname, "..", "..", "..", "uns-test-vectors");
const BCAST_PATH = join(VECTORS, "bcast.json");
const present = existsSync(BCAST_PATH);

interface BcastCommandVector {
  name: string;
  republishes: string;
  topic: string;
  input: {
    device: string;
    component: string;
    instance: string;
    includeRoot: boolean;
    class: string;
    channel: string;
  };
  envelope: {
    header: Record<string, string>;
    body: Record<string, unknown>;
  };
}

interface BcastDocument {
  description: string;
  device: string;
  commands: BcastCommandVector[];
  behavior: { jitterWindowMs: number; cooldownMs: number; replyTo: boolean };
}

function load(): BcastDocument {
  return JSON.parse(readFileSync(BCAST_PATH, "utf8")) as BcastDocument;
}

describe.skipIf(!present)("uns-test-vectors/bcast.json — RepublishListener conformance", () => {
  it("pins exactly the two republish commands, in verb order", () => {
    const doc = load();
    expect(doc.commands).toHaveLength(2);
    expect(doc.commands.map((c) => c.name)).toEqual([
      RepublishListener.REPUBLISH_STATE,
      RepublishListener.REPUBLISH_CFG,
    ]);
  });

  it("every command's topic is reproduced byte-for-byte through the Uns builder + the _bcast identity", () => {
    const doc = load();
    for (const c of doc.commands) {
      expect(c.input.device, `'${c.name}' input device`).toBe(doc.device);
      expect(c.input.component, `'${c.name}' pseudo-component token`).toBe(RepublishListener.BCAST_COMPONENT);

      const bcastIdentity = new MessageIdentity(
        [{ level: "device", value: c.input.device }],
        c.input.component,
        c.input.instance,
      );
      const cls = unsClassFromToken(c.input.class);
      expect(cls, `'${c.name}' input class token`).toBeDefined();
      const topic = new Uns(bcastIdentity, c.input.includeRoot).topic(cls!, c.input.channel);
      expect(topic, `'${c.name}' topic`).toBe(c.topic);
    }
  });

  it("every command's envelope is a fire-and-forget notification: no identity/tags/reply_to, empty body", () => {
    const doc = load();
    for (const c of doc.commands) {
      const header = c.envelope.header;
      expect(header.name, `'${c.name}' header name`).toBe(c.name);
      expect(header.version, `'${c.name}' header version`).toBe("1.0");
      expect("reply_to" in c.envelope, `'${c.name}' is fire-and-forget - no reply_to`).toBe(false);
      expect("identity" in c.envelope, `'${c.name}' is built without a config-bound builder - no identity`).toBe(
        false,
      );
      expect("tags" in c.envelope, `'${c.name}' carries no tags`).toBe(false);
      expect(c.envelope.body, `'${c.name}' body is the empty object`).toEqual({});

      const rebuilt = MessageBuilder.create(header.name, header.version)
        .withUuid(header.uuid)
        .withTimestamp(header.timestamp)
        .withCorrelationId(header.correlation_id)
        .withPayload(c.envelope.body)
        .build();
      expect(rebuilt.toObject(), `'${c.name}' envelope`).toEqual(c.envelope);
      // And the parse direction: the wire JSON round-trips to the same structure.
      expect(JSON.parse(rebuilt.toJSON()), `'${c.name}' envelope (wire)`).toEqual(c.envelope);
    }
  });

  it("the normative behavior constants match RepublishListener exactly", () => {
    const doc = load();
    expect(doc.behavior.jitterWindowMs, "jitterWindowMs must equal the implementation's window").toBe(
      RepublishListener.JITTER_WINDOW_MS,
    );
    expect(doc.behavior.cooldownMs, "cooldownMs must equal the implementation's cooldown").toBe(
      RepublishListener.COOLDOWN_MS,
    );
    expect(doc.behavior.replyTo, "the republish broadcast never carries a reply_to").toBe(false);
  });
});
