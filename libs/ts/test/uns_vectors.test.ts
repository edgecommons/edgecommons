/**
 * The TS loader for the cross-language `uns-test-vectors/` conformance suite
 * (UNS-CANONICAL-DESIGN §7 / D-U12/D-U13/D-U22), mirroring the Java
 * `UnsTestVectorsLoaderTest`/`UnsTestVectors` pair and the vault-vectors TS loader pattern.
 *
 * Reads the committed vector files and replays every case through the LIVE implementation:
 * - build/validate/filter cases through `Uns` — topics/filters byte-for-byte, failures by the
 *   exact machine-readable `UnsValidationError.code`;
 * - guard cases through the §4.1 `reservedClassOf` predicate;
 * - golden envelopes rebuilt through `MessageBuilder` (pinned uuid/timestamp/correlation_id +
 *   the vector identity parsed by the lenient wire parser) and compared STRUCTURALLY (member
 *   order is not normative — `toEqual` on parsed objects is order-insensitive, D-U22); the
 *   vector `topic` is also rebuilt byte-for-byte (includeRoot=false).
 *
 * Existence-guarded like the vault loader: skips when the vector directory is absent (the
 * files are committed, so CI always exercises this). Some vector inputs carry raw C1 control
 * bytes — the files are parsed as JSON, never preprocessed.
 */
import { existsSync, readFileSync } from "fs";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { sanitize } from "../src/config/template";
import { MessageBuilder, MessageIdentity } from "../src/message";
import type { HierLevel } from "../src/message";
import { Uns, UnsScope, UnsValidationError, reservedClassOf, unsClassFromToken } from "../src/uns";

const VECTORS = join(__dirname, "..", "..", "..", "uns-test-vectors");
const present = existsSync(join(VECTORS, "topics.json")) && existsSync(join(VECTORS, "envelopes.json"));

function load(name: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(VECTORS, name), "utf8")) as Record<string, unknown>;
}

interface VectorCase {
  name: string;
  input: Record<string, unknown>;
  expected: Record<string, unknown>;
}

/**
 * The binding identity for validate/filter cases: MULTI-level so the case's `includeRoot`
 * input is the effective root mode (D-U25). Mirrors the Java `UnsTestVectors.BINDING`.
 */
const BINDING = new MessageIdentity(
  [
    { level: "site", value: "dallas" },
    { level: "device", value: "gw-01" },
  ],
  "opcua-adapter",
  "main",
);

/** Runs one build case: sanitize identityValues/component (the config resolution path, D-U26),
 * construct the identity, build the topic. Returns `{topic}` or `{error}`. */
function runBuild(input: Record<string, unknown>): Record<string, unknown> {
  const values = input.identityValues as Record<string, string>;
  const hier: HierLevel[] = (input.hierarchyLevels as string[]).map((level) => ({
    level,
    value: sanitize(values[level]),
  }));
  const identity = new MessageIdentity(hier, sanitize(input.component as string), input.instance as string);
  const cls = unsClassFromToken(input.class as string);
  expect(cls, `build input class token '${String(input.class)}'`).toBeDefined();
  const channel = "channel" in input ? (input.channel as string) : undefined;
  try {
    return { topic: new Uns(identity, input.includeRoot as boolean).topic(cls!, channel) };
  } catch (e) {
    return errorOf(e);
  }
}

/** Runs one validate case (bound to the multi-level BINDING identity). */
function runValidate(input: Record<string, unknown>): Record<string, unknown> {
  try {
    new Uns(BINDING, input.includeRoot as boolean).validate(input.topic as string);
    return { ok: true };
  } catch (e) {
    return errorOf(e);
  }
}

/** Runs one filter case (absent scope fields render as `+`). */
function runFilter(input: Record<string, unknown>): Record<string, unknown> {
  const scope = (input.scope ?? {}) as UnsScope;
  const cls = unsClassFromToken(input.class as string);
  expect(cls, `filter input class token '${String(input.class)}'`).toBeDefined();
  return { filter: new Uns(BINDING, input.includeRoot as boolean).filter(cls!, scope) };
}

/** Runs one guard case through the §4.1 reserved-class predicate (D-U24). */
function runGuard(input: Record<string, unknown>): Record<string, unknown> {
  return { reserved: reservedClassOf(input.topic as string, input.includeRoot as boolean) !== undefined };
}

function errorOf(e: unknown): Record<string, unknown> {
  expect(e).toBeInstanceOf(UnsValidationError);
  return { error: (e as UnsValidationError).code };
}

describe.skipIf(!present)("uns-test-vectors cross-language conformance", () => {
  it("topics.json: every build case matches byte-for-byte / by exact error code", () => {
    const doc = load("topics.json");
    const cases = doc.build as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      expect(runBuild(c.input), `build case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("topics.json: every validate case matches (includeRoot-sensitive, D-U25)", () => {
    const doc = load("topics.json");
    const cases = doc.validate as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      expect(runValidate(c.input), `validate case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("topics.json: every filter case matches byte-for-byte", () => {
    const doc = load("topics.json");
    const cases = doc.filter as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      expect(runFilter(c.input), `filter case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("topics.json: every guard case matches the §4.1 reserved predicate", () => {
    const doc = load("topics.json");
    const cases = doc.guard as VectorCase[];
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      expect(runGuard(c.input), `guard case '${c.name}'`).toEqual(c.expected);
    }
  });

  it("envelopes.json: every golden envelope is reproduced structurally + its topic byte-for-byte", () => {
    const doc = load("envelopes.json");
    const cases = doc.envelopes as Array<{
      name: string;
      class: string;
      channel?: string;
      topic: string;
      envelope: Record<string, unknown>;
    }>;
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      const envelope = c.envelope;
      const header = envelope.header as Record<string, string>;

      // The vector identity parses through the lenient wire parser.
      const identity = MessageIdentity.fromObject(envelope.identity);
      expect(identity, `envelope '${c.name}' identity must parse`).toBeDefined();

      // Topic reproduction, byte-for-byte (all envelope vectors are rootless).
      const cls = unsClassFromToken(c.class);
      expect(cls, `envelope '${c.name}' class token`).toBeDefined();
      expect(new Uns(identity!, false).topic(cls!, c.channel), `envelope '${c.name}' topic`).toBe(c.topic);

      // Envelope reproduction through the single stamping site, compared STRUCTURALLY
      // (toEqual is member-order-insensitive - D-U22).
      const rebuilt = MessageBuilder.create(header.name, header.version)
        .withUuid(header.uuid)
        .withTimestamp(header.timestamp)
        .withCorrelationId(header.correlation_id)
        .withIdentity(identity!)
        .withPayload(envelope.body)
        .build();
      expect(rebuilt.toObject(), `envelope '${c.name}'`).toEqual(envelope);
      // And the parse direction: the wire JSON round-trips to the same structure.
      expect(JSON.parse(rebuilt.toJSON()), `envelope '${c.name}' (wire)`).toEqual(envelope);
    }
  });
});
