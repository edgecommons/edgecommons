/** Sync engine — seed + refresh the local vault from a central source. Offline-first, selective,
 * rotation-aware. Async (the AWS SDK is promise-based). */
import { logger } from "../logging";
import { CentralVaultSource } from "./central";
import { LocalVault } from "./vault";

/** `[callerName, centralIdOverride]`. */
export type SyncSecret = [string, string | undefined];

/** A snapshot of the sync engine's observability counters (read by the credential metrics bridge). */
export interface SyncStats {
  /** Epoch-ms of the last fully-successful pass, or `undefined` if it never completed one. */
  lastSuccessMs?: number;
  /** Number of central-fetch failures. */
  failures: number;
  /** Number of secrets written/rotated from central. */
  rotations: number;
}

export class SyncEngine {
  private timer?: NodeJS.Timeout;
  private running = false;
  private lastSuccessMs?: number;
  private failures = 0;
  private rotations = 0;

  private constructor(
    private readonly vault: LocalVault,
    private readonly source: CentralVaultSource,
    private readonly namespace: string,
    private readonly secrets: SyncSecret[],
  ) {}

  /** Build the engine, run a bootstrap pass when requested, then schedule periodic refresh. */
  static async start(
    vault: LocalVault,
    source: CentralVaultSource,
    namespace: string,
    secrets: SyncSecret[],
    intervalSecs: number,
    bootstrap: boolean,
  ): Promise<SyncEngine> {
    const e = new SyncEngine(vault, source, namespace, secrets);
    if (bootstrap) {
      await e.syncNow();
    }
    if (intervalSecs > 0) {
      e.timer = setInterval(() => void e.syncNow(), intervalSecs * 1000);
      e.timer.unref?.();
    }
    return e;
  }

  private localKey(name: string): string {
    return this.namespace ? `${this.namespace}/${name}` : name;
  }

  /** Force an immediate sync pass (skips if one is already running). */
  async syncNow(): Promise<void> {
    if (this.running) return;
    this.running = true;
    let anySuccess = false;
    try {
      for (const [name, override] of this.secrets) {
        const localKey = this.localKey(name);
        // Central id defaults to the namespaced path (per-device); override = shared/fleet id.
        const centralId = override ?? localKey;
        let cs;
        try {
          cs = await this.source.fetch(centralId);
        } catch (e) {
          // Offline-first: keep the cached value, surface the staleness.
          this.failures += 1;
          logger.warn(`central fetch failed for '${centralId}'; using cached value: ${String(e)}`);
          continue;
        }
        anySuccess = true;
        if (!cs) continue;
        this.vault.reloadIfChanged();
        if (this.vault.latestCentralVersionId(localKey) === cs.centralVersionId) continue;
        this.vault.put(localKey, cs.bytes, {
          source: "central",
          centralVersionId: cs.centralVersionId,
          labels: cs.labels,
        });
        this.rotations += 1;
        logger.info(`secret '${localKey}' synced from central (${centralId})`);
      }
    } finally {
      if (anySuccess) this.lastSuccessMs = Date.now();
      this.running = false;
    }
  }

  /** A snapshot of the sync counters (for the credential metrics bridge). */
  stats(): SyncStats {
    return { lastSuccessMs: this.lastSuccessMs, failures: this.failures, rotations: this.rotations };
  }

  close(): void {
    if (this.timer) clearInterval(this.timer);
  }
}
