/** Parse the `credentials` config section and build a service.
 *
 * Phase 1: file key provider + local vault. Phase 2: `awsSecretsManager` central source + sync.
 * `namespace` (`<thingName>/<componentName>`) is applied transparently to every key. Async because
 * the AWS SDK (and thus bootstrap) is promise-based.
 */
import { existsSync, mkdirSync } from "fs";
import { dirname } from "path";

import { CredentialError } from "./errors";
import { FileKeyProvider } from "./keyprovider";
import { DefaultCredentialService } from "./service";
import { SyncEngine, SyncSecret } from "./sync";
import { LocalVault } from "./vault";

export interface CredentialsConfig {
  vault?: {
    path?: string;
    keepVersions?: number;
    keyProvider?: { type?: string; keyPath?: string; kmsKeyId?: string; region?: string };
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

/** Open the vault and return the default credential service from a `credentials` config object. */
export async function openFromConfig(
  cfg: CredentialsConfig = {},
  namespace = "",
): Promise<DefaultCredentialService> {
  const vaultCfg = cfg.vault ?? {};
  const path = vaultCfg.path ?? "vault";
  const keep = vaultCfg.keepVersions ?? 2;
  const kind = vaultCfg.keyProvider?.type ?? "file";
  if (kind !== "file") {
    throw new CredentialError(`key provider '${kind}' is not implemented yet (phase 1 supports 'file')`);
  }
  const keyPath = vaultCfg.keyProvider?.keyPath ?? `${path}.key`;
  const dir = dirname(keyPath);
  if (dir) mkdirSync(dir, { recursive: true });
  const provider = existsSync(keyPath)
    ? FileKeyProvider.fromKeyFile(keyPath)
    : FileKeyProvider.generateKeyFile(keyPath);

  const vault = LocalVault.open(path, provider, keep);

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
