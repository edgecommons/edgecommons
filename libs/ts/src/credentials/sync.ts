/** Sync engine — seed + refresh the local vault from a central source. Offline-first, selective,
 * rotation-aware. Async (the AWS SDK is promise-based). */
import { logger } from "../logging";
import { CentralVaultSource } from "./central";
import { LocalVault } from "./vault";

/** `[callerName, centralIdOverride]`. */
export type SyncSecret = [string, string | undefined];

export class SyncEngine {
  private timer?: NodeJS.Timeout;
  private running = false;

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
    try {
      for (const [name, override] of this.secrets) {
        const localKey = this.localKey(name);
        // Central id defaults to the namespaced path (per-device); override = shared/fleet id.
        const centralId = override ?? localKey;
        let cs;
        try {
          cs = await this.source.fetch(centralId);
        } catch (e) {
          logger.warn(`central fetch failed for '${centralId}'; using cached value: ${String(e)}`);
          continue;
        }
        if (!cs) continue;
        this.vault.reloadIfChanged();
        if (this.vault.latestCentralVersionId(localKey) === cs.centralVersionId) continue;
        this.vault.put(localKey, cs.bytes, {
          source: "central",
          centralVersionId: cs.centralVersionId,
          labels: cs.labels,
        });
        logger.info(`secret '${localKey}' synced from central (${centralId})`);
      }
    } finally {
      this.running = false;
    }
  }

  close(): void {
    if (this.timer) clearInterval(this.timer);
  }
}
