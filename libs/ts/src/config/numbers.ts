/**
 * Configuration — the numeric contract (D-NC1…D-NC4, `docs/platform/DESIGN-config-numeric-canonicalization.md`).
 *
 * Configuration stores do not agree on how they encode a JSON number. A file or a ConfigMap
 * carries the literal `5000`; a store that round-trips numbers through a 64-bit float — the
 * Greengrass config store is one, being a Java process that keeps JSON numbers as `Double` —
 * hands back `5000.0` for the same setting.
 *
 * **JavaScript cannot represent that difference (D-NC3).** There is one number type, so the
 * decoded `5000.0` *is* `5000`: `JSON.parse('5000.0') === 5000`, and it serializes back as
 * `5000`. Every configuration source in this library parses JSON (`JSON.parse` for FILE / ENV /
 * CONFIGMAP / SHADOW / CONFIG_COMPONENT, and the Greengrass IPC SDK's own `JSON.parse` of the
 * eventstream payload for GG_CONFIG), so the document a component receives is already canonical
 * for every integral value. There is deliberately **no canonicalization pass here** — the pass the
 * other three languages run at their intake boundary would be a no-op, and pretending otherwise
 * would be misleading. What TypeScript shares with them is the rule below.
 *
 * ## The rule (D-NC2)
 *
 * An integer-typed setting accepts an integral value in **any** numeric encoding — `5000`,
 * `5000.0`, and `5e3` are the same setting. A value the author did not write is **never**
 * substituted: a fractional value is not truncated, a negative value is not clamped to zero or to
 * the schema default, and an out-of-range value is not saturated. Those are rejected loudly, with
 * a message naming the setting and the offending value.
 *
 * Silently rewriting someone's configuration is a worse defect than refusing to start.
 *
 * Strings and booleans are never coerced, however numeric they look (D-NC4): coercing `"5000"`
 * would corrupt legitimately-string settings such as version literals, and no store this library
 * targets stringifies numbers.
 *
 * ## Two readers, one rule
 *
 * - {@link requireNonNegativeInteger} — for a setting with an error channel. Rejection surfaces as
 *   a startup failure at `Config.fromValue`, and as a rejected candidate (the previous generation
 *   is retained) on hot reload. Mirrors Java's `ConfigNumbers.requireNonNegativeInt` and Rust's
 *   `de_integral_u64` / `de_integral_opt_u64`.
 * - {@link asNonNegativeInteger} — for a free-form probe that has no error channel and whose
 *   documented contract is to fall back to its schema default (the `metricEmission.targetConfig`
 *   accessors). Mirrors Rust's `as_integral_u64`. It shares the rule, so it falls back to the
 *   default instead of truncating `5.5` to `5` or clamping `-5` to `0`.
 *
 * ## The 64-bit window, and JavaScript's own ceiling
 *
 * The shared rule spans `[-2^63, 2^64 - 1]`; these readers cover its non-negative half, so a value
 * at or above `2^64` is rejected exactly as the other languages reject it.
 *
 * JavaScript cannot represent integers beyond `2^53` exactly. That limit is pre-existing and
 * inherent — it is the same double-precision ceiling the Greengrass store itself imposes, so
 * TypeScript is not the weakest link on that path. A value above `2^53` is delivered as the
 * nearest representable double, by `JSON.parse`, before any of this code runs; it is a documented
 * platform limit, not something this module can repair or should reject.
 */
import { EdgeCommonsError } from "../errors";

/** `2^64` — the exclusive upper bound of the shared unsigned 64-bit window. */
const UNSIGNED_64_UPPER_EXCLUSIVE = 18446744073709551616;

/** Uniform prefix for every numeric-configuration rejection (the Java wording is the reference). */
const ERROR_PREFIX = "configuration value ";

/**
 * Reads a configuration number as a non-negative integer, rejecting anything the author did not
 * actually write.
 *
 * Accepts an integral value in any numeric encoding: `5`, `5.0`, and `5e0` are the same value and
 * all parse. Returns `undefined` when the setting is absent (`undefined`) or `null`, leaving the
 * caller to apply its schema default.
 *
 * Rejects, with an {@link EdgeCommonsError} of kind `Config` naming `path`:
 *
 * - a non-number (string, boolean, object, array);
 * - `NaN` or an infinity;
 * - a fractional value — it is **never** truncated;
 * - a negative value — it is **never** clamped to zero or to a default;
 * - a value at or above `2^64`, outside the shared 64-bit window.
 *
 * @param value the configured value (may be `undefined`)
 * @param path  the dotted configuration path used in the error message, e.g. `heartbeat.intervalSecs`
 */
export function requireNonNegativeInteger(value: unknown, path: string): number | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "number") {
    throw reject(path, `must be a number, but was ${describe(value)}`);
  }
  if (!Number.isFinite(value)) {
    throw reject(path, `must be a finite number, but was ${String(value)}`);
  }
  if (!Number.isInteger(value)) {
    throw reject(path, `must be a whole number, but was ${String(value)}`);
  }
  if (value < 0) {
    throw reject(path, `must not be negative, but was ${String(value)}`);
  }
  if (value >= UNSIGNED_64_UPPER_EXCLUSIVE) {
    throw reject(path, `is out of range for a 64-bit integer: ${String(value)}`);
  }
  return value;
}

/**
 * Reads a configuration number as a non-negative integer for a probe that has no error channel:
 * `undefined` for an absent, fractional, negative, out-of-range, or non-numeric value.
 *
 * Used by the free-form `metricEmission.targetConfig` accessors, whose documented contract is to
 * fall back to their schema default rather than to fail. It shares the rule of
 * {@link requireNonNegativeInteger}, so it never truncates `5.5` to `5` and never clamps `-5` to
 * `0` on the way to that default.
 */
export function asNonNegativeInteger(value: unknown): number | undefined {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= 0 &&
    value < UNSIGNED_64_UPPER_EXCLUSIVE
    ? value
    : undefined;
}

/** Builds the uniform rejection. */
function reject(path: string, detail: string): EdgeCommonsError {
  return EdgeCommonsError.config(`${ERROR_PREFIX}'${path}' ${detail}`);
}

/** Renders a rejected value for an error message without ever throwing. */
function describe(value: unknown): string {
  if (Array.isArray(value)) return "an array";
  if (typeof value === "object") return "an object";
  if (typeof value === "string") return JSON.stringify(value);
  return String(value);
}
