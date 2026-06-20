import { describe, it, expect, vi, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { Config } from "../src/config/model";
import { logger, initLogging, reconfigureLogging, LoggingReconfigurer } from "../src/logging";

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
