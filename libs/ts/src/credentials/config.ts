/** Parse the `credentials` config section and build a service.
 *
 * Phase 1: file key provider + local vault. Phase 2: `awsSecretsManager` central source + sync.
 * `namespace` (`<thingName>/<componentName>`) is applied transparently to every key. Async because
 * the AWS SDK (and thus bootstrap) is promise-based.
 */
import { existsSync, mkdirSync, readFileSync } from "fs";
import { dirname } from "path";
import { randomUUID } from "crypto";

import { KEY_LEN, random } from "./crypto";
import { CredentialError } from "./errors";
import {
  EnvKeyProvider,
  FileKeyProvider,
  KeyProvider,
  KmsKeyProvider,
  Pkcs11KeyProvider,
  PrewrappedKeyProvider,
} from "./keyprovider";
import { logSink } from "./audit";
import { DefaultCredentialService } from "./service";
import { SyncEngine, SyncSecret } from "./sync";
import { LocalVault } from "./vault";
import { VaultFile } from "./format";

export interface CredentialsConfig {
  vault?: {
    path?: string;
    keepVersions?: number;
    keyProvider?: {
      type?: string;
      keyPath?: string;
      /** `env`: name of the env var holding the base64-encoded 32-byte KEK (default `EDGECOMMONS_VAULT_KEK`). */
      envVar?: string;
      kmsKeyId?: string;
      region?: string;
      endpointUrl?: string;
      modulePath?: string;
      tokenLabel?: string;
      keyLabel?: string;
      pinEnv?: string;
      pin?: string;
    };
  };
  central?: {
    type?: string;
    region?: string;
    endpointUrl?: string;
    refreshIntervalSecs?: number;
    bootstrapOnStart?: boolean;
    sync?: { secrets?: (string | { name: string; from?: string })[] };
  };
  /** Access-audit settings. Emit access events (op/name/version/source/outcome, never the value)
   * to the audit log. On by default — a secrets subsystem should record access; set `false` to
   * silence it. */
  audit?: { enabled?: boolean };
}

function syncSecrets(central: NonNullable<CredentialsConfig["central"]>): SyncSecret[] {
  const out: SyncSecret[] = [];
  for (const entry of central.sync?.secrets ?? []) {
    if (typeof entry === "string") out.push([entry, undefined]);
    else if (entry && entry.name) out.push([entry.name, entry.from]);
  }
  return out;
}

/** Per-provider key-provider config shape (shared by the credentials vault and the parameter cache). */
export type KeyProviderConfig = NonNullable<NonNullable<CredentialsConfig["vault"]>["keyProvider"]>;

/**
 * A KEK custodian ready to hand to {@link LocalVault.open}, with the optional brand-new vault
 * id/DEK the async KMS path must supply out of band (ignored when the vault already exists).
 */
export interface BuiltKeyProvider {
  provider: KeyProvider;
  newVaultId?: string;
  newDek?: Buffer;
}

/**
 * Build a KEK custodian from a key-provider config (mirrors the Rust `build_key_provider`). Shared
 * by the credentials vault ({@link openVault}) and the parameter cache. `file` wraps the DEK under a
 * local key file; `env` wraps it under a raw 32-byte KEK read (base64) from an env var / mounted
 * Secret; `kms`/`greengrass` wrap it via an AWS KMS CMK; `pkcs11` wraps it inside an HSM/TPM.
 *
 * Because `LocalVault.open` calls `wrapDek`/`unwrapDek` synchronously, the async KMS round trip is
 * performed eagerly here and handed back as a {@link PrewrappedKeyProvider} (plus the brand-new
 * vault id/DEK when creating a fresh KMS vault). The on-disk format is unchanged. `vaultPath` is the
 * vault file (read to unwrap an existing KMS DEK); `defaultKeyPath` is the `file` provider's key file
 * when `keyPath` is absent.
 *
 * `defaultType` is the key-provider type to use when `kp.type` is ABSENT — the platform-profile
 * default (e.g. `"env"` on KUBERNETES, FR-CRED-6) threaded from the credentials init site. When it too
 * is absent the library default `"file"` applies. An explicit `kp.type` always wins.
 */
export async function buildKeyProvider(
  kp: KeyProviderConfig,
  vaultPath: string,
  defaultKeyPath: string,
  defaultType?: string,
): Promise<BuiltKeyProvider> {
  const kind = kp.type ?? defaultType ?? "file";

  if (kind === "file") {
    const keyPath = kp.keyPath ?? defaultKeyPath;
    const dir = dirname(keyPath);
    if (dir) mkdirSync(dir, { recursive: true });
    const provider: KeyProvider = existsSync(keyPath)
      ? FileKeyProvider.fromKeyFile(keyPath)
      : FileKeyProvider.generateKeyFile(keyPath);
    return { provider };
  }

  if (kind === "env") {
    // Raw 32-byte KEK, base64, from an env var (typically a mounted k8s Secret) — the software-KEK.
    const envVar = kp.envVar ?? "EDGECOMMONS_VAULT_KEK";
    return { provider: EnvKeyProvider.fromEnv(envVar) };
  }

  if (kind === "kms" || kind === "greengrass") {
    if (!kp.kmsKeyId) {
      throw new CredentialError("kms key provider requires keyProvider.kmsKeyId");
    }
    const kms = await KmsKeyProvider.create(kp.kmsKeyId, kp.region, kp.endpointUrl);
    if (existsSync(vaultPath)) {
      // Existing vault: read its KEK + vaultId and KMS-decrypt the DEK eagerly.
      const dir = dirname(vaultPath);
      if (dir) mkdirSync(dir, { recursive: true });
      const vf = JSON.parse(readFileSync(vaultPath, "utf8")) as VaultFile;
      const dek = await kms.unwrapDek(vf.vaultId, vf.kek);
      return { provider: new PrewrappedKeyProvider("kms", vf.kek, dek) };
    }
    // New vault: generate the id + DEK here, KMS-wrap eagerly, then hand both to LocalVault.open.
    const vaultId = randomUUID();
    const dek = random(KEY_LEN);
    const kek = await kms.wrapDek(vaultId, dek);
    return { provider: new PrewrappedKeyProvider("kms", kek, dek), newVaultId: vaultId, newDek: dek };
  }

  if (kind === "pkcs11") {
    if (!kp.modulePath) throw new CredentialError("pkcs11 key provider requires keyProvider.modulePath");
    if (!kp.keyLabel) throw new CredentialError("pkcs11 key provider requires keyProvider.keyLabel");
    let pin: string;
    if (kp.pinEnv) {
      const v = process.env[kp.pinEnv];
      if (v === undefined) throw new CredentialError(`pkcs11 keyProvider.pinEnv '${kp.pinEnv}' is not set`);
      pin = v;
    } else if (kp.pin !== undefined) {
      pin = kp.pin;
    } else {
      throw new CredentialError("pkcs11 key provider requires keyProvider.pinEnv or keyProvider.pin");
    }
    // graphene-pk11 is synchronous, so the provider plugs straight into the sync LocalVault.open.
    const provider = await Pkcs11KeyProvider.create(kp.modulePath, kp.tokenLabel ?? "", kp.keyLabel, pin);
    return { provider };
  }

  throw new CredentialError(
    `key provider '${kind}' is not supported (supported: 'file', 'env', 'kms'/'greengrass', 'pkcs11')`,
  );
}

/**
 * Open (or create) the local vault under the configured key provider. Thin wrapper over
 * {@link buildKeyProvider} + {@link LocalVault.open} (the latter is synchronous). `defaultKeyProvider`
 * is the platform-profile default key-provider type used when `kp.type` is absent (see
 * {@link buildKeyProvider}).
 */
async function openVault(
  path: string,
  keep: number,
  kp: KeyProviderConfig,
  defaultKeyProvider?: string,
): Promise<LocalVault> {
  const { provider, newVaultId, newDek } = await buildKeyProvider(kp, path, `${path}.key`, defaultKeyProvider);
  return LocalVault.open(path, provider, keep, newVaultId, newDek);
}

/**
 * Open the vault and return the default credential service from a `credentials` config object.
 *
 * `defaultKeyProvider` is the platform-profile default vault key-provider type (e.g. `"env"` on
 * KUBERNETES, FR-CRED-6) supplied by the credentials init site where the resolved platform is known.
 * It only changes the DEFAULT provider TYPE used when `keyProvider.type` is absent — an explicit
 * `keyProvider.type` always wins, and this NEVER auto-enables credentials (the caller only invokes
 * this when a `credentials` config section is present).
 */
export async function openFromConfig(
  cfg: CredentialsConfig = {},
  namespace = "",
  defaultKeyProvider?: string,
): Promise<DefaultCredentialService> {
  const vaultCfg = cfg.vault ?? {};
  const path = vaultCfg.path ?? "vault";
  const keep = vaultCfg.keepVersions ?? 2;

  const vault = await openVault(path, keep, vaultCfg.keyProvider ?? {}, defaultKeyProvider);

  // Access auditing on by default (config can disable) — logs op/name/version/source/outcome,
  // never the value.
  const audit = cfg.audit?.enabled === false ? undefined : logSink();

  const central = cfg.central;
  const ctype = central?.type ?? "none";
  if (ctype === "none") {
    return new DefaultCredentialService(vault, namespace, undefined, audit);
  }
  if (ctype !== "awsSecretsManager") {
    throw new CredentialError(`central source '${ctype}' is not supported`);
  }

  const { AwsSecretsManagerSource } = await import("./central");
  const source = await AwsSecretsManagerSource.create(central!.region, central!.endpointUrl);
  const engine = await SyncEngine.start(
    vault,
    source,
    namespace,
    syncSecrets(central!),
    central!.refreshIntervalSecs ?? 300,
    central!.bootstrapOnStart ?? true,
  );
  return new DefaultCredentialService(vault, namespace, engine, audit);
}
