/**
 * Secret references (`$secret`) in config.
 *
 * Lets any subsystem's config point at a vault secret instead of embedding the value —
 * `{"$secret": "name"}` (whole value) or `{"$secret": "name", "field": "key"}` (a field of the
 * secret's JSON). Resolved at subsystem-init time so the secret never lands in a logged/templated
 * config snapshot. Mirrors the Rust `resolve_secret_refs` (TELEMETRY_STREAMING.md §7).
 */
import { CredentialError } from "./errors";
import { CredentialService } from "./service";

/**
 * Recursively replace `$secret` references in `value` with values resolved from `creds`.
 *
 * Operates on a deep clone — the input is never mutated. Throws {@link CredentialError} if a
 * referenced secret (or requested field) is absent.
 */
export function resolveSecretRefs(value: unknown, creds: CredentialService): unknown {
  return walk(value, creds);
}

function walk(value: unknown, creds: CredentialService): unknown {
  if (Array.isArray(value)) {
    return value.map((v) => walk(v, creds));
  }
  if (value !== null && typeof value === "object") {
    const obj = value as Record<string, unknown>;
    const ref = obj["$secret"];
    if (typeof ref === "string") {
      const field = typeof obj["field"] === "string" ? (obj["field"] as string) : undefined;
      return resolveOne(ref, field, creds);
    }
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(obj)) {
      out[k] = walk(v, creds);
    }
    return out;
  }
  return value;
}

function resolveOne(name: string, field: string | undefined, creds: CredentialService): string {
  const secret = creds.get(name);
  if (!secret) {
    throw new CredentialError(`secretRef '${name}' not found in the vault`);
  }
  if (field === undefined) {
    return secret.asString();
  }
  const json = secret.asJson();
  if (json !== null && typeof json === "object" && !Array.isArray(json)) {
    const v = (json as Record<string, unknown>)[field];
    if (typeof v === "string") {
      return v;
    }
  }
  throw new CredentialError(`secretRef '${name}' field '${field}' missing or not a string`);
}
