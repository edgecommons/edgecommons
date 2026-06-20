/**
 * On-device CONFIG_COMPONENT provider (GREENGRASS, TS↔TS over IPC).
 *
 * A dedicated configuration-manager component: it answers GetConfiguration
 * request/reply over Greengrass IPC on the topic the consumer's
 * `CONFIG_COMPONENT` source uses — `ggcommons/{thing}/config/get/{ComponentName}` —
 * returning a full ggcommons config document. Lets the TS `CONFIG_COMPONENT` config
 * source be validated end-to-end on a live nucleus against a TS peer.
 *
 * Run args: the consumer's FULL component name (the CONFIG_COMPONENT source builds
 * the request topic from the full name, e.g. "com.ggcommons.TsGgVerify").
 */
import { DefaultMessagingService } from "./messaging/service";
import { IpcMessagingProvider } from "./messaging/ipc-provider";
import { MessageBuilder } from "./message";

const CONSUMER_NAME = process.argv[2] ?? "com.ggcommons.TsGgVerify";
const THING = process.env.AWS_IOT_THING_NAME ?? "lab-5950x";
const GET_TOPIC = `ggcommons/${THING}/config/get/${CONSUMER_NAME}`;

/** The config served to the consumer (mirrors the GG_CONFIG recipe values). */
const CONFIG = {
  logging: { level: "INFO" },
  heartbeat: {
    intervalSecs: 3,
    measures: { cpu: true, memory: true },
    targets: [{ type: "messaging", config: { destination: "ipc" } }],
  },
  metricEmission: { target: "log", namespace: "ggcommons", targetConfig: { logFileName: "/tmp/ts_gg_metric.log" } },
  tags: { site: "verify-site", appId: "ts-cc" },
  component: { global: { publish_interval: 7 }, instances: [] },
};

async function main(): Promise<void> {
  const provider = await IpcMessagingProvider.connect({ receiveOwnMessages: false });
  const svc = new DefaultMessagingService(provider);
  await svc.subscribe(GET_TOPIC, (_topic, request) => {
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

  process.stdout.write(`config provider ready on ${GET_TOPIC}\n`);
  // Stay alive serving config requests.
  setInterval(() => undefined, 1 << 30);
}

void main();
