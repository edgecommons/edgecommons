import { describe, expect, it } from "vitest";

import { instanceConnectivity } from "../src/app";

describe("the instance-connectivity provider", () => {
  it("reports no instances, because this component owns no connections", () => {
    // The provider the `state` keepalive pushes and the `status` verb pulls — one source, two
    // surfaces. Reporting nothing is the contract, not an omission: with no instances the keepalive
    // carries no `instances[]` section and `status` answers exactly as `ping` does.
    expect(instanceConnectivity()).toEqual([]);
  });
});
