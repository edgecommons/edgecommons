/**
 * Configuration — JSON-schema validation (fail-closed).
 *
 * The cross-language config schema is embedded ({@link ./schema.json}) so it can
 * never be "missing from the classpath" (closing the Java fail-open hole). A
 * validation failure is a hard error, mirroring the Rust/Python behavior. Uses
 * `ajv` (the standard JS JSON-schema validator).
 */
import Ajv, { ValidateFunction } from "ajv";

import { EdgeCommonsError } from "../errors";
import schema from "./schema.json";

let validator: ValidateFunction | undefined;

function compiled(): ValidateFunction {
  if (!validator) {
    const ajv = new Ajv({ allErrors: true, strict: false });
    validator = ajv.compile(schema as object);
  }
  return validator;
}

/**
 * Validate a raw config document against the embedded schema. Throws
 * {@link EdgeCommonsError} of kind `Validation` listing every error on failure.
 */
export function validate(instance: unknown): void {
  const v = compiled();
  if (!v(instance)) {
    const errors = (v.errors ?? [])
      .map((e) => `${e.instancePath || "(root)"} ${e.message}`)
      .join("; ");
    throw EdgeCommonsError.validation(`config failed schema validation: ${errors}`);
  }
}
