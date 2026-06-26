import { describe, it, expect, vi, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { Config } from "../src/config/model";
import { logger, initLogging, reconfigureLogging, LoggingReconfigurer } from "../src/logging";
import {
  ENV_K8S_NODE_NAME,
  ENV_K8S_POD_NAME,
  ENV_K8S_POD_NAMESPACE,
  JSON_LOG_FORMAT,
} from "../src/platform";

const tmpDirs: string[] = [];
function tmpDir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "ggc-log-"));
  tmpDirs.push(d);
  return d;
}
afterEach(() => {
  // Reset the shared logger to a clean state.
  initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }));
  for (const d of tmpDirs.splice(0)) {
    try {
      fs.rmSync(d, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

/** Capture writes to process.stdout/stderr while fn runs. */
function captureStdio(fn: () => void): { out: string; err: string } {
  let out = "";
  let err = "";
  const so = vi.spyOn(process.stdout, "write").mockImplementation((s) => {
    out += String(s);
    return true;
  });
  const se = vi.spyOn(process.stderr, "write").mockImplementation((s) => {
    err += String(s);
    return true;
  });
  try {
    fn();
  } finally {
    so.mockRestore();
    se.mockRestore();
  }
  return { out, err };
}

describe("logging level threshold", () => {
  it("suppresses debug at INFO, shows it at DEBUG", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }));
    let cap = captureStdio(() => logger.debug("hidden-dbg"));
    expect(cap.out).not.toContain("hidden-dbg");
    cap = captureStdio(() => logger.info("shown-info"));
    expect(cap.out).toContain("shown-info");
    expect(cap.out).toContain("[INFO]");

    reconfigureLogging(Config.fromValue("c", "t", { logging: { level: "DEBUG" } }));
    cap = captureStdio(() => logger.debug("now-shown-dbg"));
    expect(cap.out).toContain("now-shown-dbg");
    expect(cap.out).toContain("[DEBUG]");
  });

  it("warn/error go to stderr", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "DEBUG" } }));
    const cap = captureStdio(() => {
      logger.warn("a-warn");
      logger.error("an-error");
    });
    expect(cap.err).toContain("a-warn");
    expect(cap.err).toContain("an-error");
  });

  it("applies a custom ts_format token template (and re-applies on reload)", () => {
    initLogging(
      Config.fromValue("c", "t", {
        logging: { level: "INFO", ts_format: "LVL={level} MSG={message}" },
      }),
    );
    let cap = captureStdio(() => logger.info("hello"));
    expect(cap.out).toContain("LVL=INFO MSG=hello");
    expect(cap.out).not.toContain("[INFO]"); // default format not used

    // A reload with a different format takes effect immediately.
    reconfigureLogging(
      Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: "{level}|{message}" } }),
    );
    cap = captureStdio(() => logger.info("again"));
    expect(cap.out).toContain("INFO|again");
  });

  it("applies per-logger levels from logging.loggers (hierarchical), reloadable", async () => {
    const { getLogger } = await import("../src/logging");
    initLogging(
      Config.fromValue("c", "t", {
        logging: { level: "INFO", loggers: { "app.db": "DEBUG", "app.noisy": "ERROR" } },
      }),
    );
    // Exact + prefix match: app.db (and app.db.pool) => DEBUG; app.noisy => ERROR; other => root INFO.
    let cap = captureStdio(() => {
      getLogger("app.db").debug("db-dbg");
      getLogger("app.db.pool").debug("pool-dbg");
      getLogger("app.noisy").info("noisy-info-hidden");
      getLogger("other").debug("other-dbg-hidden");
      getLogger("other").info("other-info");
    });
    expect(cap.out).toContain("db-dbg");
    expect(cap.out).toContain("pool-dbg");
    expect(cap.out).not.toContain("noisy-info-hidden");
    expect(cap.out).not.toContain("other-dbg-hidden");
    expect(cap.out).toContain("other-info");

    // A reload changes per-logger levels live.
    reconfigureLogging(Config.fromValue("c", "t", { logging: { level: "INFO", loggers: { "app.db": "ERROR" } } }));
    cap = captureStdio(() => getLogger("app.db").info("db-info-now-hidden"));
    expect(cap.out).not.toContain("db-info-now-hidden");
  });

  it("an unparseable level falls back to INFO", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "GIBBERISH" } }));
    const cap = captureStdio(() => {
      logger.debug("dbg");
      logger.info("inf");
    });
    expect(cap.out).not.toContain("dbg");
    expect(cap.out).toContain("inf");
  });
});

describe("logging file output + rotation", () => {
  it("writes formatted lines to a file", () => {
    const dir = tmpDir();
    const file = path.join(dir, "app.log");
    initLogging(
      Config.fromValue("c", "t", {
        logging: { level: "INFO", fileLogging: { enabled: true, filePath: file } },
      }),
    );
    captureStdio(() => logger.info("file-line"));
    const text = fs.readFileSync(file, "utf8");
    expect(text).toContain("file-line");
    expect(text).toContain("[INFO]");
  });

  it("rotates to .1/.2 backups when maxFileSize is exceeded, capped by backupCount", () => {
    const dir = tmpDir();
    const file = path.join(dir, "rot.log");
    initLogging(
      Config.fromValue("c", "t", {
        logging: {
          level: "INFO",
          fileLogging: { enabled: true, filePath: file, maxFileSize: "200B", backupCount: 2 },
        },
      }),
    );
    captureStdio(() => {
      for (let i = 0; i < 30; i++) logger.info(`line-number-${i}-padding-padding-padding`);
    });
    expect(fs.existsSync(file)).toBe(true);
    expect(fs.existsSync(`${file}.1`)).toBe(true);
    // backupCount=2 -> no .3 retained.
    expect(fs.existsSync(`${file}.3`)).toBe(false);
  });

  it("never throws on a bad file path (directory)", () => {
    const dir = tmpDir();
    // Point filePath at the directory itself: opening for append fails -> fail-soft.
    expect(() =>
      initLogging(
        Config.fromValue("c", "t", {
          logging: { level: "INFO", fileLogging: { enabled: true, filePath: dir } },
        }),
      ),
    ).not.toThrow();
    expect(() => captureStdio(() => logger.info("still-ok"))).not.toThrow();
  });
});

/** Parse captured stdout into the JSON objects emitted by the stdout-JSON sink (one per line). */
function jsonLines(out: string): Array<Record<string, unknown>> {
  return out
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

describe("stdout-JSON sink (FR-LOG-1/4)", () => {
  it("emits one valid JSON object per line with the core fields when ts_format=json", () => {
    initLogging(Config.fromValue("c", "my-thing", { logging: { level: "INFO", ts_format: "json" } }));
    const cap = captureStdio(() => {
      logger.info("first");
      logger.info("second");
    });
    const lines = jsonLines(cap.out);
    expect(lines.length).toBe(2);
    expect(lines[0].level).toBe("INFO");
    expect(lines[0].logger).toBe("ggcommons");
    expect(lines[0].message).toBe("first");
    expect(typeof lines[0].timestamp).toBe("string");
    expect(lines[1].message).toBe("second");
    // The plain token-template default must NOT be used.
    expect(cap.out).not.toContain("[INFO]");
  });

  it("the json selector is case-insensitive", () => {
    for (const token of ["JSON", "Json", " json "]) {
      initLogging(Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: token } }));
      const cap = captureStdio(() => logger.info("hi"));
      const lines = jsonLines(cap.out);
      expect(lines.length).toBe(1);
      expect(lines[0].message).toBe("hi");
    }
    // Sanity: the exported selector constant is the literal token.
    expect(JSON_LOG_FORMAT).toBe("json");
  });

  it("routes ALL levels (including warn/error) to stdout as JSON (single structured stream)", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "DEBUG", ts_format: "json" } }));
    const cap = captureStdio(() => {
      logger.debug("d");
      logger.warn("w");
      logger.error("e");
    });
    expect(cap.err).toBe(""); // nothing on stderr in JSON mode
    const lines = jsonLines(cap.out);
    expect(lines.map((l) => l.level)).toEqual(["DEBUG", "WARN", "ERROR"]);
  });

  it("includes a `thrown` field (with stack) when an error is supplied; still one JSON line", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: "json" } }));
    const cap = captureStdio(() => logger.error("boom", new Error("kaboom\nwith-newline")));
    const lines = jsonLines(cap.out);
    expect(lines.length).toBe(1); // a multi-line stack is still exactly one JSON line
    expect(lines[0].message).toBe("boom");
    expect(String(lines[0].thrown)).toContain("kaboom");
  });

  it("omits the `thrown` field when no error is supplied", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: "json" } }));
    const cap = captureStdio(() => logger.info("clean"));
    const obj = jsonLines(cap.out)[0];
    expect("thrown" in obj).toBe(false);
  });

  it("is fail-soft: a hostile error that throws while stringifying never breaks logging", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: "json" } }));
    const hostile = {
      toString() {
        throw new Error("stringify-explodes");
      },
    };
    let cap!: { out: string; err: string };
    expect(() => {
      cap = captureStdio(() => logger.error("with-hostile", hostile));
    }).not.toThrow();
    // The bad line is swallowed (no JSON emitted), but the logger keeps working afterward.
    expect(cap.out.trim()).toBe("");
    const next = captureStdio(() => logger.info("still-alive"));
    expect(jsonLines(next.out)[0].message).toBe("still-alive");
  });
});

describe("stdout-JSON correlation fields (FR-LOG-3)", () => {
  it("adds pod/namespace/node from the Downward-API env and thing from the identity", () => {
    initLogging(Config.fromValue("c", "thing-42", { logging: { level: "INFO", ts_format: "json" } }), {
      env: {
        [ENV_K8S_POD_NAME]: "ggc-pod-abc",
        [ENV_K8S_POD_NAMESPACE]: "edge",
        [ENV_K8S_NODE_NAME]: "node-7",
      },
    });
    const obj = jsonLines(captureStdio(() => logger.info("c")).out)[0];
    expect(obj.pod).toBe("ggc-pod-abc");
    expect(obj.namespace).toBe("edge");
    expect(obj.node).toBe("node-7");
    expect(obj.thing).toBe("thing-42");
  });

  it("omits correlation fields that are absent or empty (no null/empty noise)", () => {
    initLogging(Config.fromValue("c", "", { logging: { level: "INFO", ts_format: "json" } }), {
      env: { [ENV_K8S_POD_NAME]: "only-pod", [ENV_K8S_POD_NAMESPACE]: "" /* empty == absent */ },
    });
    const obj = jsonLines(captureStdio(() => logger.info("c")).out)[0];
    expect(obj.pod).toBe("only-pod");
    expect("namespace" in obj).toBe(false); // empty env value -> omitted
    expect("node" in obj).toBe(false); // unset -> omitted
    expect("thing" in obj).toBe(false); // empty identity -> omitted
  });

  it("does not emit correlation fields in text mode", () => {
    initLogging(Config.fromValue("c", "thing-42", { logging: { level: "INFO" } }), {
      env: { [ENV_K8S_POD_NAME]: "ggc-pod-abc" },
    });
    const cap = captureStdio(() => logger.info("plain"));
    expect(cap.out).not.toContain("ggc-pod-abc");
    expect(cap.out).toContain("[INFO]");
  });
});

describe("logging-format precedence (FR-RT-3) + no rotation on the JSON sink (FR-LOG-2)", () => {
  it("KUBERNETES profile default (formatDefault=json) selects JSON when config sets no ts_format", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }), {
      formatDefault: JSON_LOG_FORMAT,
    });
    const lines = jsonLines(captureStdio(() => logger.info("k8s-default")).out);
    expect(lines.length).toBe(1);
    expect(lines[0].message).toBe("k8s-default");
  });

  it("explicit config ts_format overrides the json profile default (back to text)", () => {
    initLogging(
      Config.fromValue("c", "t", { logging: { level: "INFO", ts_format: "{level}|{message}" } }),
      { formatDefault: JSON_LOG_FORMAT },
    );
    const cap = captureStdio(() => logger.info("override"));
    expect(cap.out).toContain("INFO|override");
    expect(() => jsonLines(cap.out)).toThrow(); // not JSON
  });

  it("HOST/GREENGRASS (no profile default) keep the text console default", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } })); // no formatDefault
    const cap = captureStdio(() => logger.info("host-default"));
    expect(cap.out).toContain("[INFO]");
    expect(cap.out).toContain("host-default");
  });

  it("does NOT install a rotating file writer under the JSON sink, even if fileLogging is enabled", () => {
    const dir = tmpDir();
    const file = path.join(dir, "k8s.log");
    initLogging(
      Config.fromValue("c", "t", {
        logging: {
          level: "INFO",
          ts_format: "json",
          fileLogging: { enabled: true, filePath: file, maxFileSize: "100B", backupCount: 3 },
        },
      }),
    );
    captureStdio(() => {
      for (let i = 0; i < 40; i++) logger.info(`line-${i}-padding-padding-padding`);
    });
    // No file (and certainly no rotated backups) under the JSON sink: stdout only.
    expect(fs.existsSync(file)).toBe(false);
    expect(fs.existsSync(`${file}.1`)).toBe(false);
  });

  it("re-applies the JSON profile default across a hot reload that sets no ts_format", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }), {
      formatDefault: JSON_LOG_FORMAT,
    });
    // A reload (via the LoggingReconfigurer path) with no ts_format must stay JSON.
    reconfigureLogging(Config.fromValue("c", "t", { logging: { level: "DEBUG" } }));
    const lines = jsonLines(captureStdio(() => logger.debug("still-json")).out);
    expect(lines.length).toBe(1);
    expect(lines[0].level).toBe("DEBUG");
  });
});

describe("LoggingReconfigurer", () => {
  it("onConfigurationChange returns true and applies the new level", () => {
    initLogging(Config.fromValue("c", "t", { logging: { level: "INFO" } }));
    const r = new LoggingReconfigurer();
    const ok = r.onConfigurationChange(Config.fromValue("c", "t", { logging: { level: "DEBUG" } }));
    expect(ok).toBe(true);
    const cap = captureStdio(() => logger.debug("dbg-after-reconfig"));
    expect(cap.out).toContain("dbg-after-reconfig");
  });
});
