/**
 * Messaging — MQTT-transport dual-broker configuration.
 *
 * Loaded from the `--transport MQTT <path>` JSON file. On the `KUBERNETES` platform the path is
 * optional: under `CONFIGMAP` + MQTT it defaults to the resolved ConfigMap file
 * (`/etc/edgecommons/config.json` by default), so a single mounted ConfigMap file doubles as both this
 * messaging-config (read via the `messaging` wrapper key below) and the component config (FR-MSG-1).
 *
 * `messaging.local` is required; `messaging.northbound` is optional — its presence selects single-broker
 * (air-gapped, local only) vs dual-MQTT (local + northbound MQTT). The host is an opaque
 * string, so a Kubernetes Service DNS name (e.g. `emqx.mqtt.svc.cluster.local`) works with no special
 * handling (FR-MSG-2/3). Shape matches the Java/Python/Rust libraries:
 *
 * ```json
 * { "messaging": {
 *     "local":   { "host": "localhost", "port": 1883, "clientId": "c-local",
 *                  "qos": { "publish": 1, "subscribe": 1 },
 *                  "credentials": { "username": "u", "password": "p" } },
 *     "northbound": { "host": "...", "port": 8883, "clientId": "c-north",
 *                  "qos": { "publish": 1, "subscribe": 1 },
 *                  "credentials": { "certPath": "...", "keyPath": "...", "caPath": "..." } }
 * } }
 * ```
 */
import { readFile } from "fs/promises";

import { EdgeCommonsError } from "../errors";
import { Qos } from "./types";

/** Local-broker or northbound broker credentials. */
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
  /** Defaults for MQTT operations using this broker when no explicit QoS is supplied. */
  qos?: QosDefaults;
  credentials?: Credentials;
}

/** Publish/subscribe QoS defaults for one broker side. */
export interface QosDefaults {
  publish: Qos;
  subscribe: Qos;
}

/** Default MQTT QoS for operations without an explicit QoS argument. */
export interface QosConfig {
  /** Local MQTT broker defaults. Supports QoS 0/1/2. */
  local: QosDefaults;
  /** Northbound MQTT broker defaults. Supports QoS 0/1/2. */
  northbound: QosDefaults;
}

/** The full STANDALONE messaging configuration. */
export interface MessagingConfig {
  local: BrokerConfig;
  northbound?: BrokerConfig;
}

/** Resolve a broker's host (prefers `host`, then `endpoint`). */
export function resolvedHost(broker: BrokerConfig): string {
  const h = broker.host ?? broker.endpoint;
  if (!h) throw EdgeCommonsError.messaging("broker config has neither 'host' nor 'endpoint'");
  return h;
}

function parseBroker(raw: unknown, defaultPort: number, prefix: string): BrokerConfig {
  const o = (raw ?? {}) as Record<string, unknown>;
  return {
    host: typeof o.host === "string" ? o.host : undefined,
    endpoint: typeof o.endpoint === "string" ? o.endpoint : undefined,
    port: typeof o.port === "number" ? o.port : defaultPort,
    clientId: typeof o.clientId === "string" ? o.clientId : `edgecommons-ts-${defaultPort}`,
    qos: parseQosDefaults(o.qos, 2, `${prefix}.qos`),
    credentials: o.credentials as Credentials | undefined,
  };
}

/** Load and parse a STANDALONE messaging config file. */
export async function loadMessagingConfig(path: string): Promise<MessagingConfig> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (e) {
    throw EdgeCommonsError.io(`could not read messaging config '${path}': ${String(e)}`);
  }
  let doc: Record<string, unknown>;
  try {
    doc = JSON.parse(text) as Record<string, unknown>;
  } catch (e) {
    throw EdgeCommonsError.json(`messaging config '${path}' is not valid JSON: ${String(e)}`);
  }
  const messaging = (doc.messaging ?? {}) as Record<string, unknown>;
  if (Object.prototype.hasOwnProperty.call(messaging, "lwt")) {
    throw EdgeCommonsError.messaging(
      "messaging.lwt is not supported; uns-bridge derives its site Last-Will internally",
    );
  }
  if (Object.prototype.hasOwnProperty.call(messaging, "qos")) {
    throw EdgeCommonsError.messaging(
      "messaging.qos is not supported; configure QoS under messaging.local.qos and messaging.northbound.qos",
    );
  }
  if (!messaging.local) {
    throw EdgeCommonsError.messaging("messaging config must define 'messaging.local'");
  }
  return {
    local: parseBroker(messaging.local, 1883, "messaging.local"),
    northbound: messaging.northbound ? parseBroker(messaging.northbound, 8883, "messaging.northbound") : undefined,
  };
}

function parseQosValue(raw: unknown, max: 1 | 2, field: string): Qos {
  if (raw === undefined) {
    return Qos.AtLeastOnce;
  }
  const n = typeof raw === "number" && Number.isInteger(raw) ? raw : NaN;
  if (Number.isNaN(n) || n < 0 || n > max) {
    throw EdgeCommonsError.messaging(`${field} must be 0..${max} (got ${String(raw)})`);
  }
  return n === 0 ? Qos.AtMostOnce : n === 1 ? Qos.AtLeastOnce : Qos.ExactlyOnce;
}

function parseQosDefaults(raw: unknown, max: 1 | 2, prefix: string): QosDefaults {
  const o = (raw ?? {}) as Record<string, unknown>;
  return {
    publish: parseQosValue(o.publish, max, `${prefix}.publish`),
    subscribe: parseQosValue(o.subscribe, max, `${prefix}.subscribe`),
  };
}

export function defaultQosDefaults(): QosDefaults {
  return parseQosDefaults(undefined, 2, "messaging.local.qos");
}

export function defaultQosConfig(): QosConfig {
  return {
    local: defaultQosDefaults(),
    northbound: parseQosDefaults(undefined, 2, "messaging.northbound.qos"),
  };
}

/** Build the service's effective QoS defaults from the two broker sections. */
export function qosConfigFromBrokers(config: MessagingConfig): QosConfig {
  return {
    local: config.local.qos ?? defaultQosDefaults(),
    northbound: config.northbound?.qos ?? parseQosDefaults(undefined, 2, "messaging.northbound.qos"),
  };
}
