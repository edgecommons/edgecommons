/**
 * On-device GREENGRASS-mode IPC verification for the TS library.
 *
 * Runs as a deployed Greengrass component (so the nucleus supplies the IPC auth
 * env + a pubsub accessControl grant). It exercises, against a live nucleus:
 *
 *  1. CROSS-LANGUAGE interop — subscribes to the heartbeat topic and ingests the
 *     envelope published over IPC by the already-deployed *Java* ggcommons
 *     component, decoding it with the shared TS Message model. A received,
 *     well-formed `heartbeat` envelope proves Java -> TS interop over Greengrass
 *     IPC.
 *  2. request/reply over IPC — a responder + requester through the real nucleus
 *     (correlation id must round-trip; the responder must echo the body).
 *  3. raw publish/ingest over IPC — a non-envelope payload must arrive as raw.
 *
 * The result is printed as one JSON line AND written to a world-readable file
 * (RESULT_PATH) so it can be read back without root access to the gg logs dir.
 */
import { writeFileSync } from "fs";

import { MessageBuilder } from "./message";
import { IpcProvider } from "./ipc";

const RESULT_PATH = process.env.GGC_TS_VERIFY_OUT ?? "/tmp/ts_ipc_verify_result.json";
const HEARTBEAT_FILTER = "ggcommons/+/+/heartbeat";
const HEARTBEAT_WAIT_MS = 14000; // the Java component publishes every 5s

function rid(): string {
  // A coarse unique-ish suffix without Math.random (kept dependency-free here).
  return `${process.pid}-${process.hrtime.bigint()}`;
}

async function verifyHeartbeat(prov: IpcProvider): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    let done = false;
    const finish = (r: Record<string, unknown>) => {
      if (done) return;
      done = true;
      void prov.unsubscribe(HEARTBEAT_FILTER).finally(() => resolve(r));
    };
    const timer = setTimeout(
      () => finish({ ok: false, error: "no heartbeat received", filter: HEARTBEAT_FILTER }),
      HEARTBEAT_WAIT_MS,
    );
    prov
      .subscribe(HEARTBEAT_FILTER, (topic, m) => {
        if (m.isRaw()) return; // ignore any non-envelope traffic
        const header = m.header;
        const tags = m.tags as Record<string, unknown>;
        clearTimeout(timer);
        finish({
          ok: header.name === "heartbeat",
          topic,
          header_name: header.name,
          thing: tags.thing ?? null,
          tag_keys: Object.keys(tags),
          body_keys: m.getBody() && typeof m.getBody() === "object"
            ? Object.keys(m.getBody() as object)
            : [],
        });
      })
      .catch((e) => finish({ ok: false, error: `subscribe failed: ${e}` }));
  });
}

async function verifyRequestReply(prov: IpcProvider): Promise<Record<string, unknown>> {
  const topic = `ggcommons/interop/ipc/rr/${rid()}`;
  const token = rid();
  await prov.subscribe(topic, (_t, request) => {
    const reply = MessageBuilder.create("InteropReply", "1.0")
      .withPayload({ echo: request.getBody(), responder: "ts" })
      .build();
    void prov.reply(request, reply);
  });
  try {
    const req = MessageBuilder.create("InteropRequest", "1.0")
      .withPayload({ token, from: "ts" })
      .build();
    const corr = req.getCorrelationId();
    const reply = await prov.request(topic, req, 8000);
    const body = reply.getBody() as Record<string, unknown> | null;
    const echo = body?.echo as Record<string, unknown> | undefined;
    return {
      ok: reply.getCorrelationId() === corr && echo?.token === token && body?.responder === "ts",
      correlation_match: reply.getCorrelationId() === corr,
      echoed_token: echo?.token ?? null,
    };
  } finally {
    await prov.unsubscribe(topic);
  }
}

async function verifyRaw(prov: IpcProvider): Promise<Record<string, unknown>> {
  const topic = `ggcommons/interop/ipc/raw/${rid()}`;
  const token = rid();
  return new Promise((resolve) => {
    let done = false;
    const finish = (r: Record<string, unknown>) => {
      if (done) return;
      done = true;
      void prov.unsubscribe(topic).finally(() => resolve(r));
    };
    const timer = setTimeout(() => finish({ ok: false, error: "no raw message received" }), 8000);
    prov
      .subscribe(topic, (_t, m) => {
        clearTimeout(timer);
        const raw = m.getRaw() as Record<string, unknown> | undefined;
        finish({
          ok: m.isRaw() && raw?.token === token,
          is_raw: m.isRaw(),
          token_match: raw?.token === token,
        });
      })
      .then(() => prov.publishRaw(topic, { token, from: "ts" }))
      .catch((e) => finish({ ok: false, error: `raw failed: ${e}` }));
  });
}

async function main(): Promise<void> {
  const results: Record<string, unknown> = { lang: "ts", mode: "GREENGRASS" };
  let prov: IpcProvider | undefined;
  try {
    prov = await IpcProvider.connect({ receiveOwnMessages: true });
    results.connected = true;
    // request/reply + raw first (fast); heartbeat last (waits for the Java tick).
    results.request_reply = await verifyRequestReply(prov);
    results.raw = await verifyRaw(prov);
    results.heartbeat_from_java = await verifyHeartbeat(prov);
  } catch (e) {
    results.connected = false;
    results.error = String(e);
  } finally {
    if (prov) await prov.disconnect().catch(() => undefined);
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
