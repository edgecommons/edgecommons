/**
 * Configuration — template substitution.
 *
 * Resolves `{ThingName}`, `{ComponentName}` (short name — the segment after the
 * last `.`), `{ComponentFullName}`, and any `tags` key inside config strings (log
 * file paths, MQTT topics). Mirrors the Java/Python/Rust substitution exactly.
 *
 * Substituted *values* are sanitized (path separators `/`,`\`, MQTT wildcards
 * `+`,`#`, control chars, and `..` traversal → `_`) so a hostile value cannot break
 * out of the path or topic it is interpolated into. The template literal itself is
 * left intact, so legitimate separators in the template are preserved. Unknown
 * placeholders are left untouched.
 */
import type { Config } from "./model";

/** Replace known placeholders in `template` using values from `config`. */
export function resolve(config: Config, template: string): string {
  const shortName = config.componentName.includes(".")
    ? config.componentName.slice(config.componentName.lastIndexOf(".") + 1)
    : config.componentName;

  let out = template
    .split("{ThingName}").join(sanitize(config.thingName))
    .split("{ComponentFullName}").join(sanitize(config.componentName))
    .split("{ComponentName}").join(sanitize(shortName));

  for (const [key, value] of Object.entries(config.parsed.tags)) {
    if (typeof value === "string") {
      out = out.split(`{${key}}`).join(sanitize(value));
    }
  }
  return out;
}

/**
 * Neutralize characters dangerous in a file path or MQTT topic: path separators,
 * MQTT wildcards, control chars, and `..` traversal are each replaced with `_`.
 */
function sanitize(value: string): string {
  let out = "";
  for (const ch of value) {
    const code = ch.charCodeAt(0);
    if (ch === "/" || ch === "\\" || ch === "+" || ch === "#" || code < 0x20 || code === 0x7f) {
      out += "_";
    } else {
      out += ch;
    }
  }
  return out.split("..").join("_");
}
