import { logger } from "../logging";

export type JsonObject = Record<string, unknown>;

export interface MergeWarning {
  path: string;
  code: "TYPE_CONFLICT_LATER_LAYER_WINS";
}

export interface MergeResult {
  effective: JsonObject;
  warnings: MergeWarning[];
}

export function isJsonObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function cloneJson<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((item) => cloneJson(item)) as T;
  }
  if (isJsonObject(value)) {
    const out: JsonObject = {};
    for (const [key, item] of Object.entries(value)) {
      out[key] = cloneJson(item);
    }
    return out as T;
  }
  return value;
}

export function deepMerge(layers: JsonObject[]): MergeResult {
  const warnings: MergeWarning[] = [];
  let result: unknown = {};
  for (const layer of layers) {
    result = mergeValue(result, layer, "$", warnings);
  }
  return { effective: result as JsonObject, warnings };
}

function mergeValue(left: unknown, right: unknown, path: string, warnings: MergeWarning[]): unknown {
  if (isJsonObject(left) && isJsonObject(right)) {
    const out: JsonObject = {};
    for (const [key, value] of Object.entries(left)) {
      out[key] = cloneJson(value);
    }
    for (const [key, value] of Object.entries(right)) {
      out[key] = Object.prototype.hasOwnProperty.call(out, key)
        ? mergeValue(out[key], value, `${path}.${key}`, warnings)
        : cloneJson(value);
    }
    return out;
  }

  if (shouldWarnTypeConflict(left, right)) {
    warnings.push({ path, code: "TYPE_CONFLICT_LATER_LAYER_WINS" });
    logger.warn(`hierarchical config type conflict at ${path}; later layer wins`);
  }
  return cloneJson(right);
}

function shouldWarnTypeConflict(left: unknown, right: unknown): boolean {
  if (left === undefined || right === undefined) return false;
  if (Array.isArray(left) || Array.isArray(right)) return false;
  if (left === null || right === null) return false;
  return jsonType(left) !== jsonType(right);
}

function jsonType(value: unknown): string {
  return isJsonObject(value) ? "object" : typeof value;
}
