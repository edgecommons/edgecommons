/**
 * Parameter service — TypeScript port of the Rust `parameters::service`.
 *
 * `gg.parameters()` returns a {@link ParameterService} — offline-first, source-agnostic reads of
 * externalized parameters. {@link DefaultParameterService} caches whatever a {@link ParameterSource}
 * provides and serves reads from the cache (never the network), refreshing the declared names/paths
 * selectively in the background / on demand.
 *
 * The cache is **source-aware**: a remote source (SSM, …) uses a persistent **encrypted** cache —
 * reusing the credentials {@link LocalVault} (same normative on-disk format) — so values survive
 * restarts and offline. An already-local source (`mountedDir`, `env`) uses an in-memory cache (the
 * backend is itself local + always available; re-persisting it would be redundant).
 */
import { logger } from "../logging";
import { LocalVault, PutOptions } from "../credentials";

import { ParameterError } from "./errors";
import { ParameterSource } from "./source";

const SECURE_LABEL = "secure";
const VERSION_LABEL = "pversion";

/** A cached parameter value (decrypted, in memory). `secure` values must not be logged. */
interface Cached {
  value: Buffer;
  secure: boolean;
  version?: string;
}

/** The cache layer behind the service (offline-first read store). */
interface ParamCache {
  get(name: string): Cached | undefined;
  put(name: string, c: Cached): void;
  entries(prefix: string): Array<[string, Cached]>;
  len(): number;
}

/** In-memory cache for already-local sources (`mountedDir`, `env`). */
class MemoryCache implements ParamCache {
  private readonly map = new Map<string, Cached>();

  get(name: string): Cached | undefined {
    return this.map.get(name);
  }
  put(name: string, c: Cached): void {
    this.map.set(name, c);
  }
  entries(prefix: string): Array<[string, Cached]> {
    const out: Array<[string, Cached]> = [];
    for (const [k, v] of this.map) {
      if (k.startsWith(prefix)) out.push([k, v]);
    }
    return out;
  }
  len(): number {
    return this.map.size;
  }
}

/**
 * Persistent encrypted cache for remote sources — reuses the credentials {@link LocalVault} (the
 * same normative, cross-language on-disk format). The parameter value is the secret bytes; `secure`
 * and the upstream version ride along as labels.
 */
class VaultCache implements ParamCache {
  constructor(private readonly vault: LocalVault) {}

  get(name: string): Cached | undefined {
    this.vault.reloadIfChanged();
    const s = this.vault.get(name);
    if (!s) return undefined;
    return {
      value: s.bytes(),
      secure: s.labels[SECURE_LABEL] === "true",
      version: s.labels[VERSION_LABEL],
    };
  }
  put(name: string, c: Cached): void {
    const labels: Record<string, string> = { [SECURE_LABEL]: String(c.secure) };
    if (c.version !== undefined) labels[VERSION_LABEL] = c.version;
    const opts: PutOptions = { source: "parameter", labels };
    this.vault.reloadIfChanged();
    this.vault.put(name, c.value, opts);
  }
  entries(prefix: string): Array<[string, Cached]> {
    this.vault.reloadIfChanged();
    const out: Array<[string, Cached]> = [];
    for (const meta of this.vault.list(prefix)) {
      const s = this.vault.get(meta.name);
      if (s) {
        out.push([
          meta.name,
          {
            value: s.bytes(),
            secure: s.labels[SECURE_LABEL] === "true",
            version: s.labels[VERSION_LABEL],
          },
        ]);
      }
    }
    return out;
  }
  len(): number {
    this.vault.reloadIfChanged();
    return this.vault.list("").length;
  }
}

/** Non-sensitive parameter-subsystem stats. */
export interface ParameterStats {
  parameterCount: number;
  /** Age of the last successful refresh, ms (undefined if never refreshed). */
  lastRefreshAgeMs?: number;
  refreshFailures: number;
  source: string;
}

/** `[path, recursive]` — one declared path to sync. */
export type SyncPath = [string, boolean];

/** The public parameter interface (depend on this, not {@link DefaultParameterService}). */
export interface ParameterService {
  /** The value of `name` as a UTF-8 string, or `undefined`. Served from the local cache. */
  get(name: string): string | undefined;
  /** The raw value bytes of `name`. */
  getBytes(name: string): Buffer | undefined;
  /** All cached parameters under `path` (the prefix), as name -> string value. */
  getByPath(path: string): Map<string, string>;
  /** Cached parameter names under `prefix` (metadata only — no values). */
  names(prefix: string): string[];
  /** Force an immediate pull of the declared names/paths from the source into the cache. */
  refresh(): Promise<void>;
  /** Non-sensitive stats for observability. */
  stats(): ParameterStats;
  /** The value parsed as an integer. */
  getInt(name: string): number | undefined;
  /** The value parsed as a boolean (`true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off`). */
  getBool(name: string): boolean | undefined;
  /** The value parsed as JSON. */
  getJson(name: string): unknown | undefined;
  /** A `StringList` value (comma-separated) as a list. */
  getStringList(name: string): string[] | undefined;
}

/** The shared refresh-able core (source + cache + selection + counters). */
class Inner {
  lastRefreshMs?: number;
  failures = 0;

  constructor(
    readonly source: ParameterSource,
    readonly cache: ParamCache,
    readonly syncNames: string[],
    readonly syncPaths: SyncPath[],
  ) {}

  async refresh(): Promise<void> {
    let anyErr: Error | undefined;
    for (const name of this.syncNames) {
      try {
        const v = await this.source.fetch(name);
        if (v) this.cache.put(name, { value: v.value, secure: v.secure, version: v.version });
      } catch (e) {
        // Never log the value — only the parameter name + error.
        logger.warn(`parameter refresh failed for '${name}' (keeping cached value): ${String(e)}`);
        anyErr = e as Error;
      }
    }
    for (const [path, recursive] of this.syncPaths) {
      try {
        const items = await this.source.fetchByPath(path, recursive);
        for (const [name, v] of items) {
          this.cache.put(name, { value: v.value, secure: v.secure, version: v.version });
        }
      } catch (e) {
        logger.warn(`parameter path refresh failed for '${path}' (keeping cached values): ${String(e)}`);
        anyErr = e as Error;
      }
    }
    if (anyErr) {
      this.failures += 1;
      // Offline-first: a refresh failure is non-fatal when we already have cached values.
      if (this.cache.len() === 0) throw anyErr;
    } else {
      this.lastRefreshMs = Date.now();
    }
  }
}

/**
 * Default {@link ParameterService}: a {@link ParameterSource} behind an offline-first cache,
 * optionally refreshed by a background timer. {@link close} stops the timer (TS has no RAII).
 */
export class DefaultParameterService implements ParameterService {
  private timer?: ReturnType<typeof setInterval>;

  private constructor(private readonly inner: Inner) {}

  /** Build with a persistent encrypted cache (the credentials {@link LocalVault}) — remote sources. */
  static withPersistentCache(
    source: ParameterSource,
    vault: LocalVault,
    syncNames: string[],
    syncPaths: SyncPath[],
  ): DefaultParameterService {
    return new DefaultParameterService(new Inner(source, new VaultCache(vault), syncNames, syncPaths));
  }

  /** Build with an in-memory cache — for already-local sources (`mountedDir`, `env`). */
  static withMemoryCache(
    source: ParameterSource,
    syncNames: string[],
    syncPaths: SyncPath[],
  ): DefaultParameterService {
    return new DefaultParameterService(new Inner(source, new MemoryCache(), syncNames, syncPaths));
  }

  /**
   * Start a background refresh timer that re-pulls the declared names/paths every `intervalSecs`
   * (0 disables it). The timer is stopped by {@link close}. Fluent; returns `this`.
   */
  withRefresh(intervalSecs: number): this {
    if (intervalSecs > 0) {
      this.timer = setInterval(() => {
        void this.inner.refresh().catch((e) => logger.debug(`background parameter refresh failed: ${String(e)}`));
      }, intervalSecs * 1000);
      this.timer.unref?.();
    }
    return this;
  }

  get(name: string): string | undefined {
    const b = this.getBytes(name);
    if (b === undefined) return undefined;
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(b);
    } catch {
      throw new ParameterError(`parameter '${name}' is not UTF-8`);
    }
  }

  getBytes(name: string): Buffer | undefined {
    return this.inner.cache.get(name)?.value;
  }

  getByPath(path: string): Map<string, string> {
    const out = new Map<string, string>();
    for (const [name, c] of this.inner.cache.entries(path)) {
      try {
        out.set(name, new TextDecoder("utf-8", { fatal: true }).decode(c.value));
      } catch {
        // Non-UTF-8 cached value — omit from the string subtree (mirrors the Rust skip).
      }
    }
    return out;
  }

  names(prefix: string): string[] {
    return this.inner.cache.entries(prefix).map(([n]) => n);
  }

  async refresh(): Promise<void> {
    await this.inner.refresh();
  }

  stats(): ParameterStats {
    const last = this.inner.lastRefreshMs;
    return {
      parameterCount: this.inner.cache.len(),
      lastRefreshAgeMs: last !== undefined ? Math.max(0, Date.now() - last) : undefined,
      refreshFailures: this.inner.failures,
      source: this.inner.source.sourceId(),
    };
  }

  getInt(name: string): number | undefined {
    const s = this.get(name);
    if (s === undefined) return undefined;
    const t = s.trim();
    if (!/^[+-]?\d+$/.test(t)) {
      throw new ParameterError(`parameter '${name}' is not an integer: ${t}`);
    }
    return Number.parseInt(t, 10);
  }

  getBool(name: string): boolean | undefined {
    const s = this.get(name);
    if (s === undefined) return undefined;
    switch (s.trim().toLowerCase()) {
      case "true":
      case "1":
      case "yes":
      case "on":
        return true;
      case "false":
      case "0":
      case "no":
      case "off":
        return false;
      default:
        throw new ParameterError(`parameter '${name}' is not a boolean: ${s.trim()}`);
    }
  }

  getJson(name: string): unknown | undefined {
    const b = this.getBytes(name);
    if (b === undefined) return undefined;
    try {
      return JSON.parse(b.toString("utf-8"));
    } catch (e) {
      throw new ParameterError(`parameter '${name}' is not JSON: ${(e as Error).message}`);
    }
  }

  getStringList(name: string): string[] | undefined {
    const s = this.get(name);
    if (s === undefined) return undefined;
    if (s.length === 0) return [];
    return s.split(",").map((x) => x.trim());
  }

  /** Stop the background refresh timer (TS has no RAII; mirrors CredentialMetricsBridge.close()). */
  close(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}
