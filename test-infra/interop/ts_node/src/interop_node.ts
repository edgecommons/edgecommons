/**
 * Cross-language interop node (TypeScript) for ggcommons. See python_node.py for
 * the shared CLI contract. Local-only MQTT transport against localhost:1883, using the
 * public ggcommons API (StandaloneMqttProvider + DefaultMessagingService), exactly
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
  MessageIdentity,
  DefaultMessagingService,
  StandaloneMqttProvider,
  ReservedTopicError,
  Uns,
  unsClassFromToken,
} from "@edgecommons/ggcommons";
import type { MessagingConfig } from "@edgecommons/ggcommons";

const LANG = "ts";
const HOST = process.env.GGCOMMONS_IT_MQTT_HOST ?? "localhost";
const PORT = Number(process.env.GGCOMMONS_IT_MQTT_PORT ?? "1883");

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
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const raw = m.getRaw() as Record<string, unknown> | undefined;
    const isRaw = m.isRaw();
    const rawToken = raw && typeof raw === "object" ? (raw.token as unknown) : null;
    const ok = isRaw && rawToken === token;
    emit({ ok: !!ok, is_raw: isRaw, raw_token: rawToken ?? null });
    return ok ? 0 : 1;
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
