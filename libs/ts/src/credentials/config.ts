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
  FileKeyProvider,
  KeyProvider,
  KmsKeyProvider,
  Pkcs11KeyProvider,
  PrewrappedKeyProvider,
} from "./keyprovider";
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
}

function syncSecrets(central: NonNullable<CredentialsConfig["central"]>): SyncSecret[] {
  const out: SyncSecret[] = [];
  for (const entry of central.sync?.secrets ?? []) {
    if (typeof entry === "string") out.push([entry, undefined]);
    else if (entry && entry.name) out.push([entry.name, entry.from]);
  }
  return out;
}

/**
 * Open (or create) the local vault under the configured key provider. `file` wraps the DEK under a
 * local key file; `kms`/`greengrass` wrap it via an AWS KMS CMK. The KMS round trip is performed
 * eagerly here (async) and handed to `LocalVault.open` through a {@link PrewrappedKeyProvider}, since
 * `LocalVault.open` calls `wrapDek`/`unwrapDek` synchronously. The on-disk format is unchanged.
 */
async function openVault(
  kind: string,
  path: string,
  keep: number,
  kp: NonNullable<NonNullable<CredentialsConfig["vault"]>["keyProvider"]>,
): Promise<LocalVault> {
  if (kind === "file") {
    const keyPath = kp.keyPath ?? `${path}.key`;
    const dir = dirname(keyPath);
    if (dir) mkdirSync(dir, { recursive: true });
    const provider: KeyProvider = existsSync(keyPath)
      ? FileKeyProvider.fromKeyFile(keyPath)
      : FileKeyProvider.generateKeyFile(keyPath);
    return LocalVault.open(path, provider, keep);
  }

  if (kind === "kms" || kind === "greengrass") {
    if (!kp.kmsKeyId) {
      throw new CredentialError("kms key provider requires keyProvider.kmsKeyId");
    }
    const kms = await KmsKeyProvider.create(kp.kmsKeyId, kp.region, kp.endpointUrl);
    if (existsSync(path)) {
      // Existing vault: read its KEK + vaultId and KMS-decrypt the DEK eagerly.
      const dir = dirname(path);
      if (dir) mkdirSync(dir, { recursive: true });
      const vf = JSON.parse(readFileSync(path, "utf8")) as VaultFile;
      const dek = await kms.unwrapDek(vf.vaultId, vf.kek);
      const shim = new PrewrappedKeyProvider("kms", vf.kek, dek);
      return LocalVault.open(path, shim, keep);
    }
    // New vault: generate the id + DEK here, KMS-wrap eagerly, then hand both to LocalVault.open.
    const vaultId = randomUUID();
    const dek = random(KEY_LEN);
    const kek = await kms.wrapDek(vaultId, dek);
    const shim = new PrewrappedKeyProvider("kms", kek, dek);
    return LocalVault.open(path, shim, keep, vaultId, dek);
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
    return LocalVault.open(path, provider, keep);
  }

  throw new CredentialError(
    `key provider '${kind}' is not supported (supported: 'file', 'kms'/'greengrass', 'pkcs11')`,
  );
}

/** Open the vault and return the default credential service from a `credentials` config object. */
export async function openFromConfig(
  cfg: CredentialsConfig = {},
  namespace = "",
): Promise<DefaultCredentialService> {
  const vaultCfg = cfg.vault ?? {};
  const path = vaultCfg.path ?? "vault";
  const keep = vaultCfg.keepVersions ?? 2;
  const kind = vaultCfg.keyProvider?.type ?? "file";

  const vault = await openVault(kind, path, keep, vaultCfg.keyProvider ?? {});

  const central = cfg.central;
  const ctype = central?.type ?? "none";
  if (ctype === "none") {
    return new DefaultCredentialService(vault, namespace);
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
  return new DefaultCredentialService(vault, namespace, engine);
}
