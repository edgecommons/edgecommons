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
 *   interop_node status-responder    <component>
 *   interop_node status-request      <component>
 *   interop_node state-instances-pub <component>
 *   interop_node state-instances-sub <component>
 *   interop_node describe-responder  <component>
 *   interop_node describe-requester  <component>
 *   interop_node gg-scope-matrix     <runId> <langsCsv>
 *
 * Messages are built without a config — the envelope legally omits `identity` unless
 * one is stamped explicitly (the UNS roles); `tags.thing` no longer exists (UNS hard cut).
 */
import {
  Message,
  MessageBuilder,
  MessageBodyCase,
  MessageIdentity,
  CommandOutcomes,
  CommandScopes,
  DefaultMessagingService,
  IpcMessagingProvider,
  StandaloneMqttProvider,
  ReservedTopicError,
  Uns,
  unsClassFromToken,
  EdgeCommonsBuilder,
  InstanceConnectivity,
  Qos,
} from "../../../../libs/ts/dist/index";
import type { MessagingConfig } from "../../../../libs/ts/dist/index";
import { closeSync, existsSync, fsyncSync, openSync, unlinkSync, writeFileSync, writeSync } from "fs";
import { randomUUID } from "crypto";
import { tmpdir } from "os";
import { join } from "path";

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

function logComponentToken(): string {
  return `interop-log-${LANG}`;
}

function writeCommandRuntimeConfig(componentToken: string, heartbeatEnabled = false): string {
  const path = join(tmpdir(), `edgecommons-deferred-${LANG}-${process.pid}-${Date.now()}.json`);
  writeFileSync(
    path,
    JSON.stringify({
      component: { token: componentToken },
      messaging: {
        local: {
          type: "mqtt",
          host: HOST,
          port: PORT,
          clientId: `interop-${LANG}-deferred-runtime-${process.pid}`,
        },
        requestTimeoutSeconds: 4,
      },
      heartbeat: { enabled: heartbeatEnabled, intervalSecs: 5, destination: "local" },
      health: { enabled: false },
    }),
    "utf8",
  );
  return path;
}

function writeDurableAcceptanceMarker(): string {
  const marker = join(
    tmpdir(),
    `edgecommons-p1-accept-${LANG}-${process.pid}-${randomUUID()}.marker`,
  );
  const descriptor = openSync(marker, "wx", 0o600);
  try {
    writeSync(descriptor, "accepted\n", undefined, "utf8");
    fsyncSync(descriptor);
  } catch (error) {
    try {
      unlinkSync(marker);
    } catch {
      // The original persistence error remains authoritative.
    }
    throw error;
  } finally {
    closeSync(descriptor);
  }
  return marker;
}

function removeDurableAcceptanceMarker(marker: string): void {
  try {
    unlinkSync(marker);
  } catch {
    // Cleanup is best effort after the terminal response has been attempted.
  }
}

function writeLogRuntimeConfig(): string {
  const path = join(tmpdir(), `edgecommons-log-${LANG}-${process.pid}-${Date.now()}.json`);
  writeFileSync(
    path,
    JSON.stringify({
      component: { token: logComponentToken() },
      messaging: {
        local: {
          type: "mqtt",
          host: HOST,
          port: PORT,
          clientId: `interop-${LANG}-log-runtime-${process.pid}`,
        },
        requestTimeoutSeconds: 2,
      },
      heartbeat: { enabled: false },
      health: { enabled: false },
      logging: {
        level: "WARN",
        publish: {
          enabled: true,
          destination: "local",
          minLevel: "TRACE",
          captureNative: false,
          captureConsole: false,
          redaction: { enabled: false },
        },
      },
    }),
    "utf8",
  );
  return path;
}

function logRuntimeArgs(path: string): string[] {
  return [
    "--platform",
    "HOST",
    "--transport",
    "MQTT",
    path,
    "-c",
    "FILE",
    path,
    "-t",
    "interop-device",
  ];
}

function wireIdentityDevice(identity: Record<string, unknown> | undefined): unknown {
  const hier = identity?.hier;
  return Array.isArray(hier) && hier.length > 0
    ? (hier[hier.length - 1] as Record<string, unknown>).value
    : undefined;
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

async function runDeferredResponder(componentToken: string): Promise<never> {
  const path = writeCommandRuntimeConfig(componentToken);
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  try {
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.DeferredResponder`)
      .args(logRuntimeArgs(path))
      .configureCommands((inbox) => {
        inbox.registerOutcome("deferred", CommandScopes.Both, (request, _addressedInstance) => {
          const token = inbox.defer(request, 4_000);
          let acceptanceMarker: string;
          try {
            acceptanceMarker = writeDurableAcceptanceMarker();
          } catch {
            token.discard();
            return CommandOutcomes.error("ACCEPTANCE_FAILED", "work was not accepted");
          }
          if (!token.activate()) {
            removeDurableAcceptanceMarker(acceptanceMarker);
            return CommandOutcomes.error("ACTIVATION_FAILED", "deferred token was not open");
          }
          return CommandOutcomes.deferredWithContinuation(token, () => {
            try {
              token.settleSuccess({
                token: (request.getBody() as Record<string, unknown>).token,
                responder: LANG,
                durablyAccepted: true,
              });
            } finally {
              removeDurableAcceptanceMarker(acceptanceMarker);
            }
          });
        });
      })
      .build();
    process.stdout.write("READY\n");
    // A pending Promise alone does not keep Node's event loop alive. Retain a bounded-footprint
    // timer so the real runtime remains subscribed until the harness terminates this role.
    await new Promise<never>(() => {
      setInterval(() => undefined, 3_600_000);
    });
  } finally {
    if (gg) await gg.close();
    try {
      unlinkSync(path);
    } catch {
      // best effort after a failed startup
    }
  }
  throw new Error("unreachable deferred responder completion");
}

async function runDeferredRequest(topic: string, token: string): Promise<number> {
  const svc = await service("deferredreq");
  const replyTopic = `interop/deferred/reply/${LANG}/${process.pid}-${Date.now()}`;
  const replies: Message[] = [];
  try {
    await svc.subscribe(replyTopic, (_topic, reply) => {
      replies.push(reply);
    });
    const request = MessageBuilder.create("deferred", "1.0")
      .withCommand({ token, from: LANG })
      .withReplyTo(replyTopic)
      .withTags({})
      .build();
    const correlation = request.getCorrelationId();
    await svc.publish(topic, request);
    const deadline = Date.now() + 8_000;
    while (replies.length === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    if (replies.length === 0) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    // Keep receiving after the first response, making a duplicate settlement observable.
    await new Promise((resolve) => setTimeout(resolve, 750));
    const reply = replies[0];
    const body = reply.getBody() as Record<string, unknown>;
    const result = body.result as Record<string, unknown> | undefined;
    const correlationMatch = reply.getCorrelationId() === correlation;
    const ok = replies.length === 1
      && correlationMatch
      && body.ok === true
      && result?.token === token
      && result?.durablyAccepted === true
      && typeof result?.responder === "string";
    emit({ ok, reply_count: replies.length, correlation_match: correlationMatch, reply_body: body });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runConfirmedSub(topic: string, token: string): Promise<number> {
  const svc = await service("confirmedsub");
  const messages: Message[] = [];
  try {
    await svc.subscribe(topic, (_topic, message) => {
      messages.push(message);
    });
    process.stdout.write("READY\n");
    const deadline = Date.now() + 8_000;
    while (messages.length === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    if (messages.length === 0) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    await new Promise((resolve) => setTimeout(resolve, 750));
    const body = messages[0].getBody() as Record<string, unknown>;
    const ok = messages.length === 1 && body.token === token && typeof body.from === "string";
    emit({ ok, message_count: messages.length, body });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runConfirmedPub(topic: string, token: string): Promise<number> {
  const svc = await service("confirmedpub");
  try {
    const message = MessageBuilder.create("InteropConfirmed", "1.0")
      .withPayload({ token, from: LANG })
      .withTags({})
      .build();
    // The strict standalone path resolves only after its QoS1 PUBACK callback fires.
    await svc.publishConfirmed(topic, message, Qos.AtLeastOnce, 5_000);
    emit({ ok: true, confirmed: true, qos: 1 });
    return 0;
  } catch (e) {
    emit({ ok: false, error: String(e) });
    return 1;
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

async function runLogSub(topic: string, token: string): Promise<number> {
  const svc = await service("logsub");
  try {
    const got = new Promise<{ topic: string; message: Message }>((resolve) => {
      void svc.subscribe(topic, (t, m) => resolve({ topic: t, message: m }))
        .then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 10_000));
    const received = await Promise.race([got, timeout]);
    if (received === null) {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const envelope = received.message.toObject() as Record<string, any>;
    const header = envelope.header as Record<string, unknown> | undefined;
    const identity = envelope.identity as Record<string, unknown> | undefined;
    const body = received.message.getBody() as Record<string, any>;
    const fields = (body.fields ?? {}) as Record<string, unknown>;
    const ok = received.topic === topic
      && body.schema === "edgecommons.log.v1"
      && body.level === "WARN"
      && body.message === `log-interop-${token}`
      && fields.nonce === token
      && wireIdentityDevice(identity) === "interop-device"
      && typeof identity?.component === "string"
      && identity.component.startsWith("interop-log-")
      // Component scope (D-U28): the wire identity omits `instance`.
      && identity?.instance === undefined
      && header?.name === "log"
      && header?.version === "1.0";
    emit({ ok, topic: received.topic, header, identity, body });
    return ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
}

async function runLogPub(token: string): Promise<number> {
  const path = writeLogRuntimeConfig();
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  try {
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.LogPublisher`)
      .args(logRuntimeArgs(path))
      .build();
    await gg.logs().publish({
      level: "WARN",
      logger: `interop.${LANG}`,
      message: `log-interop-${token}`,
      fields: { nonce: token, publisher: LANG },
    });
    await gg.logs().flush();
    const stats = gg.logs().stats();
    const ok = stats.published >= 1;
    emit({ ok, component: logComponentToken(), stats });
    return ok ? 0 : 1;
  } finally {
    if (gg) await gg.close();
    try {
      unlinkSync(path);
    } catch {
      // best effort
    }
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

function ggLogReadyPath(runId: string, lang: string): string {
  return `/tmp/edgecommons_gg_ipc_log_ready_${lang}_${runId}`;
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

async function waitForGgLogReady(runId: string, expectedLangs: string[]): Promise<string[]> {
  const readyWaitSecs = Number(process.env.EDGECOMMONS_GG_READY_WAIT_SECS ?? "180");
  const deadline = Date.now() + readyWaitSecs * 1000;
  while (Date.now() < deadline) {
    const missing = expectedLangs.filter((lang) => !existsSync(ggLogReadyPath(runId, lang)));
    if (missing.length === 0) return [];
    await new Promise((r) => setTimeout(r, 200));
  }
  return expectedLangs.filter((lang) => !existsSync(ggLogReadyPath(runId, lang)));
}

function ggLogRuntimeArgs(path: string): string[] {
  return [
    "--platform",
    "GREENGRASS",
    "--transport",
    "IPC",
    "-c",
    "FILE",
    path,
    "-t",
    "interop-device",
  ];
}

function ggP1ReadyPath(runId: string, actor: string): string {
  return `/tmp/edgecommons_gg_ipc_p1_ready_${actor}_${runId}`;
}

async function waitForGgP1Ready(runId: string, expectedActors: string[]): Promise<string[]> {
  const readyWaitSecs = Number(process.env.EDGECOMMONS_GG_READY_WAIT_SECS ?? "180");
  const deadline = Date.now() + readyWaitSecs * 1000;
  while (Date.now() < deadline) {
    const missing = expectedActors.filter((actor) => !existsSync(ggP1ReadyPath(runId, actor)));
    if (missing.length === 0) return [];
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return expectedActors.filter((actor) => !existsSync(ggP1ReadyPath(runId, actor)));
}

function ggP1TargetActor(targetLanguage: string, senderActor: string): string {
  return targetLanguage === "rust" && senderActor === "rust" ? "rustpeer" : targetLanguage;
}

function ggP1CommandTopic(actor: string): string {
  return `ecv1/interop-device/interop-p1-${actor}/cmd/deferred`;
}

function ggP1ConfirmedTopic(runId: string, publisher: string, targetActor: string): string {
  return `edgecommons/interop/p1/${runId}/confirmed/${publisher}/${targetActor}`;
}

async function sendGgP1Deferred(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetLanguage: string,
  targetActor: string,
): Promise<Record<string, unknown>> {
  const token = `${runId}:${senderActor}->${targetLanguage}`;
  const replyTopic = `edgecommons/interop/p1/${runId}/reply/${senderActor}/${targetActor}/${process.pid}-${Date.now()}-${Math.random()}`;
  const replies: Message[] = [];
  await svc.subscribe(replyTopic, (_topic, reply) => {
    replies.push(reply);
  }, 2, 1);
  const request = MessageBuilder.create("deferred", "1.0")
    .withCommand({ token, from: LANG, actor: senderActor })
    .withReplyTo(replyTopic)
    .withTags({})
    .build();
  const correlation = request.getCorrelationId();
  await svc.publish(ggP1CommandTopic(targetActor), request);
  const deadline = Date.now() + 8_000;
  while (replies.length === 0 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (replies.length === 0) return { ok: false, target_actor: targetActor, error: "timeout" };
  await new Promise((resolve) => setTimeout(resolve, 750));
  const reply = replies[0];
  const body = reply.getBody() as Record<string, unknown>;
  const result = body.result as Record<string, unknown> | undefined;
  const correlationMatch = reply.getCorrelationId() === correlation;
  const ok = replies.length === 1
    && correlationMatch
    && body.ok === true
    && result?.token === token
    && result?.durablyAccepted === true
    && result?.responder === targetLanguage
    && result?.responderActor === targetActor;
  return {
    ok,
    target_actor: targetActor,
    expected_token: token,
    expected_responder: targetLanguage,
    expected_responder_actor: targetActor,
    reply_count: replies.length,
    correlation_match: correlationMatch,
    duplicate_window_ms: 750,
    reply_body: body,
  };
}

async function runGgP1Matrix(runId: string, langsCsv: string): Promise<number> {
  const languages = langsCsv.split(",").filter(Boolean);
  const expectedActors = (process.env.EDGECOMMONS_GG_READY_LANGS ?? langsCsv).split(",").filter(Boolean);
  const actor = process.env.EDGECOMMONS_GG_READY_LANG ?? LANG;
  const canonicalActor = actor !== "rustpeer";
  const subscribeDelaySecs = Number(process.env.EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS ?? "2");
  const waitSecs = Number(process.env.EDGECOMMONS_GG_WAIT_SECS ?? "90");
  const expectedPublishers = actor === "rust"
    ? languages.filter((publisher) => publisher !== "rust")
    : canonicalActor ? languages : ["rust"];
  const svc = await ipcService();
  const received = new Map<string, Array<Record<string, unknown>>>();
  const errors = new Map<string, string>();
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  let path: string | undefined;
  try {
    path = writeCommandRuntimeConfig(`interop-p1-${actor}`);
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.P1Responder`)
      .args(ggLogRuntimeArgs(path))
      .configureCommands((inbox) => {
        inbox.registerOutcome("deferred", CommandScopes.Both, (request, _addressedInstance) => {
          const token = inbox.defer(request, 4_000);
          const requestBody = request.getBody() as Record<string, unknown>;
          let acceptanceMarker: string;
          try {
            acceptanceMarker = writeDurableAcceptanceMarker();
          } catch {
            token.discard();
            return CommandOutcomes.error("ACCEPTANCE_FAILED", "work was not accepted");
          }
          if (!token.activate()) {
            removeDurableAcceptanceMarker(acceptanceMarker);
            return CommandOutcomes.error("ACTIVATION_FAILED", "deferred token was not open");
          }
          return CommandOutcomes.deferredWithContinuation(token, () => {
            try {
              token.settleSuccess({
                token: requestBody.token,
                responder: LANG,
                responderActor: actor,
                durablyAccepted: true,
              });
            } finally {
              removeDurableAcceptanceMarker(acceptanceMarker);
            }
          });
        });
      })
      .build();
    await svc.subscribe(
      `edgecommons/interop/p1/${runId}/confirmed/+/${actor}`,
      (topic, message) => {
        const publisher = publisherFromGgTopic(topic);
        try {
          const body = message.getBody() as Record<string, unknown>;
          const valid = body.runId === runId
            && body.publisher === publisher
            && body.targetActor === actor
            && body.strict === true;
          const items = received.get(publisher) ?? [];
          items.push({ ok: valid, topic, body });
          received.set(publisher, items);
        } catch (error) {
          errors.set(`confirmed:${publisher}`, String(error));
        }
      },
      32,
      1,
    );
    process.stdout.write("READY\n");
    writeFileSync(ggP1ReadyPath(runId, actor), String(Date.now()), "utf8");
    const readyMissing = await waitForGgP1Ready(runId, expectedActors);
    const deferredRequests: Record<string, Record<string, unknown>> = {};
    const confirmedPublishes: Record<string, Record<string, unknown>> = {};
    if (readyMissing.length === 0 && canonicalActor) {
      await new Promise((resolve) => setTimeout(resolve, subscribeDelaySecs * 1000));
      for (const targetLanguage of languages) {
        const targetActor = ggP1TargetActor(targetLanguage, actor);
        try {
          deferredRequests[targetLanguage] = await sendGgP1Deferred(
            svc, runId, actor, targetLanguage, targetActor,
          );
        } catch (error) {
          deferredRequests[targetLanguage] = {
            ok: false, target_actor: targetActor, error: String(error),
          };
        }
        const message = MessageBuilder.create("InteropConfirmed", "1.0")
          .withPayload({
            runId,
            publisher: LANG,
            publisherActor: actor,
            targetLanguage,
            targetActor,
            strict: true,
          })
          .withTags({})
          .build();
        try {
          await svc.publishConfirmed(
            ggP1ConfirmedTopic(runId, LANG, targetActor), message, Qos.AtLeastOnce, 5_000,
          );
          confirmedPublishes[targetLanguage] = {
            ok: true, target_actor: targetActor, confirmed: true, qos: 1,
          };
        } catch (error) {
          confirmedPublishes[targetLanguage] = {
            ok: false, target_actor: targetActor, error: String(error),
          };
        }
      }
    }
    const deadline = Date.now() + waitSecs * 1000;
    while (Date.now() < deadline && !expectedPublishers.every((publisher) => received.has(publisher))) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    await new Promise((resolve) => setTimeout(resolve, 750));
    const confirmedReceived: Record<string, unknown> = {};
    const confirmedMissing = expectedPublishers.filter((publisher) => !received.has(publisher));
    let receiveOk = confirmedMissing.length === 0;
    for (const [publisher, items] of received) {
      const ok = items.length === 1 && items[0].ok === true && expectedPublishers.includes(publisher);
      confirmedReceived[publisher] = { count: items.length, items, ok };
      receiveOk = receiveOk && ok;
    }
    const requestsOk = !canonicalActor || (
      Object.keys(deferredRequests).length === languages.length
      && Object.values(deferredRequests).every((item) => item.ok === true)
    );
    const publishesOk = !canonicalActor || (
      Object.keys(confirmedPublishes).length === languages.length
      && Object.values(confirmedPublishes).every((item) => item.ok === true)
    );
    const ok = readyMissing.length === 0 && errors.size === 0 && requestsOk && publishesOk && receiveOk;
    const result = {
      schema: "edgecommons.gg-ipc-p1.v1",
      ok,
      run_id: runId,
      actor,
      language: LANG,
      canonical_actor: canonicalActor,
      ready_missing: readyMissing,
      deferred_requests: deferredRequests,
      confirmed_publishes: confirmedPublishes,
      confirmed_received: confirmedReceived,
      confirmed_missing: confirmedMissing,
      errors: Object.fromEntries(errors),
    };
    writeFileSync(`/tmp/edgecommons_gg_ipc_p1_${actor}_${runId}.json`, JSON.stringify(result), "utf8");
    emit(result);
    return ok ? 0 : 1;
  } finally {
    if (gg) await gg.close();
    if (path) {
      try {
        unlinkSync(path);
      } catch {
        // best effort after a failed runtime startup
      }
    }
    await svc.disconnect();
  }
}

// --- gg-scope-matrix: the 0.5.0 scoped-command wire contract over real Greengrass IPC -----------
//
// Every actor deploys the same component (`interop-scope-<actor>`) carrying exactly TWO custom
// verbs registered through the 0.5.0 `register(verb, scope, handler)` surface — one INSTANCE-scoped
// and one COMPONENT-scoped. Each canonical actor then probes EVERY language's component four ways:
// the `describe` manifest carries a `scope` on every entry, an instance-addressed topic reaches an
// INSTANCE-scoped verb without any `body.instance`, a topic/body instance conflict is refused, and
// a COMPONENT-scoped verb refuses instance addressing. The two rejections pin the BYTE-EXACT
// BAD_ARGS messages, so a reworded library message is a cross-language interop failure.

/** The INSTANCE-scoped custom verb every scope responder registers. */
const GG_SCOPE_PROBE_VERB = "sb/probe";

/** The COMPONENT-scoped custom verb every scope responder registers. */
const GG_SCOPE_DISCOVER_VERB = "sb/discover";

/** The pinned BAD_ARGS message for a topic/body instance conflict (probe 3). */
const GG_SCOPE_CONFLICT_MESSAGE = "instance in body conflicts with the addressed instance";

/** The pinned BAD_ARGS message for instance-addressing a COMPONENT-scoped verb (probe 4). */
const GG_SCOPE_COMPONENT_MESSAGE = `verb '${GG_SCOPE_DISCOVER_VERB}' is component-scoped`;

/** The four probe keys carried by every target entry of the result JSON. */
const GG_SCOPE_PROBE_KEYS = ["describe", "instance_routing", "conflict", "component_scope"] as const;

/**
 * The EXACT `describe` manifest entries the scope responder must advertise: the five built-ins
 * (all `both`) plus the two custom verbs at their declared scopes, verb-sorted, each entry carrying
 * EXACTLY {verb, builtIn, scope} — no `availability` key.
 */
const GG_SCOPE_EXPECTED_COMMANDS: ReadonlyArray<{ verb: string; builtIn: boolean; scope: string }> = [
  { verb: "describe", builtIn: true, scope: "both" },
  { verb: "get-configuration", builtIn: true, scope: "both" },
  { verb: "ping", builtIn: true, scope: "both" },
  { verb: "reload-config", builtIn: true, scope: "both" },
  { verb: GG_SCOPE_DISCOVER_VERB, builtIn: false, scope: "component" },
  { verb: GG_SCOPE_PROBE_VERB, builtIn: false, scope: "instance" },
  { verb: "status", builtIn: true, scope: "both" },
];

function ggScopeReadyPath(runId: string, actor: string): string {
  return `/tmp/edgecommons_gg_ipc_scope_ready_${actor}_${runId}`;
}

function ggScopeDonePath(runId: string, actor: string): string {
  return `/tmp/edgecommons_gg_ipc_scope_done_${actor}_${runId}`;
}

/** The addressed component's base UNS topic; the probes append the class/verb tail themselves. */
function ggScopeBaseTopic(actor: string): string {
  return `ecv1/interop-device/interop-scope-${actor}`;
}

async function waitForGgScopeReady(runId: string, expectedActors: string[]): Promise<string[]> {
  const readyWaitSecs = Number(process.env.EDGECOMMONS_GG_READY_WAIT_SECS ?? "180");
  const deadline = Date.now() + readyWaitSecs * 1000;
  while (Date.now() < deadline) {
    const missing = expectedActors.filter((actor) => !existsSync(ggScopeReadyPath(runId, actor)));
    if (missing.length === 0) return [];
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return expectedActors.filter((actor) => !existsSync(ggScopeReadyPath(runId, actor)));
}

/**
 * Holds this responder alive until every canonical actor has finished probing — a responder that
 * exits early would make the still-probing peers time out against a dead component.
 */
async function waitForGgScopeDone(runId: string, expectedActors: string[]): Promise<string[]> {
  const waitSecs = Number(process.env.EDGECOMMONS_GG_WAIT_SECS ?? "90");
  const deadline = Date.now() + waitSecs * 1000;
  while (Date.now() < deadline) {
    const missing = expectedActors.filter((actor) => !existsSync(ggScopeDonePath(runId, actor)));
    if (missing.length === 0) return [];
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return expectedActors.filter((actor) => !existsSync(ggScopeDonePath(runId, actor)));
}

/** The observed manifest must match the pinned entries exactly — order, values, and key set. */
function ggScopeCommandsMatch(observed: unknown): boolean {
  if (!Array.isArray(observed) || observed.length !== GG_SCOPE_EXPECTED_COMMANDS.length) return false;
  return GG_SCOPE_EXPECTED_COMMANDS.every((expected, index) => {
    const entry = observed[index] as Record<string, unknown> | null;
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return false;
    const keys = Object.keys(entry).sort();
    return keys.length === 3
      && keys[0] === "builtIn"
      && keys[1] === "scope"
      && keys[2] === "verb"
      && entry.verb === expected.verb
      && entry.builtIn === expected.builtIn
      && entry.scope === expected.scope;
  });
}

/**
 * One scoped-command round trip over raw IPC: a fresh reply topic subscribed BEFORE the publish,
 * the request published with the provider (never through the runtime), and the subscription removed
 * on the way out so a 4-probe x N-target sweep cannot leak IPC subscriptions.
 */
async function ggScopeRequest(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetActor: string,
  topic: string,
  verb: string,
  commandBody: Record<string, unknown>,
): Promise<{ correlation_match: boolean; body: Record<string, unknown> } | null> {
  const replyTopic = `edgecommons/interop/scope/${runId}/reply/${senderActor}/${targetActor}/${randomUUID()}`;
  const replies: Message[] = [];
  await svc.subscribe(replyTopic, (_topic, reply) => {
    replies.push(reply);
  }, 2, 1);
  try {
    const request = MessageBuilder.create(verb, "1.0")
      .withCommand(commandBody)
      .withReplyTo(replyTopic)
      .withTags({})
      .build();
    const correlation = request.getCorrelationId();
    await svc.publish(topic, request);
    const deadline = Date.now() + 10_000;
    while (replies.length === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    if (replies.length === 0) return null;
    const reply = replies[0];
    return {
      correlation_match: reply.getCorrelationId() === correlation,
      body: (reply.getBody() ?? {}) as Record<string, unknown>,
    };
  } finally {
    await svc.unsubscribe(replyTopic).catch(() => undefined);
  }
}

/** Probe 1 — the `describe` manifest declares a scope on every entry (D-SC-2). */
async function ggScopeDescribeProbe(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetActor: string,
): Promise<Record<string, unknown>> {
  const reply = await ggScopeRequest(
    svc, runId, senderActor, targetActor,
    `${ggScopeBaseTopic(targetActor)}/cmd/describe`, "describe", { from: LANG },
  );
  if (reply === null) return { ok: false, commands: null, digest: null, error: "timeout" };
  const result = reply.body.result as Record<string, unknown> | undefined;
  const commands = result?.commands;
  const digest = typeof result?.digest === "string" ? result.digest : null;
  const ok = reply.correlation_match
    && reply.body.ok === true
    && Array.isArray(commands)
    && digest !== null
    && ggScopeCommandsMatch(commands);
  return {
    ok,
    commands: commands ?? null,
    digest,
    ...(ok ? {} : { error: `unexpected describe manifest: ${JSON.stringify(reply.body)}` }),
  };
}

/** Probe 2 — an instance topic addresses an INSTANCE-scoped verb with NO `body.instance`. */
async function ggScopeInstanceRoutingProbe(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetLanguage: string,
  targetActor: string,
): Promise<Record<string, unknown>> {
  const reply = await ggScopeRequest(
    svc, runId, senderActor, targetActor,
    `${ggScopeBaseTopic(targetActor)}/kep1/cmd/${GG_SCOPE_PROBE_VERB}`, GG_SCOPE_PROBE_VERB,
    { from: LANG },
  );
  if (reply === null) {
    return { ok: false, addressed_instance: null, reply_body: null, error: "timeout" };
  }
  const result = reply.body.result as Record<string, unknown> | undefined;
  const addressedInstance = result?.instance ?? null;
  const ok = reply.body.ok === true
    && addressedInstance === "kep1"
    && result?.probe === targetLanguage;
  return {
    ok,
    addressed_instance: addressedInstance,
    reply_body: result ?? null,
    ...(ok ? {} : { error: `unexpected instance routing reply: ${JSON.stringify(reply.body)}` }),
  };
}

/**
 * Probes 3 and 4 — the two coded rejections. Both assert BAD_ARGS plus a BYTE-EXACT message, so a
 * reworded library string is a cross-language interop failure rather than a silent drift.
 */
async function ggScopeRejectionProbe(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetActor: string,
  topic: string,
  verb: string,
  commandBody: Record<string, unknown>,
  expectedMessage: string,
): Promise<Record<string, unknown>> {
  const reply = await ggScopeRequest(
    svc, runId, senderActor, targetActor, topic, verb, commandBody,
  );
  if (reply === null) return { ok: false, code: null, message: null, error: "timeout" };
  const error = reply.body.error as Record<string, unknown> | undefined;
  const code = error?.code ?? null;
  const message = error?.message ?? null;
  const ok = reply.body.ok === false && code === "BAD_ARGS" && message === expectedMessage;
  return {
    ok,
    code,
    message,
    ...(ok ? {} : { error: `expected BAD_ARGS '${expectedMessage}', got: ${JSON.stringify(reply.body)}` }),
  };
}

/** Probe 3 — a `body.instance` disagreeing with the addressed instance is refused. */
function ggScopeConflictProbe(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetActor: string,
): Promise<Record<string, unknown>> {
  return ggScopeRejectionProbe(
    svc, runId, senderActor, targetActor,
    `${ggScopeBaseTopic(targetActor)}/kep1/cmd/${GG_SCOPE_PROBE_VERB}`, GG_SCOPE_PROBE_VERB,
    { from: LANG, instance: "other" }, GG_SCOPE_CONFLICT_MESSAGE,
  );
}

/** Probe 4 — a COMPONENT-scoped verb refuses instance addressing. */
function ggScopeComponentScopeProbe(
  svc: DefaultMessagingService,
  runId: string,
  senderActor: string,
  targetActor: string,
): Promise<Record<string, unknown>> {
  return ggScopeRejectionProbe(
    svc, runId, senderActor, targetActor,
    `${ggScopeBaseTopic(targetActor)}/kep1/cmd/${GG_SCOPE_DISCOVER_VERB}`, GG_SCOPE_DISCOVER_VERB,
    { from: LANG }, GG_SCOPE_COMPONENT_MESSAGE,
  );
}

async function runGgScopeMatrix(runId: string, langsCsv: string): Promise<number> {
  const languages = langsCsv.split(",").filter(Boolean);
  const expectedActors = (process.env.EDGECOMMONS_GG_READY_LANGS ?? langsCsv).split(",").filter(Boolean);
  const actor = process.env.EDGECOMMONS_GG_READY_LANG ?? LANG;
  const canonicalActor = actor !== "rustpeer";
  const subscribeDelaySecs = Number(process.env.EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS ?? "2");
  const svc = await ipcService();
  const errors = new Map<string, string>();
  const targets: Record<string, Record<string, unknown>> = {};
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  let path: string | undefined;
  try {
    path = writeCommandRuntimeConfig(`interop-scope-${actor}`);
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.ScopeResponder`)
      .args(ggLogRuntimeArgs(path))
      .configureCommands((inbox) => {
        inbox.register(GG_SCOPE_PROBE_VERB, CommandScopes.Instance, (_request, addressedInstance) => ({
          probe: LANG,
          actor,
          instance: addressedInstance ?? null,
        }));
        inbox.register(GG_SCOPE_DISCOVER_VERB, CommandScopes.Component, (_request, addressedInstance) => ({
          discover: LANG,
          actor,
          instance: addressedInstance ?? null,
        }));
      })
      .build();
    process.stdout.write("READY\n");
    writeFileSync(ggScopeReadyPath(runId, actor), String(Date.now()), "utf8");
    const readyMissing = await waitForGgScopeReady(runId, expectedActors);
    await new Promise((resolve) => setTimeout(resolve, subscribeDelaySecs * 1000));
    if (readyMissing.length === 0 && canonicalActor) {
      for (const targetLanguage of languages) {
        const targetActor = ggP1TargetActor(targetLanguage, actor);
        try {
          // Sequential on purpose: one reply topic is live at a time, per target and per probe.
          const describe = await ggScopeDescribeProbe(svc, runId, actor, targetActor);
          const instanceRouting = await ggScopeInstanceRoutingProbe(
            svc, runId, actor, targetLanguage, targetActor,
          );
          const conflict = await ggScopeConflictProbe(svc, runId, actor, targetActor);
          const componentScope = await ggScopeComponentScopeProbe(svc, runId, actor, targetActor);
          targets[targetLanguage] = {
            target_actor: targetActor,
            describe,
            instance_routing: instanceRouting,
            conflict,
            component_scope: componentScope,
          };
        } catch (error) {
          // One unreachable target must not abort the sweep over the remaining languages.
          errors.set(`probe:${targetLanguage}`, String(error));
        }
      }
    }
    writeFileSync(ggScopeDonePath(runId, actor), String(Date.now()), "utf8");
    const doneMissing = await waitForGgScopeDone(
      runId, expectedActors.filter((expected) => expected !== "rustpeer"),
    );
    const targetsOk = !canonicalActor || languages.every((language) => {
      const target = targets[language];
      return target !== undefined && GG_SCOPE_PROBE_KEYS.every(
        (key) => (target[key] as Record<string, unknown> | undefined)?.ok === true,
      );
    });
    const ok = readyMissing.length === 0 && errors.size === 0 && targetsOk;
    const result = {
      schema: "edgecommons.gg-ipc-scope.v1",
      ok,
      run_id: runId,
      actor,
      language: LANG,
      canonical_actor: canonicalActor,
      ready_missing: readyMissing,
      done_missing: doneMissing,
      targets,
      errors: Object.fromEntries(errors),
    };
    writeFileSync(`/tmp/edgecommons_gg_ipc_scope_${actor}_${runId}.json`, JSON.stringify(result), "utf8");
    emit(result);
    return ok ? 0 : 1;
  } finally {
    if (gg) await gg.close();
    if (path) {
      try {
        unlinkSync(path);
      } catch {
        // best effort after a failed runtime startup
      }
    }
    await svc.disconnect();
  }
}

async function runGgLogMatrix(runId: string, langsCsv: string): Promise<number> {
  const expectedLangs = langsCsv.split(",").filter(Boolean);
  const expected = new Set(expectedLangs);
  const readyLangs = (process.env.EDGECOMMONS_GG_READY_LANGS ?? langsCsv).split(",").filter(Boolean);
  const readyLang = process.env.EDGECOMMONS_GG_READY_LANG ?? LANG;
  const subscribeDelaySecs = Number(process.env.EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS ?? "8");
  const waitSecs = Number(process.env.EDGECOMMONS_GG_WAIT_SECS ?? "35");
  const svc = await ipcService();
  const received = new Map<string, unknown>();
  const errors = new Map<string, string>();
  try {
    await svc.subscribe(
      "ecv1/interop-device/+/log/warn",
      (topic, message) => {
        try {
          const envelope = message.toObject() as Record<string, any>;
          const identity = envelope.identity as Record<string, unknown> | undefined;
          const component = typeof identity?.component === "string" ? identity.component : "";
          const publisher = component.startsWith("interop-log-")
            ? component.slice("interop-log-".length)
            : component;
          const body = message.getBody() as Record<string, any>;
          const fields = (body.fields ?? {}) as Record<string, unknown>;
          const ok = expected.has(publisher)
            && wireIdentityDevice(identity) === "interop-device"
            // D-U28: the component-scope log record omits the instance token on the wire;
            // its absence is the omit-when-absent proof over Greengrass IPC.
            && identity?.instance === undefined
            && body.schema === "edgecommons.log.v1"
            && body.level === "WARN"
            && body.logger === `interop.${publisher}`
            && body.message === `gg-log-interop-${runId}-${publisher}`
            && fields.runId === runId
            && fields.publisher === publisher;
          if (publisher && !received.has(publisher)) {
            received.set(publisher, { ok, topic, identity, body });
          }
        } catch (e) {
          errors.set(`log:${topic}`, String(e));
        }
      },
      64,
      1,
    );
    process.stdout.write("READY\n");
    writeFileSync(ggLogReadyPath(runId, readyLang), "ready", "utf8");
    const readyMissing = await waitForGgLogReady(runId, readyLangs);
    await new Promise((r) => setTimeout(r, subscribeDelaySecs * 1000));
    let published: unknown = {};
    if (readyMissing.length === 0) {
      const path = writeLogRuntimeConfig();
      let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
      try {
        gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.LogPublisher`)
          .args(ggLogRuntimeArgs(path))
          .build();
        await gg.logs().publish({
          level: "WARN",
          logger: `interop.${LANG}`,
          message: `gg-log-interop-${runId}-${LANG}`,
          fields: { runId, publisher: LANG },
        });
        await gg.logs().flush();
        published = gg.logs().stats();
      } finally {
        if (gg) await gg.close();
        try {
          unlinkSync(path);
        } catch {
          // best effort
        }
      }
    }

    const deadline = Date.now() + waitSecs * 1000;
    while (Date.now() < deadline) {
      if (expectedLangs.every((lang) => received.has(lang))) break;
      await new Promise((r) => setTimeout(r, 100));
    }

    const missing = expectedLangs.filter((lang) => !received.has(lang));
    const allOk = expectedLangs.every((lang) => (received.get(lang) as any)?.ok === true);
    const result = {
      ok: readyMissing.length === 0 && missing.length === 0 && errors.size === 0 && allOk,
      lang: LANG,
      run_id: runId,
      ready_missing: readyMissing,
      received: Object.fromEntries(received),
      missing,
      errors: Object.fromEntries(errors),
      published,
    };
    writeFileSync(`/tmp/edgecommons_gg_ipc_log_${readyLang}_${runId}.json`, JSON.stringify(result), "utf8");
    emit(result);
    return result.ok ? 0 : 1;
  } finally {
    await svc.disconnect();
  }
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
async function runUnsGuard(topic?: string): Promise<number> {
  const svc = await service("guard");
  try {
    // Reserved-class target selectable (D-U28): instance-scoped default or the
    // component-scoped ecv1/dev1/comp1/state — the guard must reject both.
    const guardTopic = topic ?? "ecv1/dev1/comp1/main/state";
    try {
      await svc.publishRaw(guardTopic, { from: LANG });
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

// --- per-instance connectivity: the `status` verb (pull) + `state.instances[]` (push) ----------
//
// ONE provider feeds both surfaces (the library samples it through the same seam), so the sample
// below is the single cross-language canonical fixture — see test_interop.EXPECTED_INSTANCES.
// The three elements pin the contract: every optional member present (cam-01), a rich `state` that
// a boolean cannot express (cam-02, BACKOFF != FAILED), and the minimal element whose optional
// members must be OMITTED, never emitted as null/empty (cam-03).

/** The fixed interop device/thing token every node's runtime identity is stamped with. */
const DEVICE = "interop-device";

/** The canonical provider sample every language's node reports, verbatim. */
function canonicalInstances(): InstanceConnectivity[] {
  return [
    InstanceConnectivity.of("cam-01", true, "rtsp://cam-01/stream")
      .withState("ONLINE")
      .withAttributes({ capabilities: ["ptz", "snapshot"], vendor: "acme", retries: 0 }),
    InstanceConnectivity.of("cam-02", false, "connect timed out").withState("BACKOFF"),
    InstanceConnectivity.of("cam-03", true),
  ];
}

/** The component's own command-inbox topic for one verb (component scope, D-U28: no instance). */
function commandTopic(component: string, verb: string): string {
  return `ecv1/${DEVICE}/${component}/cmd/${verb}`;
}

/** The component's reserved `state` keepalive topic (component scope, D-U28: no instance). */
function stateTopic(component: string): string {
  return `ecv1/${DEVICE}/${component}/state`;
}

/** Keeps the event loop alive for a server role until the harness terminates it. */
function stayAlive(): Promise<never> {
  return new Promise<never>(() => {
    setInterval(() => undefined, 3_600_000);
  });
}

/**
 * status-responder <component> — a real component that registers the connectivity provider; the
 * library's always-on command inbox then serves the built-in `status` verb from that provider.
 */
async function runStatusResponder(component: string): Promise<never> {
  const path = writeCommandRuntimeConfig(component);
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  try {
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.StatusResponder`)
      .args(logRuntimeArgs(path))
      .build();
    gg.setInstanceConnectivityProvider(canonicalInstances);
    process.stdout.write("READY\n");
    await stayAlive();
  } finally {
    if (gg) await gg.close();
    try {
      unlinkSync(path);
    } catch {
      // best effort after a failed startup
    }
  }
  throw new Error("unreachable status responder completion");
}

/**
 * status-request <component> — pull the built-in `status` verb on that component's inbox and print
 * the verb's result (the command reply body is `{ok, result}`; `result` is the status payload).
 */
async function runStatusRequest(component: string): Promise<number> {
  const svc = await service("statusreq");
  try {
    const request = MessageBuilder.create("status", "1.0")
      .withCommand({ from: LANG })
      .withTags({})
      .build();
    let reply: Message;
    try {
      reply = await svc.request(commandTopic(component, "status"), request, 15_000);
    } catch (e) {
      emit({ ok: false, error: `timeout: ${String(e)}` });
      return 1;
    }
    const body = reply.getBody() as Record<string, unknown> | null;
    if (!body || body.ok !== true) {
      emit({ ok: false, error: `command failed: ${JSON.stringify(body)}` });
      return 1;
    }
    const result = body.result as Record<string, unknown> | undefined;
    if (!result || result.status !== "RUNNING") {
      emit({ ok: false, error: `unexpected status result: ${JSON.stringify(result)}` });
      return 1;
    }
    emit({ ok: true, reply_body: result });
    return 0;
  } finally {
    await svc.disconnect();
  }
}

/**
 * state-instances-pub <component> — the same component with the heartbeat ENABLED, so the RUNNING
 * `state` keepalive pushes the very sample the `status` verb returns.
 */
async function runStateInstancesPub(component: string): Promise<never> {
  const path = writeCommandRuntimeConfig(component, true);
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  try {
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.StateInstancesPublisher`)
      .args(logRuntimeArgs(path))
      .build();
    gg.setInstanceConnectivityProvider(canonicalInstances);
    process.stdout.write("READY\n");
    await stayAlive();
  } finally {
    if (gg) await gg.close();
    try {
      unlinkSync(path);
    } catch {
      // best effort after a failed startup
    }
  }
  throw new Error("unreachable state instances publisher completion");
}

/**
 * state-instances-sub <component> — subscribe that component's reserved `state` topic (consuming a
 * reserved class is allowed; only PUBLISHING to one is rejected) and report the first RUNNING
 * keepalive carrying a non-empty instances[]. The first tick fires before the provider is
 * registered, so earlier RUNNING bodies without instances[] are skipped, not failed.
 */
async function runStateInstancesSub(component: string): Promise<number> {
  const svc = await service("statesub");
  try {
    const got = new Promise<Record<string, unknown>>((resolve) => {
      void svc
        .subscribe(stateTopic(component), (_t, m) => {
          const body = m.getBody() as Record<string, unknown> | null;
          const instances = body?.instances;
          if (body?.status === "RUNNING" && Array.isArray(instances) && instances.length > 0) {
            resolve(body);
          }
        })
        .then(() => process.stdout.write("READY\n"));
    });
    const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), 35_000));
    const body = await Promise.race([got, timeout]);
    if (body === null) {
      emit({ ok: false, error: "timeout: no RUNNING state carrying instances[]" });
      return 1;
    }
    emit({ ok: true, state_status: body.status, instances: body.instances });
    return 0;
  } finally {
    await svc.disconnect();
  }
}

// --- declared verb scope on the `describe` manifest (DESIGN-scoped-commands §2.3) --------------

/**
 * The ONE custom verb every language's describe-responder registers, declared INSTANCE-scoped, so
 * the manifest advertises a non-built-in entry whose `scope` differs from the five built-ins (all
 * `both`, D-SC-3).
 */
const DESCRIBE_PROBE_VERB = "sb/probe";

/**
 * describe-responder <component> — a real component carrying exactly ONE custom verb, `sb/probe`,
 * declared INSTANCE-scoped, so the built-in `describe` manifest advertises a non-built-in entry
 * with scope "instance" beside the five "both" built-ins.
 */
async function runDescribeResponder(component: string): Promise<never> {
  const path = writeCommandRuntimeConfig(component);
  let gg: Awaited<ReturnType<EdgeCommonsBuilder["build"]>> | undefined;
  try {
    gg = await new EdgeCommonsBuilder(`com.mbreissi.edgecommons.interop.${LANG}.DescribeResponder`)
      .args(logRuntimeArgs(path))
      .configureCommands((inbox) => {
        inbox.register(DESCRIBE_PROBE_VERB, CommandScopes.Instance, (_request, addressedInstance) => ({
          probe: LANG,
          instance: addressedInstance ?? null,
        }));
      })
      .build();
    process.stdout.write("READY\n");
    await stayAlive();
  } finally {
    if (gg) await gg.close();
    try {
      unlinkSync(path);
    } catch {
      // best effort after a failed startup
    }
  }
  throw new Error("unreachable describe responder completion");
}

/**
 * describe-requester <component> — pull the built-in `describe` verb on that component's inbox and
 * print the manifest (the command reply body is `{ok, result}`; `result` is the manifest).
 */
async function runDescribeRequester(component: string): Promise<number> {
  const svc = await service("describereq");
  try {
    const request = MessageBuilder.create("describe", "1.0")
      .withCommand({ from: LANG })
      .withTags({})
      .build();
    let reply: Message;
    try {
      reply = await svc.request(commandTopic(component, "describe"), request, 15_000);
    } catch (e) {
      emit({ ok: false, error: `timeout: ${String(e)}` });
      return 1;
    }
    const body = reply.getBody() as Record<string, unknown> | null;
    if (!body || body.ok !== true) {
      emit({ ok: false, error: `command failed: ${JSON.stringify(body)}` });
      return 1;
    }
    const result = body.result as Record<string, unknown> | undefined;
    if (!result || !Array.isArray(result.commands) || typeof result.digest !== "string") {
      emit({ ok: false, error: `unexpected describe manifest: ${JSON.stringify(result)}` });
      return 1;
    }
    emit({ ok: true, reply_body: result });
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
    case "deferred-responder":
      await runDeferredResponder(a);
      return;
    case "deferred-request":
      process.exit(await runDeferredRequest(a, b));
    case "confirmed-sub":
      process.exit(await runConfirmedSub(a, b));
    case "confirmed-pub":
      process.exit(await runConfirmedPub(a, b));
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
    case "log-sub":
      process.exit(await runLogSub(a, b));
    case "log-pub":
      process.exit(await runLogPub(a));
    case "gg-log-matrix":
      process.exit(await runGgLogMatrix(a, b));
    case "gg-binary-matrix":
      process.exit(await runGgBinaryMatrix(a, b, c));
    case "gg-p1-matrix":
      process.exit(await runGgP1Matrix(a, b));
    case "gg-scope-matrix":
      process.exit(await runGgScopeMatrix(a, b));
    case "uns-pub":
      process.exit(await runUnsPub(a, b, c));
    case "uns-sub":
      process.exit(await runUnsSub(a));
    case "uns-guard":
      process.exit(await runUnsGuard(a));
    case "status-responder":
      await runStatusResponder(a);
      return;
    case "status-request":
      process.exit(await runStatusRequest(a));
    case "state-instances-pub":
      await runStateInstancesPub(a);
      return;
    case "state-instances-sub":
      process.exit(await runStateInstancesSub(a));
    case "describe-responder":
      await runDescribeResponder(a);
      return;
    case "describe-requester":
      process.exit(await runDescribeRequester(a));
    default:
      process.stderr.write(`unknown role: ${role}\n`);
      process.exit(2);
  }
}

void main();
