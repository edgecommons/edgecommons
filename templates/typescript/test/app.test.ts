import { CommandException } from "@edgecommons/edgecommons";
import { describe, expect, it } from "vitest";

import { INITIAL_GREETING, applyGreeting, instanceConnectivity } from "../src/app";

describe("the instance-connectivity provider", () => {
  it("reports no instances, because this component owns no connections", () => {
    // The provider the `state` keepalive pushes and the `status` verb pulls — one source, two
    // surfaces. Reporting nothing is the contract, not an omission: with no instances the keepalive
    // carries no `instances[]` section and `status` answers exactly as `ping` does.
    expect(instanceConnectivity()).toEqual([]);
  });
});

describe("the set-greeting command decision", () => {
  it("computes the greeting change from a well-formed body", () => {
    // The runtime applies `.greeting` to its in-memory state and returns the whole object, so a
    // console sees the previous and new greeting and the next status tick reflects the new one.
    const change = applyGreeting({ greeting: "hi there" }, INITIAL_GREETING);
    expect(change).toEqual({ previousGreeting: INITIAL_GREETING, greeting: "hi there" });
  });

  it("rejects a body without a string `greeting` with BAD_ARGS", () => {
    // A malformed command must be refused, not silently ignored — the caller gets a typed reason.
    for (const bad of [undefined, null, 42, "nope", {}, { greeting: 7 }]) {
      expect(() => applyGreeting(bad, INITIAL_GREETING)).toThrow(CommandException);
    }
  });
});
