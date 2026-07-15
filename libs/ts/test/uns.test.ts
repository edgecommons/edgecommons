/**
 * Unit tests for the UNS topic builder/validator (`src/uns.ts`) — the §2.2 grammar rules,
 * D-U25 (includeRoot needs a multi-level hierarchy), D-U26 (token rule ≡ the sanitizer), the
 * §4.1 reserved-class guard predicate + its enforcement on the messaging service, and the §5
 * request-deadline default. The cross-language byte-level pins live in `uns_vectors.test.ts`;
 * these tests cover behavior the vectors do not (bound-identity paths, topicFor, the guard on
 * every public publish path, the privileged seam).
 */
import { describe, expect, it } from "vitest";

import { Message, MessageBuilder, MessageIdentity } from "../src/message";
import { DefaultMessagingService, publishReservedVia } from "../src/messaging/service";
import { RequestTimeoutError, ReservedTopicError } from "../src/messaging/types";
import {
  MAX_TOPIC_UTF8_BYTES,
  RESERVED_CLASSES,
  Uns,
  UnsClass,
  UnsScope,
  UnsValidationError,
  checkToken,
  isLeafClass,
  reservedClassOf,
  unsClassFromToken,
} from "../src/uns";
import { FakeMessagingProvider, RecordingMessagingService, tick } from "./_fakes";

const SINGLE = new MessageIdentity([{ level: "device", value: "gw-01" }], "comp");
const MULTI = new MessageIdentity(
  [
    { level: "site", value: "dallas" },
    { level: "zone", value: "zone-3" },
    { level: "device", value: "gw-01" },
  ],
  "comp",
);

function codeOf(fn: () => unknown): string | undefined {
  try {
    fn();
    return undefined;
  } catch (e) {
    expect(e).toBeInstanceOf(UnsValidationError);
    return (e as UnsValidationError).code;
  }
}

describe("UnsClass", () => {
  it("closed set, leaf semantics, reserved set", () => {
    expect(unsClassFromToken("state")).toBe(UnsClass.State);
    expect(unsClassFromToken("bogus")).toBeUndefined();
    expect(isLeafClass(UnsClass.State)).toBe(true);
    expect(isLeafClass(UnsClass.Cfg)).toBe(true);
    expect(isLeafClass(UnsClass.Data)).toBe(false);
    expect([...RESERVED_CLASSES].sort()).toEqual(["cfg", "log", "metric", "state"]);
  });
});

describe("Uns.topic / topicFor", () => {
  it("builds leaf and channeled topics from the bound identity", () => {
    // D-U28: SINGLE/MULTI are component-scoped identities, so the instance slot is omitted.
    const uns = new Uns(SINGLE, false);
    expect(uns.topic(UnsClass.State)).toBe("ecv1/gw-01/comp/state");
    expect(uns.topic(UnsClass.Data, "temp")).toBe("ecv1/gw-01/comp/data/temp");
    expect(uns.topic(UnsClass.Cmd, "sb/status")).toBe("ecv1/gw-01/comp/cmd/sb/status");
    expect(uns.identity()).toBe(SINGLE);
  });

  it("an instance-bound identity emits the instance slot (D-U28)", () => {
    const uns = new Uns(SINGLE.withInstance("kep1"), false);
    expect(uns.topic(UnsClass.State)).toBe("ecv1/gw-01/comp/kep1/state");
    expect(uns.topic(UnsClass.Data, "temp")).toBe("ecv1/gw-01/comp/kep1/data/temp");
  });

  it("multi-level rootless uses the device (last hier value); rooted prepends hier[0]", () => {
    expect(new Uns(MULTI, false).topic(UnsClass.State)).toBe("ecv1/gw-01/comp/state");
    expect(new Uns(MULTI, true).topic(UnsClass.State)).toBe("ecv1/dallas/gw-01/comp/state");
  });

  it("includeRoot is a no-op on a single-level hierarchy (D-U25)", () => {
    expect(new Uns(SINGLE, true).topic(UnsClass.State)).toBe("ecv1/gw-01/comp/state");
    // ...and the no-op leaves the multi-token channel budget intact.
    expect(new Uns(SINGLE, true).topic(UnsClass.Data, "a/b/c")).toBe("ecv1/gw-01/comp/data/a/b/c");
  });

  it("topicFor mints a topic for a peer identity (a received message's identity)", () => {
    const peer = new MessageIdentity([{ level: "device", value: "gw-02" }], "modbus-adapter", "kep1");
    expect(new Uns(SINGLE, false).topicFor(peer, UnsClass.Cmd, "set-log-level")).toBe(
      "ecv1/gw-02/modbus-adapter/kep1/cmd/set-log-level",
    );
  });

  it("enforces the class/channel rules with precise codes", () => {
    const uns = new Uns(SINGLE, false);
    expect(codeOf(() => uns.topic(UnsClass.State, "x"))).toBe("CHANNEL_ON_LEAF");
    expect(codeOf(() => uns.topic(UnsClass.Data))).toBe("CHANNEL_REQUIRED");
    expect(codeOf(() => uns.topic(UnsClass.Data, ""))).toBe("CHANNEL_REQUIRED");
    expect(codeOf(() => uns.topic(UnsClass.Data, "a//b"))).toBe("EMPTY_TOKEN");
    expect(codeOf(() => uns.topic(UnsClass.Data, "te+mp"))).toBe("BAD_CHAR");
    expect(codeOf(() => uns.topic(UnsClass.Data, "a..b"))).toBe("TRAVERSAL");
    // D-U28: component scope frees one channel slot, so depth is exceeded one token later.
    expect(codeOf(() => uns.topic(UnsClass.Data, "a/b/c/d/e"))).toBe("DEPTH_EXCEEDED");
    expect(codeOf(() => new Uns(MULTI, true).topic(UnsClass.Data, "a/b/c/d"))).toBe("DEPTH_EXCEEDED");
    expect(codeOf(() => uns.topic(UnsClass.Data, "x".repeat(MAX_TOPIC_UTF8_BYTES)))).toBe("LENGTH_EXCEEDED");
  });

  it("length limit counts UTF-8 bytes, not characters", () => {
    const uns = new Uns(SINGLE, false);
    // 80 x U+00E9 (2 UTF-8 bytes each) in 3 channel tokens: 240 bytes of channel + the 27-byte
    // prefix + separators pushes past 256 bytes at well under 256 characters.
    const channel = `${"é".repeat(80)}/${"é".repeat(40)}`;
    expect(codeOf(() => uns.topic(UnsClass.Data, channel))).toBe("LENGTH_EXCEEDED");
  });
});

describe("Uns.filter", () => {
  it("wildcards absent scope fields; leaf classes end at the class token", () => {
    const uns = new Uns(SINGLE, false);
    expect(uns.filter(UnsClass.Data, UnsScope.all())).toBe("ecv1/+/+/+/data/#");
    expect(uns.filter(UnsClass.State, UnsScope.all())).toBe("ecv1/+/+/+/state");
    expect(uns.filter(UnsClass.Data, UnsScope.device("gw-01"))).toBe("ecv1/gw-01/+/+/data/#");
    expect(uns.filter(UnsClass.Evt, UnsScope.component("gw-01", "comp"))).toBe("ecv1/gw-01/comp/+/evt/#");
    expect(uns.filter(UnsClass.Cmd, UnsScope.instance("gw-01", "comp", "kep1"))).toBe(
      "ecv1/gw-01/comp/kep1/cmd/#",
    );
  });

  it("the site position exists only under an effective root (multi-level + includeRoot)", () => {
    expect(new Uns(MULTI, true).filter(UnsClass.Data, UnsScope.all())).toBe("ecv1/+/+/+/+/data/#");
    expect(new Uns(MULTI, true).filter(UnsClass.Data, { site: "dallas", device: "gw-01" })).toBe(
      "ecv1/dallas/gw-01/+/+/data/#",
    );
    // Rootless (or single-level) binding ignores scope.site.
    expect(new Uns(MULTI, false).filter(UnsClass.Data, { site: "dallas" })).toBe("ecv1/+/+/+/data/#");
    expect(new Uns(SINGLE, true).filter(UnsClass.Data, { site: "dallas" })).toBe("ecv1/+/+/+/data/#");
  });

  it("a pinned scope field must pass the token rule", () => {
    expect(codeOf(() => new Uns(SINGLE, false).filter(UnsClass.Data, UnsScope.device("gw+01")))).toBe("BAD_CHAR");
  });
});

describe("Uns.validate", () => {
  const uns = new Uns(MULTI, false);
  const rooted = new Uns(MULTI, true);

  it("accepts concrete grammar-conformant topics (instance and component scope, D-U28)", () => {
    expect(() => uns.validate("ecv1/gw-01/comp/main/state")).not.toThrow();
    expect(() => uns.validate("ecv1/gw-01/comp/main/cmd/sb/status")).not.toThrow();
    expect(() => rooted.validate("ecv1/dallas/gw-01/comp/main/state")).not.toThrow();
    // D-U28: the instance slot is optional — component-scope topics validate too.
    expect(() => uns.validate("ecv1/gw-01/comp/state")).not.toThrow();
    expect(() => uns.validate("ecv1/gw-01/comp/cmd/sb/status")).not.toThrow();
    expect(() => rooted.validate("ecv1/dallas/gw-01/comp/state")).not.toThrow();
  });

  it("rejects with precise codes", () => {
    expect(codeOf(() => uns.validate(""))).toBe("EMPTY_TOKEN");
    expect(codeOf(() => uns.validate("ecv1//comp/main/state"))).toBe("EMPTY_TOKEN");
    expect(codeOf(() => uns.validate("ecv1/gw\\01/c/i/state"))).toBe("BAD_CHAR");
    expect(codeOf(() => uns.validate("ecv1/a..b/c/i/state"))).toBe("TRAVERSAL");
    expect(codeOf(() => uns.validate("notroot/d/c/i/state"))).toBe("BAD_ROOT");
    expect(codeOf(() => uns.validate("edgecommons/reply-42/x/main/state"))).toBe("BAD_ROOT");
    expect(codeOf(() => uns.validate("ecv1/d/c/i/data/a/b/c/d"))).toBe("DEPTH_EXCEEDED");
    expect(codeOf(() => uns.validate(`ecv1/${"d".repeat(250)}/c/i/state`))).toBe("LENGTH_EXCEEDED");
    expect(codeOf(() => uns.validate("ecv1/d/c/i/state/extra"))).toBe("CHANNEL_ON_LEAF");
    expect(codeOf(() => uns.validate("ecv1/d/c/i/data"))).toBe("CHANNEL_REQUIRED");
    expect(codeOf(() => uns.validate("ecv1/d/c/i/bogus/x"))).toBe("BAD_CLASS");
    expect(codeOf(() => uns.validate("ecv1/d/c/i"))).toBe("BAD_CLASS");
    expect(codeOf(() => uns.validate("ecv1/+/c/i/state"))).toBe("WILDCARD_IN_TOPIC");
    expect(codeOf(() => uns.validate("ecv1/d/c/i/data/#"))).toBe("WILDCARD_IN_TOPIC");
  });

  it("locates the class by the class-token set, not a fixed position (D-U28)", () => {
    // A rootless-shaped topic under rooted mode now validates: the class token is LOCATED
    // (the leading levels are read as component scope), matching the regenerated vector
    // `validate-rootless-topic-under-rooted-mode`.
    expect(() => rooted.validate("ecv1/gw-01/comp/main/state")).not.toThrow();
    // A rooted-shaped topic under rootless mode still fails — the token where the class must be
    // (`main`) is not a class token.
    expect(codeOf(() => uns.validate("ecv1/dallas/gw-01/comp/main"))).toBe("BAD_CLASS");
  });
});

describe("checkToken (the D-U26 token rule ≡ the sanitizer)", () => {
  it("accepts sanitized values (spaces, dots) and rejects the blacklist incl. C1", () => {
    expect(() => checkToken("gw 01", "t")).not.toThrow(); // spaces are legal
    expect(() => checkToken("v1.2", "t")).not.toThrow(); // dots are legal
    expect(codeOf(() => checkToken("", "t"))).toBe("EMPTY_TOKEN");
    expect(codeOf(() => checkToken(undefined, "t"))).toBe("EMPTY_TOKEN");
    expect(codeOf(() => checkToken("a/b", "t"))).toBe("BAD_CHAR");
    expect(codeOf(() => checkToken("a+b", "t"))).toBe("BAD_CHAR");
    expect(codeOf(() => checkToken("a#b", "t"))).toBe("BAD_CHAR");
    expect(codeOf(() => checkToken("a\\b", "t"))).toBe("BAD_CHAR");
    expect(codeOf(() => checkToken("ab", "t"))).toBe("BAD_CHAR"); // C0
    expect(codeOf(() => checkToken("ab", "t"))).toBe("BAD_CHAR"); // DEL
    expect(codeOf(() => checkToken("ab", "t"))).toBe("BAD_CHAR"); // C1 (D-U26)
    expect(codeOf(() => checkToken("a..b", "t"))).toBe("TRAVERSAL");
  });
});

describe("reserved-class guard (§4.1)", () => {
  it("predicate: position 4 always, position 5 only when includeRoot", () => {
    expect(reservedClassOf("ecv1/d/c/i/state", false)).toBe(UnsClass.State);
    expect(reservedClassOf("ecv1/d/c/i/metric/cpu", false)).toBe(UnsClass.Metric);
    expect(reservedClassOf("ecv1/d/c/i/data/temp", false)).toBeUndefined();
    expect(reservedClassOf("ecv1/d/c/i/app/state", false)).toBeUndefined();
    expect(reservedClassOf("ecv1/s/d/c/i/state", true)).toBe(UnsClass.State);
    expect(reservedClassOf("ecv1/s/d/c/i/state", false)).toBeUndefined();
    expect(reservedClassOf("ecv1/s/d/c/i/app/state", true)).toBeUndefined();
    expect(reservedClassOf("edgecommons/reply-1", false)).toBeUndefined();
    expect(reservedClassOf("cloudwatch/metric/put", false)).toBeUndefined();
    expect(reservedClassOf("ecv1x/d/c/i/state", false)).toBeUndefined();
    expect(reservedClassOf(undefined, false)).toBeUndefined();
  });

  it("guards publish/publishRaw/publishNorthbound*/request*/reply* on the messaging service", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const reserved = "ecv1/gw-01/comp/main/state";
    const msg = (): Message => MessageBuilder.create("x", "1").withPayload({}).build();

    await expect(svc.publish(reserved, msg())).rejects.toBeInstanceOf(ReservedTopicError);
    await expect(svc.publishRaw(reserved, {})).rejects.toBeInstanceOf(ReservedTopicError);
    await expect(svc.publishNorthbound(reserved, msg())).rejects.toBeInstanceOf(ReservedTopicError);
    await expect(svc.publishNorthboundRaw(reserved, {})).rejects.toBeInstanceOf(ReservedTopicError);
    expect(() => svc.request(reserved, msg())).toThrow(ReservedTopicError);
    expect(() => svc.requestNorthbound(reserved, msg())).toThrow(ReservedTopicError);
    // Hostile reply_to forgery (D-U8): a request whose reply_to targets a reserved topic.
    const forged = MessageBuilder.create("x", "1").withReplyTo(reserved).build();
    const reply = msg();
    await expect(svc.reply(forged, reply)).rejects.toBeInstanceOf(ReservedTopicError);
    await expect(svc.replyNorthbound(forged, reply)).rejects.toBeInstanceOf(ReservedTopicError);
    // The error names the topic and the class token.
    await svc.publish(reserved, msg()).catch((e: ReservedTopicError) => {
      expect(e.topic).toBe(reserved);
      expect(e.classToken).toBe("state");
    });
    // Non-reserved topics pass; subscribe is never guarded.
    await expect(svc.publish("ecv1/gw-01/comp/main/data/temp", msg())).resolves.toBeUndefined();
    await expect(svc.subscribe(reserved, () => undefined)).resolves.toBeUndefined();
    expect(provider.published.map((p) => p.topic)).toEqual(["ecv1/gw-01/comp/main/data/temp"]);
  });

  it("setGuardIncludeRoot(true) extends the check to position 5 (D-U24/D-U27)", async () => {
    const svc = new DefaultMessagingService(new FakeMessagingProvider());
    const rootedReserved = "ecv1/dallas/gw-01/comp/main/state";
    const msg = MessageBuilder.create("x", "1").build();
    // Pre-bind (default false): position 5 is unchecked.
    await expect(svc.publish(rootedReserved, msg)).resolves.toBeUndefined();
    svc.setGuardIncludeRoot(true);
    await expect(svc.publish(rootedReserved, msg)).rejects.toBeInstanceOf(ReservedTopicError);
    // A legit rooted app channel whose channel token is a reserved word still passes
    // (positions 4='main' and 5='app' are both non-reserved).
    await expect(svc.publish("ecv1/dallas/gw-01/comp/main/app/state", msg)).resolves.toBeUndefined();
  });

  it("the privileged seam bypasses the guard (§4.2) and publishReservedVia falls back on fakes", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    const reserved = "ecv1/gw-01/comp/main/state";
    const msg = MessageBuilder.create("state", "1.0").withPayload({ status: "RUNNING" }).build();
    await svc.publishReserved(reserved, msg);
    await svc.publishReservedRaw(reserved, { raw: 1 });
    await svc.publishReservedNorthbound(reserved, msg);
    expect(provider.published).toHaveLength(3);

    // publishReservedVia prefers the seam...
    await publishReservedVia(svc, reserved, msg);
    expect(provider.published).toHaveLength(4);
    await publishReservedVia(svc, reserved, msg, "northbound");
    expect(provider.published).toHaveLength(5);

    // ...and falls back to the public path for services without one (they carry no guard).
    const bare = new RecordingMessagingService();
    // Shadow the seam methods to simulate a plain IMessagingService implementation.
    (bare as unknown as Record<string, unknown>).publishReserved = undefined;
    (bare as unknown as Record<string, unknown>).publishReservedNorthbound = undefined;
    await publishReservedVia(bare, reserved, msg);
    await publishReservedVia(bare, reserved, msg, "northbound");
    expect(bare.published.map((p) => p.kind)).toEqual(["publish", "publishNorthbound"]);
  });
});

describe("request() default deadline (§5 / D-U5)", () => {
  it("undefined timeout resolves to the bound default; explicit 0 disables", async () => {
    const svc = new DefaultMessagingService(new FakeMessagingProvider());
    expect(svc.getDefaultRequestTimeout()).toBe(30_000); // built-in pre-bind default
    svc.setDefaultRequestTimeout(20);
    expect(svc.getDefaultRequestTimeout()).toBe(20);
    // No responder: the default (20 ms) deadline fires with RequestTimeoutError.
    const fut = svc.request("no/responder", MessageBuilder.create("q", "1").build());
    await expect(fut).rejects.toBeInstanceOf(RequestTimeoutError);

    // Explicit 0 disables the deadline: nothing settles the future.
    const fut0 = svc.request("no/responder", MessageBuilder.create("q", "1").build(), 0);
    let settled = false;
    void fut0.then(
      () => (settled = true),
      () => (settled = true),
    );
    await tick(60);
    expect(settled).toBe(false);
    svc.cancelRequest(fut0);
    await expect(fut0).rejects.toThrow(/canceled/);
  });

  it("the deadline cleans up the ephemeral reply subscription", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    svc.setDefaultRequestTimeout(10);
    const fut = svc.request("no/responder", MessageBuilder.create("q", "1").build());
    await tick(); // let the reply subscription register
    expect(provider.subs.length).toBe(1);
    await expect(fut).rejects.toBeInstanceOf(RequestTimeoutError);
    await tick();
    expect(provider.subs.length).toBe(0);
  });

  it("an arrived reply wins over a later deadline (single settle path)", async () => {
    const provider = new FakeMessagingProvider();
    const svc = new DefaultMessagingService(provider);
    await svc.subscribe("rpc/echo", async (_t, req) => {
      await svc.reply(req, MessageBuilder.create("r", "1").withPayload({ ok: 1 }).build());
    });
    const reply = await svc.request("rpc/echo", MessageBuilder.create("q", "1").withPayload({}).build(), 5_000);
    expect(reply.getBody()).toEqual({ ok: 1 });
  });
});
