/**
 * CLI — the standard command-line contract shared verbatim across the Java,
 * Python, Rust, and TS libraries.
 *
 * - `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
 * - `-m/--mode <MODE> [path]` — `GREENGRASS (default) | STANDALONE <messaging_config.json>`
 * - `-t/--thing <name>` — IoT Thing name (takes the **full** string value)
 *
 * The variadic `-c`/`-m` options mirror the Java `configArgs[]` array: the first
 * token selects the source/mode, the rest are source-specific. STANDALONE without a
 * path is a hard error; `-t` is never truncated (guards a historical bug). Parse
 * failures surface as {@link GgError} of kind `Cli`.
 */
import { GgError } from "./errors";

/** Runtime mode selected by `-m/--mode`. */
export type RuntimeMode =
  | { kind: "GREENGRASS" }
  | { kind: "STANDALONE"; messagingConfigPath: string };

/** Configuration source selected by `-c/--config`. */
export type ConfigSourceSpec =
  | { kind: "FILE"; path: string }
  | { kind: "ENV"; var: string }
  | { kind: "GG_CONFIG"; component?: string; key: string }
  | { kind: "SHADOW"; name?: string }
  | { kind: "CONFIG_COMPONENT" };

/** Parsed standard arguments. */
export interface ParsedArgs {
  mode: RuntimeMode;
  config: ConfigSourceSpec;
  thing?: string;
}

const DEFAULT_CONFIG_FILE = "config.json";
const DEFAULT_ENV_VAR = "CONFIG";
const DEFAULT_GG_CONFIG_KEY = "ComponentConfig";

/**
 * Parse the standard arguments from an argv-style array (NOT including the program
 * name — pass `process.argv.slice(2)`).
 */
export function parseArgs(argv: string[]): ParsedArgs {
  let configTokens: string[] | undefined;
  let modeTokens: string[] | undefined;
  let thing: string | undefined;

  let i = 0;
  while (i < argv.length) {
    const arg = argv[i];
    if (arg === "-c" || arg === "--config") {
      configTokens = takeVariadic(argv, i + 1, 3);
      i += 1 + configTokens.length;
    } else if (arg === "-m" || arg === "--mode") {
      modeTokens = takeVariadic(argv, i + 1, 2);
      i += 1 + modeTokens.length;
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

  const config = configTokens ? parseConfigSource(configTokens) : { kind: "GG_CONFIG" as const, key: DEFAULT_GG_CONFIG_KEY };
  const mode = modeTokens ? parseMode(modeTokens) : { kind: "GREENGRASS" as const };
  return { mode, config, thing };
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

function parseMode(args: string[]): RuntimeMode {
  const mode = args[0].toUpperCase();
  switch (mode) {
    case "GREENGRASS":
      return { kind: "GREENGRASS" };
    case "STANDALONE": {
      const path = args[1];
      if (!path) {
        throw GgError.cli("STANDALONE mode requires a messaging config file path");
      }
      return { kind: "STANDALONE", messagingConfigPath: path };
    }
    default:
      throw GgError.cli(`unknown mode '${mode}'`);
  }
}
