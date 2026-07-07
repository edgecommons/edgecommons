/**
 * Cross-language interop node (TypeScript) for edgecommons. See python_node.py for
 * the shared CLI contract. Local-only MQTT transport against localhost:1883, using the
 * public edgecommons API (StandaloneMqttProvider + DefaultMessagingService), exactly
 * like the rust_node/java_node/python_node consume their libraries.
 *
 *   interop_node responder <request_topic>
 *   interop_node request   <request_topic> <token>
 *   interop_node raw-sub   <topic> <token>
 *   interop_node raw-pub   <topic> <token>
 *   interop_node uns-pub   <identityJson> <class> [channel]
 *   interop_node uns-sub   <topic>
 *   interop_node uns-guard
 *
 * Messages are built without a config — the envelope legally omits `identity` unless
 * one is stamped explicitly (the UNS roles); `tags.thing` no longer exists (UNS hard cut).
 */
import {
  Message,
  MessageBuilder,
  MessageBodyCase,
  MessageIdentity,
  DefaultMessagingService,
  IpcMessagingProvider,
  StandaloneMqttProvider,
  ReservedTopicError,
  Uns,
  unsClassFromToken,
} from "../../../../libs/ts/dist/index";
import type { MessagingConfig } from "../../../../libs/ts/dist/index";
import { existsSync, writeFileSync } from "fs";

const LANG = "ts";
const HOST = process.env.EDGECOMMONS_IT_MQTT_HOST ?? "localhost";
const PORT = Number(process.env.EDGECOMMONS_IT_MQTT_PORT ?? "1883");

// Canonical cross-language payload permutations (echoed by the responder; test_interop asserts a
// deep round-trip both ways). null is tested inside an array.
const TYPES = {
  b: true,
  bf: false,
  i: 42,
  ni: -7,
  fl: 3.5,
  slash: "a/b",
  quote: 'x"y',
  arr: [1, "two", false, null],
  nullv: null,
  nested: { k: [1, { d: 2 }] },
  ea: [],
  eo: {},
};

async function service(suffix: string): Promise<DefaultMessagingService> {
  const mc: MessagingConfig = {
    local: { host: HOST, port: PORT, clientId: `interop-${LANG}-${suffix}-${process.pid}` },
  };
  const provider = await StandaloneMqttProvider.connect(mc);
  return new DefaultMessagingService(provider);
}

async function ipcService(): Promise<DefaultMessagingService> {
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages: true });
  return new DefaultMessagingService(provider);
}

function emit(obj: unknown): void {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

async function runResponder(topic: string): Promise<never> {
  const svc = await service("resp");
  await svc.subscribe(topic, (_t, request) => {
    const reply = MessageBuilder.create("InteropReply", "1.0")
      .withPayload({ echo: request.getBody(), responder: LANG })
      .withTags({})
      .build();
    void svc.reply(request, reply);
  });
  process.stdout.write("READY\n");
  return new Promise<never>(() => {});
}

async function runRequest(topic: string, token: string): Promise<number> {
  const svc = await service("req");
  try {
    const req = MessageBuilder.create("InteropRequest", "1.0")
      .withPayload({ token, from: LANG, types: TYPES })
      .withTags({})
      .build();
    const corr = req.getCorrelationId();
    let reply: Message;
    try {
      reply = await svc.request(topic, req, 8000);
    } catch {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const body = reply.getBody() as Record<string, unknown> | null;
    const match = reply.getCorrelationId() === corr;
    emit({ ok: true, correlation_match: match, reply_body: body });
    const echo = body && (body.echo as Record<string, unknown> | undefined);
    const ok = match && !!body && !!body.responder && !!echo && echo.token === token;
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runRawSub(topic: string, token: string): Promise<number> {
  const svc = await service("rawsub");
  try {
    const got = new Promise<Message>((resolve) => {
      void svc.subscribe(topic, (_t, m) => resolve(m)).then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 10_000));
    const m = await Promise.race([got, timeout]);
    if (m === null) {
      emit({ ok: true, delivered: false, error: "timeout" });
      return 0;
    }
    emit({
      ok: false,
      delivered: true,
      raw: m.getRaw(),
      body: m.getBody(),
      expected_token: token,
    });
    return 1;
  } finally {
    await svc.disconnect();
  }
}

async function runRawPub(topic: string, token: string): Promise<number> {
  const svc = await service("rawpub");
  try {
    await svc.publishRaw(topic, { token, from: LANG });
    await new Promise((r) => setTimeout(r, 500));
    return 0;
  } finally {
    await svc.disconnect();
  }
}

async function runBinarySub(topic: string, expectedHex: string): Promise<number> {
  const svc = await service("binsub");
  try {
    const got = new Promise<Message>((resolve) => {
      void svc.subscribe(topic, (_t, m) => resolve(m)).then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 10_000));
    const m = await Promise.race([got, timeout]);
    if (m === null) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    let hex: string | null = null;
    let error: string | undefined;
    const isBinary = m.isBinaryBody();
    try {
      hex = m.getBinaryBody()?.toString("hex") ?? null;
    } catch (e) {
      error = String(e);
    }
    const ok = isBinary && hex === expectedHex.toLowerCase();
    emit({ ok, is_binary: isBinary, hex, ...(error ? { error } : {}) });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runBinaryPub(topic: string, bodyHex: string): Promise<number> {
  const svc = await service("binpub");
  try {
    const bytes = Buffer.from(bodyHex, "hex");
    const msg = MessageBuilder.create("InteropBinary", "1.0")
      .withPayload(bytes)
      .withTags({ from: LANG })
      .build();
    await svc.publish(topic, msg);
    await new Promise((r) => setTimeout(r, 500));
    return 0;
  } finally {
    await svc.disconnect();
  }
}

function typedBody(bodyHex: string): Record<string, unknown> {
  const bytes = Buffer.from(bodyHex, "hex");
  return {
    signal: { id: "camera-1/roi-17/thumbnail", name: "Thumbnail" },
    samples: [{
      value: {
        _edgecommonsBinary: {
          encoding: "base64",
          length: bytes.length,
          data: bytes.toString("base64"),
        },
      },
      quality: "GOOD",
      sourceTsMs: 1783360799900,
      serverTsMs: 1783360800000,
    }],
  };
}

async function runTypedSub(topic: string, expectedHex: string): Promise<number> {
  const svc = await service("typedsub");
  try {
    const got = new Promise<Message>((resolve) => {
      void svc.subscribe(topic, (_t, m) => resolve(m)).then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 10_000));
    const m = await Promise.race([got, timeout]);
    if (m === null) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const body = m.getBody() as { samples?: Array<Record<string, any>> };
    const sample = body.samples?.[0] ?? {};
    const marker = sample.value?._edgecommonsBinary;
    const hex = marker?.data ? Buffer.from(marker.data, "base64").toString("hex") : null;
    const result = {
      body_case: m.getBodyCase(),
      hex,
      source_ts_ms: sample.sourceTsMs,
      server_ts_ms: sample.serverTsMs,
      tag_from: (m.tags as Record<string, unknown> | undefined)?.from,
    };
    const ok = result.body_case === MessageBodyCase.SouthboundSignalUpdate
      && result.hex === expectedHex.toLowerCase()
      && result.source_ts_ms === 1783360799900
      && result.server_ts_ms === 1783360800000;
    emit({ ...result, ok });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runTypedPub(topic: string, bodyHex: string): Promise<number> {
  const svc = await service("typedpub");
  try {
    const msg = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
      .withSouthboundSignalUpdate(typedBody(bodyHex))
      .withTags({ from: LANG })
      .build();
    await svc.publish(topic, msg);
    await new Promise((r) => setTimeout(r, 500));
    return 0;
  } finally {
    await svc.disconnect();
  }
}

function ggTopic(runId: string, publisher: string, subscriber: string): string {
  return `edgecommons/interop/binary/${runId}/${publisher}/${subscriber}`;
}

function ggTypedTopic(runId: string, publisher: string, subscriber: string): string {
  return `edgecommons/interop/typed/${runId}/${publisher}/${subscriber}`;
}

function publisherFromGgTopic(topic: string): string {
  const parts = topic.split("/");
  return parts.length >= 2 ? parts[parts.length - 2] : "unknown";
}

function ggReadyPath(runId: string, lang: string): string {
  return `/tmp/edgecommons_gg_ipc_binary_ready_${lang}_${runId}`;
}

async function waitForGgReady(runId: string, expectedLangs: string[]): Promise<string[]> {
  const readyWaitSecs = Number(process.env.EDGECOMMONS_GG_READY_WAIT_SECS ?? "180");
  const deadline = Date.now() + readyWaitSecs * 1000;
  while (Date.now() < deadline) {
    const missing = expectedLangs.filter((lang) => !existsSync(ggReadyPath(runId, lang)));
    if (missing.length === 0) return [];
    await new Promise((r) => setTimeout(r, 200));
  }
  return expectedLangs.filter((lang) => !existsSync(ggReadyPath(runId, lang)));
}

async function runGgBinaryMatrix(runId: string, langsCsv: string, expectedHex: string): Promise<number> {
  const expectedLangs = langsCsv.split(",").filter(Boolean);
  const readyLangs = (process.env.EDGECOMMONS_GG_READY_LANGS ?? langsCsv).split(",").filter(Boolean);
  const readyLang = process.env.EDGECOMMONS_GG_READY_LANG ?? LANG;
  const expectedBytes = Buffer.from(expectedHex, "hex");
  const subscribeDelaySecs = Number(process.env.EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS ?? "8");
  const waitSecs = Number(process.env.EDGECOMMONS_GG_WAIT_SECS ?? "35");
  const svc = await ipcService();
  const received = new Map<string, { is_binary: boolean; hex: string | null; ok: boolean }>();
  const receivedTyped = new Map<string, {
    body_case: MessageBodyCase | null;
    hex: string | null;
    source_ts_ms?: unknown;
    server_ts_ms?: unknown;
    tag_from?: unknown;
    ok: boolean;
  }>();
  const errors = new Map<string, string>();
  try {
    await svc.subscribe(
      ggTopic(runId, "+", LANG),
      (_topic, m) => {
        const publisher = publisherFromGgTopic(_topic);
        try {
          const isBinary = m.isBinaryBody();
          const bytes = isBinary ? m.getBinaryBody() : undefined;
          const hex = bytes?.toString("hex") ?? null;
          const ok = isBinary && bytes !== undefined && Buffer.compare(bytes, expectedBytes) === 0;
          if (!received.has(publisher)) received.set(publisher, { is_binary: isBinary, hex, ok });
        } catch (e) {
          errors.set(`${publisher}:binary`, String(e));
          if (!received.has(publisher)) received.set(publisher, { is_binary: false, hex: null, ok: false });
        }
      },
      64,
      1,
    );
    await svc.subscribe(
      ggTypedTopic(runId, "+", LANG),
      (_topic, m) => {
        const publisher = publisherFromGgTopic(_topic);
        try {
          const body = m.getBody() as { samples?: Array<Record<string, any>> };
          const sample = body.samples?.[0] ?? {};
          const marker = sample.value?._edgecommonsBinary;
          const bytes = marker?.data ? Buffer.from(marker.data, "base64") : undefined;
          const hex = bytes?.toString("hex") ?? null;
          const tagFrom = (m.tags as Record<string, unknown> | undefined)?.from;
          const item = {
            body_case: m.getBodyCase(),
            hex,
            source_ts_ms: sample.sourceTsMs,
            server_ts_ms: sample.serverTsMs,
            tag_from: tagFrom,
            ok: m.getBodyCase() === MessageBodyCase.SouthboundSignalUpdate
              && hex === expectedHex.toLowerCase()
              && sample.sourceTsMs === 1783360799900
              && sample.serverTsMs === 1783360800000
              && tagFrom === publisher,
          };
          if (!receivedTyped.has(publisher)) receivedTyped.set(publisher, item);
        } catch (e) {
          errors.set(`${publisher}:typed`, String(e));
          if (!receivedTyped.has(publisher)) {
            receivedTyped.set(publisher, { body_case: null, hex: null, ok: false });
          }
        }
      },
      64,
      1,
    );
    process.stdout.write("READY\n");
    writeFileSync(ggReadyPath(runId, readyLang), String(Date.now()), "utf8");
    const readyMissing = await waitForGgReady(runId, readyLangs);
    await new Promise((r) => setTimeout(r, subscribeDelaySecs * 1000));
    if (readyMissing.length === 0) {
      const msg = MessageBuilder.create("InteropBinary", "1.0")
        .withPayload(expectedBytes)
        .withTags({ from: LANG })
        .build();
      const typedMsg = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
        .withSouthboundSignalUpdate(typedBody(expectedHex))
        .withTags({ from: LANG })
        .build();
      for (const target of expectedLangs) {
        await svc.publish(ggTopic(runId, LANG, target), msg);
        await svc.publish(ggTypedTopic(runId, LANG, target), typedMsg);
      }
    }
    const deadline = Date.now() + waitSecs * 1000;
    while (
      Date.now() < deadline
      && !expectedLangs.every((lang) => received.has(lang) && receivedTyped.has(lang))
    ) {
      await new Promise((r) => setTimeout(r, 100));
    }
    const missing = expectedLangs.filter((lang) => !received.has(lang));
    const missingTyped = expectedLangs.filter((lang) => !receivedTyped.has(lang));
    const receivedObj = Object.fromEntries(received.entries());
    const receivedTypedObj = Object.fromEntries(receivedTyped.entries());
    const errorsObj = Object.fromEntries(errors.entries());
    const ok =
      readyMissing.length === 0 &&
      missing.length === 0 &&
      missingTyped.length === 0 &&
      errors.size === 0 &&
      expectedLangs.every((lang) => received.get(lang)?.ok === true && receivedTyped.get(lang)?.ok === true);
    const result = {
      ok,
      lang: LANG,
      run_id: runId,
      expected_hex: expectedHex.toLowerCase(),
      ready_missing: readyMissing,
      received: receivedObj,
      received_typed: receivedTypedObj,
      missing,
      missing_typed: missingTyped,
      errors: errorsObj,
    };
    writeFileSync(`/tmp/edgecommons_gg_ipc_binary_${LANG}_${runId}.json`, JSON.stringify(result), "utf8");
    emit(result);
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

/**
 * uns-pub <identityJson> <class> [channel] — mint the topic with the real Uns builder
 * (includeRoot=false), stamp the identity via the real MessageBuilder, publish, and
 * print {"ok":true,"topic":...,"envelope":...}.
 */
async function runUnsPub(identityJson: string, clsToken: string, channel?: string): Promise<number> {
  const identity = MessageIdentity.fromObject(JSON.parse(identityJson));
  if (!identity) {
    emit({ ok: false, error: `bad identity: ${identityJson}` });
    return 2;
  }
  const cls = unsClassFromToken(clsToken);
  if (cls === undefined) {
    emit({ ok: false, error: `bad class: ${clsToken}` });
    return 2;
  }
  const topic = new Uns(identity, false).topic(cls, channel);
  const svc = await service("unspub");
  try {
    const msg = MessageBuilder.create("UnsInterop", "1.0")
      .withPayload({ from: LANG })
      .withIdentity(identity)
      .build();
    await svc.publish(topic, msg);
    await new Promise((r) => setTimeout(r, 500));
    emit({ ok: true, topic, envelope: msg.toObject() });
    return 0;
  } finally {
    await svc.disconnect();
  }
}

/** uns-sub <topic> — receive one envelope and print its parsed identity. */
async function runUnsSub(topic: string): Promise<number> {
  const svc = await service("unssub");
  try {
    const got = new Promise<Message>((resolve) => {
      void svc.subscribe(topic, (_t, m) => resolve(m)).then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 10_000));
    const m = await Promise.race([got, timeout]);
    if (m === null) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const identity = m.getIdentity();
    const ok = identity !== undefined;
    emit({ ok, identity: identity ? identity.toObject() : null, body: m.getBody() });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

/**
 * uns-guard — attempt a raw publish to a reserved-class topic through the guarded
 * public service; must fail with ReservedTopicError (§4.1).
 */
async function runUnsGuard(): Promise<number> {
  const svc = await service("guard");
  try {
    const topic = "ecv1/dev1/comp1/main/state";
    try {
      await svc.publishRaw(topic, { from: LANG });
    } catch (e) {
      if (e instanceof ReservedTopicError) {
        emit({ error: "ReservedTopicError", class: e.classToken, topic: e.topic });
        return 3;
      }
      emit({ error: String(e) });
      return 4;
    }
    emit({ ok: true });
    return 0;
  } finally {
    await svc.disconnect();
  }
}

async function main(): Promise<void> {
  const [role, a, b, c] = process.argv.slice(2);
  switch (role) {
    case "responder":
      await runResponder(a);
      return;
    case "request":
      process.exit(await runRequest(a, b));
    case "raw-sub":
      process.exit(await runRawSub(a, b));
    case "raw-pub":
      process.exit(await runRawPub(a, b));
    case "binary-sub":
      process.exit(await runBinarySub(a, b));
    case "binary-pub":
      process.exit(await runBinaryPub(a, b));
    case "typed-sub":
      process.exit(await runTypedSub(a, b));
    case "typed-pub":
      process.exit(await runTypedPub(a, b));
    case "gg-binary-matrix":
      process.exit(await runGgBinaryMatrix(a, b, c));
    case "uns-pub":
      process.exit(await runUnsPub(a, b, c));
    case "uns-sub":
      process.exit(await runUnsSub(a));
    case "uns-guard":
      process.exit(await runUnsGuard());
    default:
      process.stderr.write(`unknown role: ${role}\n`);
      process.exit(2);
  }
}

void main();
