/**
 * Configuration source — FILE.
 *
 * Loads configuration from a JSON file on disk, with file-watch hot reload. A port
 * of the Rust `file.rs` source: `load` reads+parses the path; `watch` installs an OS
 * watcher on the file's *directory* (so atomic rename-replace edits are caught) and
 * emits a fresh document whenever the target file is created or modified. Malformed
 * reloads are logged (`console.warn`) and skipped — the previous config stays in
 * effect (i.e. `onUpdate` is not called).
 */
import * as fs from "fs";
import * as fsp from "fs/promises";
import * as path from "path";

import { EdgeCommonsError } from "../../errors";
import { ConfigSource, ConfigWatch } from "./index";

/** Loads configuration from a JSON file on disk, with file-watch hot reload. */
export class FileConfigSource implements ConfigSource {
  constructor(private readonly filePath: string) {}

  async load(): Promise<unknown> {
    let text: string;
    try {
      text = await fsp.readFile(this.filePath, "utf8");
    } catch (e) {
      throw EdgeCommonsError.io(`failed to read config file '${this.filePath}': ${(e as Error).message}`);
    }
    try {
      return JSON.parse(text);
    } catch (e) {
      throw EdgeCommonsError.config(`failed to parse config file '${this.filePath}': ${(e as Error).message}`);
    }
  }

  sourceName(): string {
    return "FILE";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    const target = path.resolve(this.filePath);
    const targetName = path.basename(target);
    // Watch the parent directory so atomic rename-replace edits are caught.
    const parent = path.dirname(target);
    const dir = parent.length > 0 ? parent : ".";

    let watcher: fs.FSWatcher;
    try {
      watcher = fs.watch(dir, { persistent: false }, (_eventType, filename) => {
        // Match by file name within the watched directory.
        if (filename !== null && filename !== undefined && path.basename(filename.toString()) !== targetName) {
          return;
        }
        // Re-read+parse; keep previous config on any failure.
        fs.readFile(target, "utf8", (readErr, data) => {
          if (readErr) {
            console.warn(`failed to read changed config file: ${readErr.message}`);
            return;
          }
          let value: unknown;
          try {
            value = JSON.parse(data);
          } catch (parseErr) {
            console.warn(`ignoring malformed config file change: ${(parseErr as Error).message}`);
            return;
          }
          onUpdate(value);
        });
      });
    } catch (e) {
      console.warn(`failed to watch config directory '${dir}': ${(e as Error).message}`);
      return undefined;
    }

    return {
      close: async () => {
        watcher.close();
      },
    };
  }
}
