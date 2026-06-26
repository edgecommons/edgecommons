/**
 * Platform × transport runtime model (DESIGN-core §2–§6).
 *
 * The pure precedence resolver and platform auto-detector. Maps parse-time inputs
 * (explicit flags, then environment, then the platform-profile defaults) to a single
 * {@link ResolvedProfile} consumed by the lifecycle builder. Mirrors the canonical Java
 * `com.breissinger.ggcommons.platform` package.
 *
 * One rule governs every defaultable setting:
 * ```
 *   resolve(setting) = explicit flag ▸ platform-profile default ▸ library default
 * ```
 *
 * **Phase 0:** {@link Platform.GREENGRASS} and {@link Platform.HOST} both default their config source
 * to `GG_CONFIG` (a faithful re-expression of today's behavior — HOST does NOT flip to `FILE` until
 * Phase 1).
 *
 * **Phase 1a:** {@link Platform.KUBERNETES} now has a profile (transport `MQTT`, config source
 * `CONFIGMAP`) and resolves cleanly — a service-account-token pod auto-detects to it. The
 * IPC×KUBERNETES rejection still holds (the IPC lock).
 *
 * **Phase 1b:** {@link resolveIdentity} now reads the Kubernetes Downward-API env vars
 * (`GGCOMMONS_THING_NAME`, then `POD_NAME`) ahead of the generic `AWS_IOT_THING_NAME` probe **when
 * the resolved platform is KUBERNETES** (FR-RT-7).
 *
 * **Phase 1c:** the KUBERNETES profile now defaults its logging format to {@link JSON_LOG_FORMAT} (the
 * structured stdout-JSON sink, FR-LOG-1); {@link profileLoggingFormat} exposes that default to the
 * logging configurator. The KUBERNETES profile also turns the HTTP health endpoint on by default
 * (FR-HB-1); {@link profileHealthEnabled} exposes that default to the lifecycle builder. The
 * `prometheus` metrics target remains deferred to a later Phase-1 sub-phase.
 */
import { existsSync } from "fs";

import { GgError } from "./errors";
import { logger } from "./logging";

/**
 * The deployment platform — the primary runtime axis (DESIGN-core §2/§3). A platform is a named
 * profile: a table of per-subsystem default providers/targets/sinks selected by {@link resolveProfile}.
 * Orthogonal to {@link Transport}; only messaging-transport is platform-coupled (via the IPC lock,
 * {@link validate}). Phase 0 populates only GREENGRASS and HOST; KUBERNETES is declared but not wired.
 */
export enum Platform {
  /** On an AWS IoT Greengrass v2 Nucleus: IPC transport, Nucleus-managed config/identity. */
  GREENGRASS = "GREENGRASS",
  /** A plain host (Docker/bare host without a Nucleus): MQTT transport. */
  HOST = "HOST",
  /** Kubernetes: MQTT transport, ConfigMap-mounted config (Phase 1a). */
  KUBERNETES = "KUBERNETES",
}

/**
 * The messaging transport — the secondary runtime axis (DESIGN-core §2). Defaults from the resolved
 * {@link Platform} (GREENGRASS→IPC, HOST→MQTT) and is independently overridable, but constrained:
 * {@link Transport.IPC} is valid only on {@link Platform.GREENGRASS} (the Nucleus provides the IPC
 * socket). See {@link validate}.
 */
export enum Transport {
  /** Greengrass Nucleus IPC (domain socket). Requires {@link Platform.GREENGRASS}. */
  IPC = "IPC",
  /** Dual-MQTT (local broker + AWS IoT Core). The off-Nucleus transport. */
  MQTT = "MQTT",
}

/**
 * A platform profile: the table of per-subsystem defaults for a {@link Platform} (DESIGN-core §3).
 * Pure data; the resolver consults it only for settings the caller did not set explicitly. Phase 0
 * carries only the two defaultable settings the resolver actually injects — the default messaging
 * `transport` and the default `configSource` token.
 */
export interface PlatformProfile {
  /** The default messaging transport for this platform. */
  readonly transport: Transport;
  /** The default `-c/--config` source token (e.g. `"GG_CONFIG"`, `"FILE"`) when `-c` is omitted. */
  readonly configSource: string;
  /**
   * The default logging format for this platform, applied when the component config sets no
   * `logging.ts_format` (FR-LOG-1/FR-RT-3). `KUBERNETES` defaults to {@link JSON_LOG_FORMAT} (the
   * structured stdout-JSON sink); `GREENGRASS`/`HOST` leave this `undefined` so the library default
   * (console/text) is unchanged. Consulted by the logging configurator via {@link profileLoggingFormat}.
   */
  readonly loggingFormat?: string;
  /**
   * Whether the HTTP health endpoint is on by default for this platform when the component config sets
   * no explicit `health.enabled` (Phase 1c / FR-HB-1, precedence FR-RT-3). `true` on `KUBERNETES` (the
   * kubelet needs probes), `false`/absent on `GREENGRASS`/`HOST`. Consulted by the lifecycle builder via
   * {@link profileHealthEnabled}; an explicit `health.enabled` always wins.
   */
  readonly healthEnabled?: boolean;
}

/**
 * The fully resolved runtime settings consumed by the lifecycle builder (DESIGN-core §4). Produced
 * once, right after argument parse and before messaging init, from parse-time inputs only
 * (flags → env → messaging-config payload).
 */
export interface ResolvedProfile {
  /** The resolved platform (after auto-detection / explicit flag). */
  readonly platform: Platform;
  /** The resolved messaging transport (validated against the platform). */
  readonly transport: Transport;
  /** The resolved `-c/--config` token vector (explicit, else the profile default as `[token]`). */
  readonly configSource: string[];
  /** The resolved IoT Thing name (identity), never empty. */
  readonly identity: string;
}

/**
 * The parse-time inputs to the resolver. Any field may be `undefined`, meaning "not specified — fall
 * back to detection / the profile default".
 */
export interface ResolverInputs {
  /** Explicit `--platform` value, or `undefined` for `auto`. */
  readonly platform?: Platform;
  /** Explicit `--transport` value, or `undefined` to derive from the platform. */
  readonly transport?: Transport;
  /** Explicit `-c/--config` token vector, or `undefined` when `-c` is omitted. */
  readonly configArgs?: string[];
  /** Explicit `-t/--thing` value, or `undefined`. */
  readonly thing?: string;
}

/** An environment map (typically `process.env`). */
export type Env = Record<string, string | undefined>;

/** Nucleus-injected env var pointing at the IPC domain socket (definitive GREENGRASS signal). */
export const ENV_GG_IPC_SOCKET = "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT";
/** Nucleus-injected component service-UID (definitive GREENGRASS signal). */
export const ENV_GG_SVCUID = "SVCUID";
/** Greengrass-injected IoT Thing name (identity probe). */
export const ENV_THING_NAME = "AWS_IOT_THING_NAME";
/**
 * Kubernetes Downward-API thing-name env var (FR-RT-7). The chart maps the
 * `ggcommons.io/thing-name` pod annotation (or an explicit value) into this var. Honored only when
 * the resolved platform is {@link Platform.KUBERNETES}; it then takes precedence over
 * {@link ENV_THING_NAME}.
 */
export const ENV_K8S_THING_NAME = "GGCOMMONS_THING_NAME";
/**
 * Kubernetes Downward-API pod-name env var (`metadata.name` via `fieldRef`). The KUBERNETES identity
 * fallback after {@link ENV_K8S_THING_NAME}. Honored only when the resolved platform is
 * {@link Platform.KUBERNETES}. Also a logging correlation field (`pod`) on the stdout-JSON sink (FR-LOG-3).
 */
export const ENV_K8S_POD_NAME = "POD_NAME";
/**
 * Kubernetes Downward-API pod-namespace env var (`metadata.namespace` via `fieldRef`). Used only as a
 * logging correlation field (`namespace`) on the stdout-JSON sink (FR-LOG-3); same Downward-API wiring
 * as {@link ENV_K8S_POD_NAME} (Phase 1b).
 */
export const ENV_K8S_POD_NAMESPACE = "POD_NAMESPACE";
/**
 * Kubernetes Downward-API node-name env var (`spec.nodeName` via `fieldRef`). Used only as a logging
 * correlation field (`node`) on the stdout-JSON sink (FR-LOG-3).
 */
export const ENV_K8S_NODE_NAME = "NODE_NAME";

/**
 * The logging-format selector token that selects the structured stdout-JSON sink (FR-LOG-1/FR-LOG-4),
 * matched case-insensitively against `logging.<lang>_format` (TS: `logging.ts_format`). The KUBERNETES
 * profile's default logging format (see {@link PROFILES}). Any other token value is treated as the
 * existing console/text token template. Kept here (next to the profile default) as the single source of
 * truth; consumed by the logging configurator.
 */
export const JSON_LOG_FORMAT = "json";
/** Confirming (secondary) Kubernetes signal. The token file is the primary, definitive one. */
export const ENV_K8S_SERVICE_HOST = "KUBERNETES_SERVICE_HOST";
/** Projected service-account token path: the primary, definitive Kubernetes signal. */
export const K8S_SA_TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/** The library-default identity when no thing name is available (matches today's behavior). */
export const DEFAULT_IDENTITY = "NOT_GREENGRASS";

/**
 * The platform-profile table (DESIGN-core §3). GREENGRASS and HOST deliberately default the config
 * source to `GG_CONFIG` to preserve current behavior. KUBERNETES (Phase 1a) defaults to the `MQTT`
 * transport and the k8s-native `CONFIGMAP` config source.
 *
 * Phase 1c adds the KUBERNETES profile's default `loggingFormat` ({@link JSON_LOG_FORMAT}: the
 * structured stdout-JSON sink). TODO (Phase 1b–1d): the metrics/credentials/streaming defaults
 * (prometheus target, env KeyProvider, PVC buffer) are not yet modeled here — those subsystems keep
 * their current library defaults for now.
 */
export const PROFILES: ReadonlyMap<Platform, PlatformProfile> = new Map([
  [Platform.GREENGRASS, { transport: Transport.IPC, configSource: "GG_CONFIG" } as PlatformProfile],
  [Platform.HOST, { transport: Transport.MQTT, configSource: "GG_CONFIG" } as PlatformProfile],
  [
    Platform.KUBERNETES,
    {
      transport: Transport.MQTT,
      configSource: "CONFIGMAP",
      loggingFormat: JSON_LOG_FORMAT,
      healthEnabled: true,
    } as PlatformProfile,
  ],
]);

/**
 * The platform-profile default logging format for `platform` (FR-LOG-1/FR-RT-3), or `undefined` when
 * the profile pins no default (GREENGRASS/HOST → library default). Threaded into the logging
 * configurator so a KUBERNETES pod with no `logging.ts_format` config logs JSON, while explicit config
 * still wins. Pure lookup; the configurator owns the precedence
 * (explicit config ▸ this profile default ▸ library default).
 */
export function profileLoggingFormat(platform: Platform): string | undefined {
  return PROFILES.get(platform)?.loggingFormat;
}

/**
 * Whether the HTTP health endpoint is on by default for `platform` (Phase 1c / FR-HB-1), i.e. `true`
 * on KUBERNETES and `false` on GREENGRASS/HOST. Threaded into the lifecycle builder, which owns the
 * precedence (explicit `health.enabled` ▸ this profile default). Pure lookup; mirrors
 * {@link profileLoggingFormat}.
 */
export function profileHealthEnabled(platform: Platform): boolean {
  return PROFILES.get(platform)?.healthEnabled === true;
}

/**
 * Resolves the runtime profile from parse-time inputs and the environment (DESIGN-core §4).
 *
 * @throws {@link GgError} of kind `Cli` if the resolved platform has no profile in this build, or the
 *         platform/transport combination is illegal (the IPC lock).
 */
export function resolveProfile(inputs: ResolverInputs, env: Env): ResolvedProfile {
  const autoDetected = inputs.platform === undefined;
  const platform = autoDetected ? detectPlatform(env) : inputs.platform!;
  const basis = autoDetected ? "auto-detected" : "explicit --platform";

  const profile = PROFILES.get(platform);
  if (!profile) {
    throw GgError.cli(
      `Platform ${platform} is not supported in this build (no profile). ` +
        `Valid platforms: ${[...PROFILES.keys()].join(", ")}.`,
    );
  }

  const transport = inputs.transport ?? profile.transport;
  validate(platform, transport);

  const configSource = inputs.configArgs ?? [profile.configSource];
  const identity = resolveIdentity(inputs.thing, platform, env);

  logger.info(
    `Resolved platform=${platform} (basis=${basis}) transport=${transport} ` +
      `configSource=${configSource[0]} identity=${identity}`,
  );

  return { platform, transport, configSource, identity };
}

/**
 * Auto-detects the platform from the environment (DESIGN-core §5). Signal order is load-bearing: a
 * containerized Nucleus component can set both Greengrass and Kubernetes signals, and GREENGRASS must
 * win. First match wins; HOST is the fallback. The filesystem probe (Kubernetes SA token) is
 * injectable for tests.
 */
export function detectPlatform(env: Env, fileExists: (p: string) => boolean = existsSync): Platform {
  // 1. GREENGRASS — Nucleus-injected signals exist nowhere else (definitive).
  if (isSet(env, ENV_GG_IPC_SOCKET) || isSet(env, ENV_GG_SVCUID)) {
    return Platform.GREENGRASS;
  }
  // 2. KUBERNETES — projected SA token (primary); service host (confirming/secondary).
  if (fileExists(K8S_SA_TOKEN_PATH) || isSet(env, ENV_K8S_SERVICE_HOST)) {
    return Platform.KUBERNETES;
  }
  // 3. HOST — fallback.
  return Platform.HOST;
}

/**
 * Validates the platform/transport combination — the IPC lock (DESIGN-core §4.1). IPC is valid only
 * on a Greengrass Nucleus, which provides the IPC domain socket.
 *
 * @throws {@link GgError} of kind `Cli` if `transport === IPC && platform !== GREENGRASS`.
 */
export function validate(platform: Platform, transport: Transport): void {
  if (transport === Transport.IPC && platform !== Platform.GREENGRASS) {
    throw GgError.cli(
      `IPC transport requires --platform GREENGRASS (the Nucleus provides the IPC socket); ` +
        `got platform=${platform}`,
    );
  }
}

/**
 * Resolves the IoT Thing name / identity (DESIGN-core §6.2, FR-RT-7 / FR-CFG-6). Precedence:
 *
 *   1. explicit `-t/--thing` (highest, every platform);
 *   2. **KUBERNETES only** — the Downward-API env vars in order: {@link ENV_K8S_THING_NAME}
 *      (`GGCOMMONS_THING_NAME`, the chart-mapped `ggcommons.io/thing-name` annotation or an explicit
 *      value), then {@link ENV_K8S_POD_NAME} (`POD_NAME`, `metadata.name` via `fieldRef`);
 *   3. {@link ENV_THING_NAME} (`AWS_IOT_THING_NAME`, GREENGRASS / generic platform-supplied);
 *   4. the library fallback ({@link DEFAULT_IDENTITY}).
 *
 * The KUBERNETES tier (2) takes precedence over the generic AWS probe (3) **only** when
 * `platform === KUBERNETES`; on every other platform behavior is unchanged. A present-but-empty env
 * value is treated as absent (cross-language parity with the canonical Java/Python/Rust resolvers,
 * which only honor a non-empty thing name). The resolved value is later sanitized wherever it is
 * interpolated into a path or topic (see `config/template.ts`).
 */
export function resolveIdentity(
  thing: string | undefined,
  platform: Platform,
  env: Env | undefined,
): string {
  // (1) explicit -t/--thing wins everywhere.
  if (thing !== undefined) {
    return thing;
  }
  // (2) KUBERNETES Downward-API identity (precedence over the generic AWS probe only on k8s).
  if (platform === Platform.KUBERNETES) {
    const k8sThing = nonEmpty(env, ENV_K8S_THING_NAME) ?? nonEmpty(env, ENV_K8S_POD_NAME);
    if (k8sThing !== undefined) {
      return k8sThing;
    }
  }
  // (3) Greengrass / generic platform-supplied identity probe.
  const fromEnv = nonEmpty(env, ENV_THING_NAME);
  if (fromEnv !== undefined) {
    return fromEnv;
  }
  // (4) library fallback.
  return DEFAULT_IDENTITY;
}

function isSet(env: Env | undefined, key: string): boolean {
  return nonEmpty(env, key) !== undefined;
}

/** Return `env[key]` if present and non-empty, else `undefined` (treats `""` as absent). */
function nonEmpty(env: Env | undefined, key: string): string | undefined {
  if (!env) return undefined;
  const v = env[key];
  return v !== undefined && v !== "" ? v : undefined;
}
