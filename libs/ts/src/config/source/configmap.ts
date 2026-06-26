/**
 * Configuration source — CONFIGMAP (the Kubernetes-native source).
 *
 * Reads the component configuration from a mounted **ConfigMap directory** and hot-reloads it across
 * the kubelet's atomic `..data` symlink swap (DESIGN-subsystems §1, FR-CFG-1..5). It is the default
 * config source on the `KUBERNETES` platform and the k8s analogue of {@link FileConfigSource} — it
 * reuses the same load + watch + reject-and-keep seam, but watches the mount *directory* (not the
 * file inode) and re-arms after the swap so hot-reload survives ConfigMap edits.
 *
 * Selected via `-c CONFIGMAP [mountDir] [key]`; defaults are mount dir `/etc/ggcommons` and key
 * `config.json` (so a pod with a ConfigMap mounted at `/etc/ggcommons` loads `config.json` with no
 * `-c` flag). Mirrors the canonical Java `ConfigMapConfigProvider` + `DirectoryWatcher`.
 *
 * **Why not {@link FileConfigSource}?** A mounted ConfigMap is a directory of symlinks the kubelet
 * swaps atomically: the user-visible `config.json` points at `..data/config.json`, and `..data` is
 * itself a symlink the kubelet replaces (write a new timestamped dir, stage `..data_tmp` -> it, then
 * `rename(..data_tmp, ..data)`). A watch on the user-visible *file* fires once and dies after the
 * swap (its inode is gone — `IN_DELETE_SELF`); worse, the swap manifests as an event on `..data`, not
 * on `config.json`, so the name-filtered watch {@link FileConfigSource} uses never reloads. This
 * source therefore watches the mount directory, reacts to *any* entry event (so the `..data` swap
 * triggers a reload), and **re-arms** if the watch is ever invalidated (FR-CFG-2).
 *
 * **Reject-and-keep (FR-CFG-5).** On a reload, a malformed read (mid-swap window, or a bad ConfigMap
 * edit) must never crash a running pod: a read/parse/empty failure is logged and the previous config
 * is kept (`onUpdate` is not called). The *initial* {@link load} still fails loudly, like
 * {@link FileConfigSource}.
 *
 * **The `subPath` caveat (FR-CFG-3).** A ConfigMap mounted with `subPath` is *never* updated by the
 * kubelet — there is no `..data` symlink farm and hot-reload is silently dead. This source warns when
 * it detects a mount with no `..data` entry. Mount the whole volume, not a `subPath`; for a forced
 * `subPath`/immutable/env mount use a restart-on-change controller (e.g. Stakater Reloader).
 *
 * **Dotfile filter (FR-CFG-4).** Kubelet projection artifacts (`..data`, `..2026_…` timestamped
 * dirs, `..data_tmp`) are never parsed as config: the configured key is rejected if it is itself such
 * an artifact, reusing the {@link isProjectionArtifact} dotfile filter shared with `MountedDirSource`.
 */
import * as fs from "fs";
import * as fsp from "fs/promises";
import * as path from "path";

import { GgError } from "../../errors";
import { logger } from "../../logging";
import { isProjectionArtifact } from "../../parameters/source";
import { ConfigSource, ConfigWatch } from "./index";

/** Loads configuration from a mounted ConfigMap directory, with directory-watch hot reload. */
export class ConfigMapConfigSource implements ConfigSource {
  /** Default ConfigMap mount directory when `-c CONFIGMAP` is given no path argument. */
  static readonly DEFAULT_MOUNT_DIR = "/etc/ggcommons";
  /** Default config key (file name within the mount) when none is given. */
  static readonly DEFAULT_KEY = "config.json";
  /** The kubelet's atomic-swap symlink; its presence indicates a whole-volume (reloadable) mount. */
  static readonly KUBELET_DATA_LINK = "..data";
  /** Backoff before re-arming after the directory watch is invalidated / could not arm. */
  private static readonly REARM_BACKOFF_MS = 200;

  private readonly mountDir: string;
  private readonly key: string;
  private readonly configFile: string;

  /**
   * @param mountDir the ConfigMap mount directory, or `undefined` for `/etc/ggcommons`
   * @param key      the config file name within the mount, or `undefined` for `config.json`
   * @throws {@link GgError} of kind `Config` if `key` is a kubelet projection artifact (a `..`/`.` entry)
   */
  constructor(mountDir?: string, key?: string) {
    this.mountDir = mountDir ?? ConfigMapConfigSource.DEFAULT_MOUNT_DIR;
    this.key = key ?? ConfigMapConfigSource.DEFAULT_KEY;
    if (isProjectionArtifact(this.key)) {
      throw GgError.config(
        `ConfigMap key must not be a kubelet projection artifact (a '..'/'.' entry): ${this.key}`,
      );
    }
    this.configFile = path.join(this.mountDir, this.key);
    this.warnIfSubPathMount();
  }

  /**
   * Warns when the mount appears to be a `subPath` (or otherwise non-projected) mount that will never
   * hot-reload — detected by the absence of the kubelet `..data` symlink (FR-CFG-3).
   */
  private warnIfSubPathMount(): void {
    if (!fs.existsSync(path.join(this.mountDir, ConfigMapConfigSource.KUBELET_DATA_LINK))) {
      logger.warn(
        `ConfigMap mount '${this.mountDir}' has no '${ConfigMapConfigSource.KUBELET_DATA_LINK}' ` +
          `symlink — this looks like a subPath/immutable mount, which the kubelet never updates, so ` +
          `hot-reload is disabled. Mount the whole volume (not a subPath), or use a restart-on-change ` +
          `controller.`,
      );
    }
  }

  async load(): Promise<unknown> {
    let text: string;
    try {
      text = await fsp.readFile(this.configFile, "utf8");
    } catch (e) {
      throw GgError.io(
        `failed to read ConfigMap config '${this.configFile}': ${(e as Error).message}`,
      );
    }
    try {
      return JSON.parse(text);
    } catch (e) {
      throw GgError.config(
        `failed to parse ConfigMap config '${this.configFile}': ${(e as Error).message}`,
      );
    }
  }

  sourceName(): string {
    return "CONFIGMAP";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    let watcher: fs.FSWatcher | undefined;
    let closed = false;
    let rearmTimer: NodeJS.Timeout | undefined;

    // Re-read the configured key and apply it. Reject-and-keep on a transient/malformed read (a
    // mid-swap window or a bad edit) so a running pod never crashes on reload (FR-CFG-5).
    const reload = (): void => {
      fs.readFile(this.configFile, "utf8", (err, data) => {
        if (closed) return;
        if (err) {
          logger.warn(`ConfigMap reload read failed (keeping previous config): ${err.message}`);
          return;
        }
        let value: unknown;
        try {
          value = JSON.parse(data);
        } catch (e) {
          logger.warn(`ConfigMap reload parse failed (keeping previous config): ${(e as Error).message}`);
          return;
        }
        if (value === null || value === undefined) {
          logger.warn("ConfigMap reload yielded empty configuration (keeping previous config).");
          return;
        }
        onUpdate(value);
      });
    };

    const scheduleRearm = (): void => {
      if (closed) return;
      rearmTimer = setTimeout(arm, ConfigMapConfigSource.REARM_BACKOFF_MS);
      // Do not keep the event loop alive solely for the re-arm timer.
      if (typeof rearmTimer.unref === "function") rearmTimer.unref();
    };

    const arm = (): void => {
      if (closed) return;
      try {
        // Watch the mount DIRECTORY (persists across swaps); react to ANY entry event so the kubelet
        // `..data` symlink swap triggers a reload (no name filter — the swap shows up on `..data`, not
        // on the user-visible key).
        watcher = fs.watch(this.mountDir, { persistent: false }, () => {
          reload();
        });
        watcher.on("error", (e) => {
          // Watch invalidated (e.g. the mount directory inode was replaced) — re-arm rather than
          // silently going dead (FR-CFG-2).
          logger.warn(
            `ConfigMap directory watch on '${this.mountDir}' errored (${(e as Error).message}); re-arming.`,
          );
          try {
            watcher?.close();
          } catch {
            /* ignore */
          }
          watcher = undefined;
          scheduleRearm();
        });
        logger.debug(`ConfigMap directory watch armed on ${this.mountDir}`);
      } catch (e) {
        // The mount directory may not exist yet (a swap window / late mount). Back off and retry
        // rather than giving up — the directory watch must survive inode replacement (FR-CFG-2).
        logger.warn(
          `ConfigMap directory watch could not arm on '${this.mountDir}' (${(e as Error).message}); retrying.`,
        );
        scheduleRearm();
      }
    };

    arm();

    return {
      close: async () => {
        closed = true;
        if (rearmTimer) clearTimeout(rearmTimer);
        try {
          watcher?.close();
        } catch {
          /* ignore */
        }
      },
    };
  }
}
