import { describe, it, expect } from "vitest";

import { sanitizeShadowName, ShadowConfigSource } from "../src/config/source/shadow";

describe("sanitizeShadowName", () => {
  it("replaces characters AWS IoT shadow names disallow with underscore", () => {
    expect(sanitizeShadowName("com.ggcommons.TsGgVerify")).toBe("com_ggcommons_TsGgVerify");
    expect(sanitizeShadowName("a.b/c+d#e f")).toBe("a_b_c_d_e_f");
  });

  it("leaves already-valid names (alnum, ':', '_', '-') untouched", () => {
    expect(sanitizeShadowName("My-Shadow_1:v2")).toBe("My-Shadow_1:v2");
  });
});

describe("ShadowConfigSource default shadow name", () => {
  // Minimal fake exposing only what the constructor path touches.
  const fakeIpc = {} as unknown as ConstructorParameters<typeof ShadowConfigSource>[0];

  it("sanitizes the component-name default (dots -> underscores)", () => {
    const src = new ShadowConfigSource(fakeIpc, undefined, "thing-1", "com.example.MyComponent");
    expect((src as unknown as { shadowName: string }).shadowName).toBe("com_example_MyComponent");
  });

  it("uses an explicit name verbatim (the user's responsibility)", () => {
    const src = new ShadowConfigSource(fakeIpc, "my.explicit.name", "thing-1", "com.example.MyComponent");
    expect((src as unknown as { shadowName: string }).shadowName).toBe("my.explicit.name");
  });
});
