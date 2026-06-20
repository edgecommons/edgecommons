/**
 * Cross-language interop node (TypeScript) for ggcommons. See python_node.py for
 * the shared CLI contract. STANDALONE local-only against localhost:1883.
 *
 *   interop_node responder <request_topic>
 *   interop_node request   <request_topic> <token>
 *   interop_node raw-sub   <topic> <token>
 *   interop_node raw-pub   <topic> <token>
 *
 * For the spike this node lives inside libs/ts and is compiled to
 * dist/interop_node.js, so the interop harness can run it with `node` without a
 * separate package build.
 */
import { Message, MessageBuilder } from "./message";
import { StandaloneProvider } from "./standalone";

const LANG = "ts";
const HOST = process.env.GGCOMMONS_IT_MQTT_HOST ?? "localhost";
const PORT = Number(process.env.GGCOMMONS_IT_MQTT_PORT ?? "1883");

function provider(suffix: string): Promise<StandaloneProvider> {
  return StandaloneProvider.connect({
    host: HOST,
    port: PORT,
    clientId: `interop-${LANG}-${suffix}-${process.pid}`,
  });
}

function emit(obj: unknown): void {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

async function runResponder(topic: string): Promise<never> {
  const prov = await provider("resp");
  await prov.subscribe(topic, (_t, request) => {
    const reply = MessageBuilder.create("InteropReply", "1.0")
      .withPayload({ echo: request.getBody(), responder: LANG })
      .withTags({})
      .build();
    void prov.reply(request, reply);
  });
  process.stdout.write("READY\n");
  // Run until killed by the harness.
  return new Promise<never>(() => {});
}

async function runRequest(topic: string, token: string): Promise<number> {
  const prov = await provider("req");
  try {
    const req = MessageBuilder.create("InteropRequest", "1.0")
      .withPayload({ token, from: LANG })
      .withTags({})
      .build();
    const corr = req.getCorrelationId();
    let reply: Message;
    try {
      reply = await prov.request(topic, req, 8000);
    } catch {
      emit({ ok: false, error: "timeout" });
      return 1;
    }
    const body = reply.getBody() as Record<string, unknown> | null;
    const match = reply.getCorrelationId() === corr;
    emit({ ok: true, correlation_match: match, reply_body: body });
    const echo = body && (body.echo as Record<string, unknown> | undefined);
    const ok =
      match &&
      !!body &&
      typeof body === "object" &&
      !!body.responder &&
      !!echo &&
      echo.token === token;
    return ok ? 0 : 1;
  } finally {
    await prov.disconnect();
  }
}

async function runRawSub(topic: string, token: string): Promise<number> {
  const prov = await provider("rawsub");
  try {
    const got = new Promise<Message>((resolve) => {
      void prov.subscribe(topic, (_t, m) => resolve(m)).then(() => {
        process.stdout.write("READY\n");
      });
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
    await prov.disconnect();
  }
}

async function runRawPub(topic: string, token: string): Promise<number> {
  const prov = await provider("rawpub");
  try {
    await prov.publishRaw(topic, { token, from: LANG });
    await new Promise((r) => setTimeout(r, 500)); // let the publish drain
    return 0;
  } finally {
    await prov.disconnect();
  }
}

async function main(): Promise<void> {
  const [role, a, b] = process.argv.slice(2);
  switch (role) {
    case "responder":
      await runResponder(a);
      return;
    case "request":
      process.exit(await runRequest(a, b));
    // falls through (process.exit never returns)
    case "raw-sub":
      process.exit(await runRawSub(a, b));
    case "raw-pub":
      process.exit(await runRawPub(a, b));
    default:
      process.stderr.write(`unknown role: ${role}\n`);
      process.exit(2);
  }
}

void main();
