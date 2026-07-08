/**
 * Configuration — template substitution.
 *
 * Resolves `{ThingName}`, `{ComponentName}` (short name — the segment after the
 * last `.`), `{ComponentFullName}`, hierarchy identity level names, and any
 * `tags` key inside config strings (log file paths, MQTT topics). Mirrors the
 * Java/Python/Rust substitution exactly.
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

  for (const entry of config.componentIdentity.hier) {
    const placeholder = `{${entry.level}}`;
    if (out.includes(placeholder)) {
      out = out.split(placeholder).join(sanitize(entry.value));
    }
  }

  for (const [key, value] of Object.entries(config.parsed.tags)) {
    const placeholder = `{${key}}`;
    if (typeof value === "string" && out.includes(placeholder)) {
      out = out.split(placeholder).join(sanitize(value));
    }
  }
  return out;
}

/**
 * Neutralize characters dangerous in a file path or MQTT topic: path separators,
 * MQTT wildcards, control chars, and `..` traversal are each replaced with `_`.
 *
 * Exported because this is also the **normative UNS token sanitizer**
 * (UNS-CANONICAL-DESIGN §2.2 rule 1 / D-U26): the `uns()` token rule is exactly this
 * blacklist, so "sanitized ⇒ publishable" holds. Control characters follow Java's
 * `Character.isISOControl` — C0 (U+0000–U+001F), DEL (U+007F) **and C1
 * (U+0080–U+009F)**. Identity values and metric-name channels pass through here.
 */
export function sanitize(value: string): string {
  let out = "";
  for (const ch of value) {
    const code = ch.charCodeAt(0);
    if (ch === "/" || ch === "\\" || ch === "+" || ch === "#" || isIsoControl(code)) {
      out += "_";
    } else {
      out += ch;
    }
  }
  return out.split("..").join("_");
}

/**
 * Java `Character.isISOControl` equivalence (D-U26): C0 U+0000–U+001F, U+007F DEL,
 * and C1 U+0080–U+009F. The exact predicate the UNS token rule shares.
 */
export function isIsoControl(code: number): boolean {
  return code < 0x20 || (code >= 0x7f && code <= 0x9f);
}
