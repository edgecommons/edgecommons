import { MessageBuilder } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import { CountPerTick, FieldEquals, Pipeline, ProcMsg, buildStage, pluck } from "../src/proc";

const msg = (body: unknown): ProcMsg => ({
  topic: "ecv1/gw/x/main/data/t",
  msg: MessageBuilder.create("T", "1.0").withPayload(body).build(),
});

describe("the pipeline", () => {
  it("drops what a filter stage does not match", () => {
    const p = new Pipeline([new FieldEquals("quality", "GOOD")]);

    expect(p.run([msg({ quality: "GOOD" })])).toHaveLength(1);
    // A filter that does not match emits nothing.
    expect(p.run([msg({ quality: "BAD" })])).toHaveLength(0);
  });

  it("lets a stateful stage emit on the tick, not on arrival", () => {
    const p = new Pipeline([new CountPerTick()]);

    // Three messages arrive: nothing goes downstream yet.
    for (let i = 0; i < 3; i += 1) {
      expect(p.run([msg({ v: 1 })])).toHaveLength(0);
    }
    // The tick closes the window and emits one rollup.
    const out = p.run([], 1_000);
    expect(out).toHaveLength(1);
    expect((out[0].msg.body as { count: number }).count).toBe(3);

    // A second tick with nothing accumulated emits nothing — an empty window is not an event.
    expect(p.run([], 2_000)).toHaveLength(0);
  });

  it("chains stages, and a tick flows through the rest of the pipeline on the same pass", () => {
    const p = new Pipeline([new FieldEquals("quality", "GOOD"), new CountPerTick()]);

    p.run([msg({ quality: "GOOD" })]);
    p.run([msg({ quality: "BAD" })]); // filtered out before it ever reaches the counter

    const out = p.run([], 1_000);
    expect(out).toHaveLength(1);
    expect((out[0].msg.body as { count: number }).count).toBe(1);
  });

  it("lets a stage fan out to many messages", () => {
    const fanOut = { process: (m: ProcMsg): ProcMsg[] => [m, m, m] };
    expect(new Pipeline([fanOut]).run([msg({ v: 1 })])).toHaveLength(3);
  });
});

describe("pluck", () => {
  it("walks a dotted path", () => {
    const body = { signal: { id: "temp-1" } };
    expect(pluck(body, "signal.id")).toBe("temp-1");
    expect(pluck(body, "signal.nope")).toBeUndefined();
    expect(pluck(body, "nope.nope")).toBeUndefined();
  });
});

describe("stage config", () => {
  it("builds the stages it knows", () => {
    expect(buildStage({ fieldEquals: { path: "a.b", value: 1 } })).toBeInstanceOf(FieldEquals);
    expect(buildStage({ countPerTick: {} })).toBeInstanceOf(CountPerTick);
  });

  it("rejects a stage it does not know, rather than silently doing nothing", () => {
    expect(() => buildStage({ fieldEqals: { path: "a", value: 1 } })).toThrow(/unknown stage/);
    expect(() => buildStage({ fieldEquals: { value: 1 } })).toThrow(/path/);
    expect(() => buildStage({ a: {}, b: {} })).toThrow(/exactly one/);
  });
});
