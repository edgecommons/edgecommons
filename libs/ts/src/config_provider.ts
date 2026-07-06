/**
 * On-device CONFIG_COMPONENT provider (GREENGRASS, TS↔TS over IPC).
 *
 * A dedicated configuration-manager component: it answers GetConfiguration request/reply over
 * Greengrass IPC on the UNS Flow-A rendezvous the consumer's `CONFIG_COMPONENT` source uses —
 * `ecv1/{device}/config/main/cmd/get-configuration` (UNS-CANONICAL-DESIGN §4.3, D-U19; the
 * server is the sole subscriber under the reserved-by-convention logical component name
 * `config`) — returning a full edgecommons config document to the requester that self-identifies
 * in the body with `{"component": "<bootstrap short name>"}`. Lets the TS `CONFIG_COMPONENT`
 * config source be validated end-to-end on a live nucleus against a TS peer.
 *
 * Run args: the consumer's FULL component name (the served component is matched against its
 * sanitized short name, e.g. "com.mbreissi.edgecommons.TsEdgeVerify" -> "TsEdgeVerify".
 * The served config then supplies `component.token` for the consumer's lower-kebab UNS token.
 */
import { DefaultMessagingService } from "./messaging/service";
import { IpcMessagingProvider } from "./messaging/ipc-provider";
import { MessageBuilder } from "./message";
import { sanitize } from "./config/template";

const CONSUMER_NAME = process.argv[2] ?? "com.mbreissi.edgecommons.TsEdgeVerify";
const THING = process.env.AWS_IOT_THING_NAME ?? "lab-5950x";
const GET_TOPIC = `ecv1/${sanitize(THING)}/config/main/cmd/get-configuration`;
const CONSUMER_TOKEN = sanitize(CONSUMER_NAME.split(".").pop() ?? CONSUMER_NAME);

/** The config served to the consumer (mirrors the GG_CONFIG recipe values). */
const CONFIG = {
  logging: { level: "INFO" },
  heartbeat: {
    enabled: true,
    intervalSecs: 3,
    measures: { cpu: true, memory: true },
    destination: "local",
  },
  metricEmission: { target: "log", namespace: "edgecommons", targetConfig: { logFileName: "/tmp/ts_edge_metric.log" } },
  tags: { site: "verify-site", appId: "ts-cc" },
  component: { token: "ts-edge-verify", global: { publish_interval: 7 }, instances: [] },
};

async function main(): Promise<void> {
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages: false });
  const svc = new DefaultMessagingService(provider);
  await svc.subscribe(GET_TOPIC, (_topic, request) => {
    // Flow A: the requester self-identifies in the body ({"component": "<short name>"}) —
    // serve only the consumer this provider was started for.
    const body = (request.getBody() ?? {}) as Record<string, unknown>;
    if (body.component !== CONSUMER_TOKEN) return;
    void svc.reply(request, MessageBuilder.create("Configuration", "1.0").withPayload(CONFIG).build());
  });

  // Unsubscribe + disconnect on stop so we never leave the subscription behind on
  // the Nucleus's shared MQTT connection (Greengrass sends SIGTERM on stop/remove).
  let shuttingDown = false;
  const shutdown = async (): Promise<void> => {
    if (shuttingDown) return;
    shuttingDown = true;
    try {
      await svc.unsubscribe(GET_TOPIC);
      await svc.disconnect();
    } finally {
      process.exit(0);
    }
  };
  process.on("SIGTERM", () => void shutdown());
  process.on("SIGINT", () => void shutdown());

  process.stdout.write(`config provider ready on ${GET_TOPIC} (serving ${CONSUMER_TOKEN})\n`);
  // Stay alive serving config requests.
  setInterval(() => undefined, 1 << 30);
}

void main();
