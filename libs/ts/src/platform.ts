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
 * **Phase 0:** only {@link Platform.GREENGRASS} and {@link Platform.HOST} have profiles, and both
 * default their config source to `GG_CONFIG` (a faithful re-expression of today's behavior — HOST
 * does NOT flip to `FILE` until Phase 1). Resolving to {@link Platform.KUBERNETES} fails fast.
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
  /** Kubernetes (declared for Phase 0; profile populated in Phase 1). */
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
/** Confirming (secondary) Kubernetes signal. The token file is the primary, definitive one. */
export const ENV_K8S_SERVICE_HOST = "KUBERNETES_SERVICE_HOST";
/** Projected service-account token path: the primary, definitive Kubernetes signal. */
export const K8S_SA_TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/** The library-default identity when no thing name is available (matches today's behavior). */
export const DEFAULT_IDENTITY = "NOT_GREENGRASS";

/**
 * The platform-profile table (DESIGN-core §3). Phase 0 populates only GREENGRASS and HOST; both
 * deliberately default the config source to `GG_CONFIG` to preserve current behavior. KUBERNETES is
 * intentionally absent (declared enum value, no profile yet).
 */
export const PROFILES: ReadonlyMap<Platform, PlatformProfile> = new Map([
  [Platform.GREENGRASS, { transport: Transport.IPC, configSource: "GG_CONFIG" } as PlatformProfile],
  [Platform.HOST, { transport: Transport.MQTT, configSource: "GG_CONFIG" } as PlatformProfile],
]);

/**
 * Resolves the runtime profile from parse-time inputs and the environment (DESIGN-core §4).
 *
 * @throws {@link GgError} of kind `Cli` if the resolved platform has no Phase-0 profile (KUBERNETES),
 *         or the platform/transport combination is illegal (the IPC lock).
 */
export function resolveProfile(inputs: ResolverInputs, env: Env): ResolvedProfile {
  const autoDetected = inputs.platform === undefined;
  const platform = autoDetected ? detectPlatform(env) : inputs.platform!;
  const basis = autoDetected ? "auto-detected" : "explicit --platform";

  const profile = PROFILES.get(platform);
  if (!profile) {
    throw GgError.cli(
      `Platform ${platform} is not supported in this build (no profile). ` +
        `Valid platforms: GREENGRASS, HOST. (KUBERNETES ships in Phase 1.)`,
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
 * Resolves the IoT Thing name / identity (DESIGN-core §6.2). Order: explicit `-t/--thing`, then the
 * `AWS_IOT_THING_NAME` env probe, then the library fallback. For Phase 0 the GREENGRASS and HOST
 * platforms share the same probe, so behavior is unchanged; KUBERNETES Downward-API identity is
 * Phase 1.
 */
export function resolveIdentity(
  thing: string | undefined,
  _platform: Platform,
  env: Env | undefined,
): string {
  if (thing !== undefined) {
    return thing;
  }
  const fromEnv = env ? env[ENV_THING_NAME] : undefined;
  // Treat a present-but-empty env value as absent (cross-language parity with the
  // canonical Java/Python/Rust resolvers, which only honor a non-empty thing name).
  if (fromEnv !== undefined && fromEnv !== "") {
    return fromEnv;
  }
  return DEFAULT_IDENTITY;
}

function isSet(env: Env | undefined, key: string): boolean {
  if (!env) return false;
  const v = env[key];
  return v !== undefined && v !== "";
}
