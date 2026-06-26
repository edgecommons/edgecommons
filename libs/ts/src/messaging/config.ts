/**
 * Messaging — MQTT-transport dual-broker configuration.
 *
 * Loaded from the `--transport MQTT <path>` JSON file. On the `KUBERNETES` platform the path is
 * optional: under `CONFIGMAP` + MQTT it defaults to the resolved ConfigMap file
 * (`/etc/ggcommons/config.json` by default), so a single mounted ConfigMap file doubles as both this
 * messaging-config (read via the `messaging` wrapper key below) and the component config (FR-MSG-1).
 *
 * `messaging.local` is required; `messaging.iotCore` is optional — its presence selects single-broker
 * (air-gapped, local only) vs dual-MQTT (local + AWS IoT Core, mutual-TLS). The host is an opaque
 * string, so a Kubernetes Service DNS name (e.g. `emqx.mqtt.svc.cluster.local`) works with no special
 * handling (FR-MSG-2/3). Shape matches the Java/Python/Rust libraries:
 *
 * ```json
 * { "messaging": {
 *     "local":   { "host": "localhost", "port": 1883, "clientId": "c-local",
 *                  "credentials": { "username": "u", "password": "p" } },
 *     "iotCore": { "endpoint": "...", "port": 8883, "clientId": "c-iot",
 *                  "credentials": { "certPath": "...", "keyPath": "...", "caPath": "..." } }
 * } }
 * ```
 */
import { readFile } from "fs/promises";

import { GgError } from "../errors";

/** Local-broker or IoT Core credentials. */
export interface Credentials {
  username?: string;
  password?: string;
  certPath?: string;
  keyPath?: string;
  caPath?: string;
}

/** One broker's connection settings. */
export interface BrokerConfig {
  host?: string;
  endpoint?: string;
  port: number;
  clientId: string;
  credentials?: Credentials;
}

/** The full STANDALONE messaging configuration. */
export interface MessagingConfig {
  local: BrokerConfig;
  iotCore?: BrokerConfig;
}

/** Resolve a broker's host (prefers `host`, then `endpoint`). */
export function resolvedHost(broker: BrokerConfig): string {
  const h = broker.host ?? broker.endpoint;
  if (!h) throw GgError.messaging("broker config has neither 'host' nor 'endpoint'");
  return h;
}

function parseBroker(raw: unknown, defaultPort: number): BrokerConfig {
  const o = (raw ?? {}) as Record<string, unknown>;
  return {
    host: typeof o.host === "string" ? o.host : undefined,
    endpoint: typeof o.endpoint === "string" ? o.endpoint : undefined,
    port: typeof o.port === "number" ? o.port : defaultPort,
    clientId: typeof o.clientId === "string" ? o.clientId : `ggcommons-ts-${defaultPort}`,
    credentials: o.credentials as Credentials | undefined,
  };
}

/** Load and parse a STANDALONE messaging config file. */
export async function loadMessagingConfig(path: string): Promise<MessagingConfig> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (e) {
    throw GgError.io(`could not read messaging config '${path}': ${String(e)}`);
  }
  let doc: Record<string, unknown>;
  try {
    doc = JSON.parse(text) as Record<string, unknown>;
  } catch (e) {
    throw GgError.json(`messaging config '${path}' is not valid JSON: ${String(e)}`);
  }
  const messaging = (doc.messaging ?? {}) as Record<string, unknown>;
  if (!messaging.local) {
    throw GgError.messaging("messaging config must define 'messaging.local'");
  }
  return {
    local: parseBroker(messaging.local, 1883),
    iotCore: messaging.iotCore ? parseBroker(messaging.iotCore, 8883) : undefined,
  };
}
