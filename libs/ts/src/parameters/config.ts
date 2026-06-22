/**
 * Parameters config — TypeScript port of the Rust `parameters::config`.
 *
 * Parse the `parameters` config section and build a {@link DefaultParameterService}: select the
 * {@link ParameterSource} backend, choose a source-aware cache (persistent-encrypted for remote
 * sources, in-memory for already-local ones — overridable via `cache.persist`), wire the declared
 * sync names/paths, optionally bootstrap, and start the background refresh.
 *
 * Phase 1 ships three sources: `awsSsm` (remote; `@aws-sdk/client-ssm` via dynamic import),
 * `mountedDir` (K8s ConfigMap/Secret volumes, Docker secrets), and `env`.
 */
import { logger } from "../logging";
import { buildKeyProvider, KeyProviderConfig, LocalVault } from "../credentials";

import { ParameterError } from "./errors";
import { DefaultParameterService, SyncPath } from "./service";
import { EnvSource, MountedDirSource, ParameterSource } from "./source";

/** One path to sync: a bare string (recursive) or `{ path, recursive }`. */
export type PathEntry = string | { path: string; recursive?: boolean };

/** The `parameters` config section. */
export interface ParametersConfig {
  source?: {
    /** `none` | `awsSsm` | `mountedDir` | `env`. */
    type?: string;
    // ----- awsSsm -----
    region?: string;
    /** Override the SSM endpoint (floci/LocalStack/VPC endpoint). */
    endpointUrl?: string;
    /** Decrypt `SecureString` parameters (flagging them `secure`). Default true. */
    withDecryption?: boolean;
    // ----- mountedDir -----
    /** Root directory for the `mountedDir` source. */
    root?: string;
    /** Parameter-name prefixes whose values are sensitive (a Secret volume vs a ConfigMap volume). */
    securePaths?: string[];
    // ----- env -----
    /** Env-var prefix for the `env` source (e.g. `GG_PARAM_`). */
    prefix?: string;
  };
  cache?: {
    /** Force persistence on/off. Unset (default) is source-aware: persist for remote sources. */
    persist?: boolean;
    /** On-disk path for the persistent cache vault. */
    path?: string;
    /** KEK custodian for the persistent cache (reuses the credentials key-provider config). */
    keyProvider?: KeyProviderConfig;
  };
  /** Greengrass delivers config numbers as doubles (300.0); coerced leniently to an integer. */
  refreshIntervalSecs?: number;
  bootstrapOnStart?: boolean;
  sync?: { names?: string[]; paths?: PathEntry[] };
}

const REMOTE_KINDS = new Set(["awsSsm"]);

/** Normalize a {@link PathEntry} to `[path, recursive]` (a bare string is recursive). */
function pathEntry(entry: PathEntry): SyncPath {
  if (typeof entry === "string") return [entry, true];
  return [entry.path, entry.recursive ?? true];
}

/** Build the {@link ParameterSource} backend named by `source.type`. */
async function buildSource(source: NonNullable<ParametersConfig["source"]>): Promise<ParameterSource> {
  const kind = source.type ?? "none";
  switch (kind) {
    case "env":
      return new EnvSource(source.prefix ?? "GG_PARAM_");
    case "mountedDir": {
      if (!source.root) {
        throw new ParameterError("mountedDir source requires source.root");
      }
      return new MountedDirSource(source.root, source.securePaths ?? []);
    }
    case "awsSsm": {
      /* v8 ignore next 3 -- the awsSsm branch needs @aws-sdk/client-ssm + AWS; covered out-of-band (see ssm.ts note) */
      const { AwsSsmSource } = await import("./ssm");
      return AwsSsmSource.create(source.region, source.endpointUrl, source.withDecryption ?? true);
    }
    default:
      throw new ParameterError(
        `parameter source '${kind}' is not available (supported: 'env', 'mountedDir', 'awsSsm')`,
      );
  }
}

/**
 * Open the parameter service from a parsed `parameters` config object.
 *
 * Mirrors the Rust `parameters::config::open`: select the source, pick a source-aware cache
 * (persistent-encrypted for remote, in-memory for local — `cache.persist` overrides), wire the
 * declared sync names/paths, bootstrap (offline-tolerant), and start the background refresh.
 * Async because building a remote source / KMS key provider is promise-based.
 *
 * No namespacing of parameter keys (matches the Rust port — the cache path is per-component
 * templated by the caller).
 */
export async function openFromConfig(cfg: ParametersConfig = {}): Promise<DefaultParameterService> {
  const sourceCfg = cfg.source ?? {};
  const source = await buildSource(sourceCfg);

  const syncNames = cfg.sync?.names ?? [];
  const syncPaths: SyncPath[] = (cfg.sync?.paths ?? []).map(pathEntry);

  const refreshIntervalSecs = Math.trunc(cfg.refreshIntervalSecs ?? 300);
  const bootstrapOnStart = cfg.bootstrapOnStart ?? true;

  // Source-aware default: remote sources persist encrypted (survive restart/offline); local
  // sources stay in memory (the backend is itself always available). `cache.persist` overrides.
  const isRemote = REMOTE_KINDS.has(sourceCfg.type ?? "none");
  const persist = cfg.cache?.persist ?? isRemote;

  let service: DefaultParameterService;
  if (persist) {
    const cachePath = cfg.cache?.path ?? "param-cache";
    const { provider, newVaultId, newDek } = await buildKeyProvider(
      cfg.cache?.keyProvider ?? {},
      cachePath,
      `${cachePath}.key`,
    );
    // keepVersions = 1: the cache only ever needs the latest value of each parameter.
    const vault = LocalVault.open(cachePath, provider, 1, newVaultId, newDek);
    service = DefaultParameterService.withPersistentCache(source, vault, syncNames, syncPaths);
  } else {
    service = DefaultParameterService.withMemoryCache(source, syncNames, syncPaths);
  }

  if (bootstrapOnStart) {
    // Offline-first: a bootstrap failure is non-fatal — the component starts and can retry via
    // refresh(). A persisted cache from a prior run still serves reads while the source is down.
    try {
      await service.refresh();
    } catch (e) {
      logger.warn(`parameter bootstrap refresh failed (continuing; cache may be empty): ${String(e)}`);
    }
  }

  // Background refresh on the configured interval (0 disables; the timer stops on close()).
  return service.withRefresh(refreshIntervalSecs);
}
