/**
 * On-device GREENGRASS-mode IPC verification for the TS library.
 *
 * Runs as a deployed Greengrass component (so the nucleus supplies the IPC auth env
 * + a pubsub accessControl grant). Exercises, against a live nucleus, using the
 * refactored provider/service stack: request/reply, raw publish/ingest, and
 * cross-language ingest of the Java component's heartbeat envelope over IPC.
 *
 * Result is printed as one JSON line AND written to RESULT_PATH (world-readable).
 */
import { writeFileSync } from "fs";

import { MessageBuilder, Message } from "./message";
import { DefaultMessagingService } from "./messaging/service";
import { IpcMessagingProvider } from "./messaging/ipc-provider";

const RESULT_PATH = process.env.GGC_TS_VERIFY_OUT ?? "/tmp/ts_ipc_verify_result.json";
const HEARTBEAT_FILTER = "ggcommons/+/+/heartbeat";
const HEARTBEAT_WAIT_MS = 14000;

function rid(): string {
  return `${process.pid}-${process.hrtime.bigint()}`;
}

async function verifyHeartbeat(svc: DefaultMessagingService): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    let done = false;
    const finish = (r: Record<string, unknown>): void => {
      if (done) return;
      done = true;
      void svc.unsubscribe(HEARTBEAT_FILTER).finally(() => resolve(r));
    };
    const timer = setTimeout(
      () => finish({ ok: false, error: "no heartbeat received", filter: HEARTBEAT_FILTER }),
      HEARTBEAT_WAIT_MS,
    );
    svc
      .subscribe(HEARTBEAT_FILTER, (topic, m) => {
        if (m.isRaw()) return;
        const tags = m.tags as Record<string, unknown>;
        clearTimeout(timer);
        const body = m.getBody();
        finish({
          ok: m.header.name === "heartbeat",
          topic,
          header_name: m.header.name,
          thing: tags.thing ?? null,
          tag_keys: Object.keys(tags),
          body_keys: body && typeof body === "object" ? Object.keys(body as object) : [],
        });
      })
      .catch((e) => finish({ ok: false, error: `subscribe failed: ${String(e)}` }));
  });
}

async function verifyRequestReply(svc: DefaultMessagingService): Promise<Record<string, unknown>> {
  const topic = `ggcommons/interop/ipc/rr/${rid()}`;
  const token = rid();
  await svc.subscribe(topic, (_t, request) => {
    const reply = MessageBuilder.create("InteropReply", "1.0")
      .withPayload({ echo: request.getBody(), responder: "ts" })
      .build();
    void svc.reply(request, reply);
  });
  try {
    const req = MessageBuilder.create("InteropRequest", "1.0").withPayload({ token, from: "ts" }).build();
    const corr = req.getCorrelationId();
    const reply: Message = await svc.request(topic, req, 8000);
    const body = reply.getBody() as Record<string, unknown> | null;
    const echo = body?.echo as Record<string, unknown> | undefined;
    return {
      ok: reply.getCorrelationId() === corr && echo?.token === token && body?.responder === "ts",
      correlation_match: reply.getCorrelationId() === corr,
      echoed_token: echo?.token ?? null,
    };
  } finally {
    await svc.unsubscribe(topic);
  }
}

async function verifyRaw(svc: DefaultMessagingService): Promise<Record<string, unknown>> {
  const topic = `ggcommons/interop/ipc/raw/${rid()}`;
  const token = rid();
  return new Promise((resolve) => {
    let done = false;
    const finish = (r: Record<string, unknown>): void => {
      if (done) return;
      done = true;
      void svc.unsubscribe(topic).finally(() => resolve(r));
    };
    const timer = setTimeout(() => finish({ ok: false, error: "no raw message received" }), 8000);
    svc
      .subscribe(topic, (_t, m) => {
        clearTimeout(timer);
        const raw = m.getRaw() as Record<string, unknown> | undefined;
        finish({ ok: m.isRaw() && raw?.token === token, is_raw: m.isRaw(), token_match: raw?.token === token });
      })
      .then(() => svc.publishRaw(topic, { token, from: "ts" }))
      .catch((e) => finish({ ok: false, error: `raw failed: ${String(e)}` }));
  });
}

async function main(): Promise<void> {
  const results: Record<string, unknown> = { lang: "ts", mode: "GREENGRASS" };
  let svc: DefaultMessagingService | undefined;
  try {
    const provider = await IpcMessagingProvider.connect({ receiveOwnMessages: true });
    svc = new DefaultMessagingService(provider);
    results.connected = true;
    results.request_reply = await verifyRequestReply(svc);
    results.raw = await verifyRaw(svc);
    results.heartbeat_from_java = await verifyHeartbeat(svc);
  } catch (e) {
    results.connected = false;
    results.error = String(e);
  } finally {
    if (svc) await svc.disconnect().catch(() => undefined);
  }

  const rr = results.request_reply as Record<string, unknown> | undefined;
  const raw = results.raw as Record<string, unknown> | undefined;
  const hb = results.heartbeat_from_java as Record<string, unknown> | undefined;
  results.all_ok = Boolean(results.connected && rr?.ok && raw?.ok && hb?.ok);

  const line = JSON.stringify(results);
  process.stdout.write(line + "\n");
  try {
    writeFileSync(RESULT_PATH, line + "\n");
  } catch {
    /* best effort */
  }
  process.exit(results.all_ok ? 0 : 1);
}

void main();
