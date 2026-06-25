/**
 * CLI — the standard command-line contract shared verbatim across the Java,
 * Python, Rust, and TS libraries (DESIGN-core §6).
 *
 * - `--platform GREENGRASS | HOST | KUBERNETES | auto` (default `auto`) — the primary runtime axis.
 * - `--transport IPC | MQTT [messaging_config.json]` — secondary axis; defaults from the platform.
 * - `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT`
 *   (default: from the resolved platform profile).
 * - `-t/--thing <name>` — IoT Thing name (takes the **full** string value; default: platform identity).
 *
 * The legacy single-axis `-m/--mode` flag is removed (FR-RT-1): passing it errors with guidance to
 * `--platform`/`--transport`. After parsing the raw flags, {@link parseArgs} runs the precedence
 * resolver ({@link resolveProfile}) so the returned {@link ParsedArgs} already carries the two
 * resolved axes plus the resolved config source and identity. Parse failures surface as {@link GgError}
 * of kind `Cli`.
 */
import { GgError } from "./errors";
import {
  Env,
  Platform,
  ResolverInputs,
  Transport,
  resolveProfile,
} from "./platform";

/** Configuration source selected by `-c/--config`. */
export type ConfigSourceSpec =
  | { kind: "FILE"; path: string }
  | { kind: "ENV"; var: string }
  | { kind: "GG_CONFIG"; component?: string; key: string }
  | { kind: "SHADOW"; name?: string }
  | { kind: "CONFIG_COMPONENT" };

/** Parsed + resolved standard arguments (after the platform/transport resolver has run). */
export interface ParsedArgs {
  /** Resolved deployment platform (the primary runtime axis). */
  platform: Platform;
  /** Resolved messaging transport (derived from the platform unless overridden). */
  transport: Transport;
  /** Resolved configuration source. */
  config: ConfigSourceSpec;
  /** The MQTT messaging-config file path (the `--transport MQTT <path>` payload), if supplied. */
  messagingConfigPath?: string;
  /** Resolved IoT Thing name (identity), never undefined. */
  thing: string;
}

const DEFAULT_CONFIG_FILE = "config.json";
const DEFAULT_ENV_VAR = "CONFIG";
const DEFAULT_GG_CONFIG_KEY = "ComponentConfig";

/**
 * Parse the standard arguments from an argv-style array (NOT including the program name — pass
 * `process.argv.slice(2)`), then resolve the platform/transport axes against `env` (default
 * `process.env`, mirroring the Java `processArgs` reading `System.getenv()`).
 */
export function parseArgs(argv: string[], env: Env = process.env): ParsedArgs {
  rejectLegacyModeFlag(argv);

  let configTokens: string[] | undefined;
  let platformFlag: Platform | undefined;
  let transportFlag: Transport | undefined;
  let messagingConfigPath: string | undefined;
  let thing: string | undefined;

  let i = 0;
  while (i < argv.length) {
    const arg = argv[i];
    if (arg === "-c" || arg === "--config") {
      configTokens = takeVariadic(argv, i + 1, 3);
      i += 1 + configTokens.length;
    } else if (arg === "--platform") {
      const tokens = takeVariadic(argv, i + 1, 1);
      platformFlag = parsePlatform(tokens[0]);
      i += 1 + tokens.length;
    } else if (arg === "--transport") {
      const tokens = takeVariadic(argv, i + 1, 2);
      transportFlag = parseTransport(tokens[0]);
      if (tokens.length > 1) messagingConfigPath = tokens[1];
      i += 1 + tokens.length;
    } else if (arg === "-t" || arg === "--thing") {
      const next = argv[i + 1];
      if (next === undefined || isFlag(next)) {
        throw GgError.cli("-t/--thing requires a value");
      }
      thing = next; // full string value, never truncated
      i += 2;
    } else if (arg === "-h" || arg === "--help") {
      i += 1;
    } else {
      throw GgError.cli(`unexpected argument '${arg}'`);
    }
  }

  // Resolve the two runtime axes + the default config provider + identity from parse-time inputs only
  // (DESIGN-core §4 / §4.2). Validation failures (e.g. the IPC lock, KUBERNETES) propagate as Cli.
  const inputs: ResolverInputs = {
    platform: platformFlag,
    transport: transportFlag,
    configArgs: configTokens,
    thing,
  };
  const resolved = resolveProfile(inputs, env);

  return {
    platform: resolved.platform,
    transport: resolved.transport,
    config: parseConfigSource(resolved.configSource),
    messagingConfigPath,
    thing: resolved.identity,
  };
}

/** Rejects the removed `-m`/`--mode` flag with guidance to the new axes. */
function rejectLegacyModeFlag(argv: string[]): void {
  for (const arg of argv) {
    if (arg === "-m" || arg === "--mode") {
      throw GgError.cli(
        "The -m/--mode flag has been removed. Use --platform GREENGRASS|HOST|KUBERNETES and " +
          "--transport IPC|MQTT instead (e.g. '-m STANDALONE <path>' becomes " +
          "'--platform HOST --transport MQTT <path>').",
      );
    }
  }
}

/** Parses `--platform`; `auto` yields `undefined` so the resolver auto-detects. */
function parsePlatform(raw: string): Platform | undefined {
  const v = raw.trim();
  if (v.toLowerCase() === "auto") {
    return undefined;
  }
  const upper = v.toUpperCase();
  if (upper in Platform) {
    return Platform[upper as keyof typeof Platform];
  }
  throw GgError.cli(`unknown platform '${raw}'. Valid: GREENGRASS, HOST, KUBERNETES, auto.`);
}

/** Parses `--transport` (`IPC`|`MQTT`). */
function parseTransport(raw: string): Transport {
  const upper = raw.trim().toUpperCase();
  if (upper in Transport) {
    return Transport[upper as keyof typeof Transport];
  }
  throw GgError.cli(`unknown transport '${raw}'. Valid: IPC, MQTT.`);
}

/** Collect up to `max` non-flag tokens starting at `start` (the variadic value list). */
function takeVariadic(argv: string[], start: number, max: number): string[] {
  const out: string[] = [];
  for (let j = start; j < argv.length && out.length < max; j++) {
    if (isFlag(argv[j])) break;
    out.push(argv[j]);
  }
  if (out.length === 0) {
    throw GgError.cli(`option at position ${start - 1} requires a value`);
  }
  return out;
}

function isFlag(token: string): boolean {
  return token.startsWith("-") && token.length > 1 && !/^-?\d/.test(token);
}

function parseConfigSource(args: string[]): ConfigSourceSpec {
  const source = args[0].toUpperCase();
  switch (source) {
    case "FILE":
      return { kind: "FILE", path: args[1] ?? DEFAULT_CONFIG_FILE };
    case "ENV":
      return { kind: "ENV", var: args[1] ?? DEFAULT_ENV_VAR };
    case "GG_CONFIG":
      return { kind: "GG_CONFIG", component: args[1], key: args[2] ?? DEFAULT_GG_CONFIG_KEY };
    case "SHADOW":
      return { kind: "SHADOW", name: args[1] };
    case "CONFIG_COMPONENT":
      return { kind: "CONFIG_COMPONENT" };
    default:
      throw GgError.cli(`unknown config source '${source}'`);
  }
}
