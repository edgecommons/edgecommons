/**
 * On-device GREENGRASS-mode lifecycle verification for the TS library.
 *
 * Runs as a deployed component built through the FULL `EdgeCommonsBuilder` (so the
 * whole runtime — IPC messaging, config source, logging, metrics, heartbeat — wires
 * up against the live nucleus). The config source is taken from the recipe's Run
 * args (`-c GG_CONFIG` | `SHADOW` | `CONFIG_COMPONENT`), so one component covers all
 * three. It runs ~120s, writing a JSON result to RESULT_PATH and updating it as
 * asynchronous events (config hot-reload, an IoT Core command) arrive, so an
 * external driver can stimulate it within the window.
 *
 * Checks: config-source load (values from the deployment config), request/reply +
 * raw over IPC, the log metric target, heartbeat actually firing over IPC, the IoT
 * Core bridge (subscribe + publish), and config hot-reload via the listener.
 */
import { writeFileSync, existsSync, statSync } from "fs";

import { EdgeCommons, EdgeCommonsBuilder } from "./edgecommons";
import { MessageBuilder } from "./message";
import { Qos } from "./messaging/types";

const RESULT_PATH = process.env.EDGECOMMONS_TS_VERIFY_OUT ?? "/tmp/ts_edge_verify_result.json";
const METRIC_LOG = "/tmp/ts_edge_metric.log";
const RUN_MS = 120_000;

function rid(): string {
  return `${process.pid}-${process.hrtime.bigint()}`;
}

const results: Record<string, unknown> = { lang: "ts", mode: "GREENGRASS" };
function write(): void {
  try {
    writeFileSync(RESULT_PATH, JSON.stringify(results) + "\n");
  } catch {
    /* best effort */
  }
}

function num(v: unknown): number | null {
  return typeof v === "number" ? v : null;
}

/** Include the eventstream-RPC `serviceError` detail when present. */
function errDetail(e: unknown): string {
  const se = (e as { serviceError?: unknown })?.serviceError;
  return se ? `${String(e)} serviceError=${JSON.stringify(se)}` : String(e);
}

async function main(): Promise<void> {
  let reloadCount = 0;
  let gg: EdgeCommons | undefined;
  // Every subscription registers its unsubscribe here so we ALWAYS clean up before
  // exiting — leaving broker/IPC subscriptions behind accumulates them on the
  // Nucleus's single shared MQTT connection (and eventually trips its per-connection
  // subscription quota).
  const cleanups: Array<() => Promise<void>> = [];
  let shuttingDown = false;
  const shutdown = async (code: number): Promise<void> => {
    if (shuttingDown) return;
    shuttingDown = true;
    await Promise.allSettled(cleanups.map((c) => c()));
    if (gg) await Promise.race([gg.close(), new Promise((r) => setTimeout(r, 5000))]);
    process.exit(code);
  };
  // Greengrass sends SIGTERM when a component is stopped/removed — unsubscribe then too.
  process.on("SIGTERM", () => void shutdown(0));
  process.on("SIGINT", () => void shutdown(0));
  try {
    gg = await new EdgeCommonsBuilder("com.mbreissi.edgecommons.TsEdgeVerify")
      .args(process.argv.slice(2))
      .receiveOwnMessages(true) // so in-process request/reply + heartbeat self-ingest work
      .build();
    results.connected = true;
    results.config_source = gg.args().config.kind;

    const cfg = gg.config();
    const global = (cfg.global() ?? {}) as Record<string, unknown>;
    results.config_loaded = {
      ok:
        num(global.publish_interval) !== null &&
        typeof cfg.parsed.tags.site === "string" &&
        cfg.parsed.metricEmission.target() === "log",
      publish_interval: num(global.publish_interval),
      site: cfg.parsed.tags.site ?? null,
      hb_interval: cfg.parsed.heartbeat.intervalSecs ?? null,
      metric_target: cfg.parsed.metricEmission.target(),
    };

    const svc = gg.messaging();
    const thing = cfg.thingName;

    // --- request/reply over IPC ---
    const rrTopic = `edgecommons/${thing}/tsedge/rr/${rid()}`;
    const rrToken = rid();
    try {
      await svc.subscribe(rrTopic, (_t, req) => {
        void svc.reply(
          req,
          MessageBuilder.create("InteropReply", "1.0").withPayload({ echo: req.getBody(), responder: "ts" }).build(),
        );
      });
      cleanups.push(() => svc.unsubscribe(rrTopic));
      const reply = await svc.request(
        rrTopic,
        MessageBuilder.create("InteropRequest", "1.0").withPayload({ token: rrToken }).build(),
        8000,
      );
      const body = reply.getBody() as Record<string, unknown> | null;
      const echo = body?.echo as Record<string, unknown> | undefined;
      results.request_reply = { ok: echo?.token === rrToken && body?.responder === "ts" };
    } catch (e) {
      results.request_reply = { ok: false, error: String(e) };
    }

    // --- raw over IPC ---
    const rawTopic = `edgecommons/${thing}/tsedge/raw/${rid()}`;
    const rawToken = rid();
    const rawDone = new Promise<Record<string, unknown>>((resolve) => {
      const timer = setTimeout(() => resolve({ ok: false, error: "timeout" }), 8000);
      void svc
        .subscribe(rawTopic, (_t, m) => {
          clearTimeout(timer);
          const raw = m.getRaw() as Record<string, unknown> | undefined;
          resolve({ ok: m.isRaw() && raw?.token === rawToken, is_raw: m.isRaw() });
        })
        .then(() => svc.publishRaw(rawTopic, { token: rawToken }));
    });
    cleanups.push(() => svc.unsubscribe(rawTopic));
    results.raw = await rawDone;

    // --- metric (log target from the deployment config) ---
    try {
      gg.metrics().defineMetric(
        (await import("./metrics/metric")).MetricBuilder.create("verify_pub").addMeasure("count", "Count", 60).build(),
      );
      await gg.metrics().emitMetric("verify_pub", { count: 1 });
      await gg.metrics().flushMetrics();
      results.metric_log = { ok: existsSync(METRIC_LOG) && statSync(METRIC_LOG).size > 0, path: METRIC_LOG };
    } catch (e) {
      results.metric_log = { ok: false, error: errDetail(e) };
    }

    // --- heartbeat actually fires over IPC (the library publishes it per config) ---
    const hbFilter = "ecv1/+/+/+/state";
    const componentToken = cfg.componentIdentity.component;
    void svc.subscribe(hbFilter, (topic, m) => {
      if (m.isRaw() || m.header.name !== "state") return;
      if (!topic.includes(componentToken)) return; // our own component's heartbeat
      if (!(results.heartbeat_over_ipc as { ok?: boolean })?.ok) {
        results.heartbeat_over_ipc = {
          ok: true,
          topic,
          body_keys: Object.keys((m.getBody() ?? {}) as object),
        };
        write();
      }
    });
    cleanups.push(() => svc.unsubscribe(hbFilter));

    // --- IoT Core bridge (device side) --- each leg non-fatal so the rest runs.
    const cmdTopic = `edgecommons/${thing}/ts-edge-verify/cmd`;
    const telemetryTopic = `edgecommons/${thing}/ts-edge-verify/telemetry`;
    try {
      await svc.subscribeNorthbound(
        cmdTopic,
        (_t, m) => {
          results.iot_command_received = { ok: true, body: m.isRaw() ? m.getRaw() : m.getBody() };
          write();
        },
        Qos.AtLeastOnce,
      );
      cleanups.push(() => svc.unsubscribeNorthbound(cmdTopic));
      results.iot_subscribe = { ok: true, topic: cmdTopic };
    } catch (e) {
      results.iot_subscribe = { ok: false, error: errDetail(e) };
    }
    try {
      await svc.publishNorthbound(
        telemetryTopic,
        MessageBuilder.create("Telemetry", "1.0").withConfig(cfg).withPayload({ seq: 1 }).build(),
        Qos.AtLeastOnce,
      );
      results.iot_publish = { ok: true, topic: telemetryTopic };
    } catch (e) {
      results.iot_publish = { ok: false, error: errDetail(e) };
    }

    // --- config hot-reload via listener ---
    gg.addConfigChangeListener({
      onConfigurationChange: (c) => {
        reloadCount += 1;
        const g = (c.global() ?? {}) as Record<string, unknown>;
        results.config_reload = { ok: true, count: reloadCount, publish_interval: num(g.publish_interval) };
        write();
        return true;
      },
    });

    results.cmd_topic = cmdTopic; // surfaced so the driver knows where to publish
    write();

    const ticker = setInterval(write, 5000);
    setTimeout(() => {
      clearInterval(ticker);
      results.done = true;
      write();
      void shutdown(0); // unsubscribes everything, then closes
    }, RUN_MS);
  } catch (e) {
    results.connected = false;
    results.error = String(e);
    write();
    void shutdown(1);
  }
}

void main();
