/** Parse the `credentials` config section and build a service (phase 1: file provider, no central). */
import { existsSync, mkdirSync } from "fs";
import { dirname } from "path";

import { CredentialError } from "./errors";
import { FileKeyProvider } from "./keyprovider";
import { DefaultCredentialService } from "./service";
import { LocalVault } from "./vault";

export interface CredentialsConfig {
  vault?: {
    path?: string;
    keepVersions?: number;
    keyProvider?: { type?: string; keyPath?: string; kmsKeyId?: string; region?: string };
  };
  central?: { type?: string };
}

/** Open the vault and return the default credential service from a `credentials` config object. */
export function openFromConfig(cfg: CredentialsConfig = {}): DefaultCredentialService {
  const vaultCfg = cfg.vault ?? {};
  const path = vaultCfg.path ?? "vault";
  const keep = vaultCfg.keepVersions ?? 2;
  const kind = vaultCfg.keyProvider?.type ?? "file";
  if (kind !== "file") {
    throw new CredentialError(`key provider '${kind}' is not implemented yet (phase 1 supports 'file')`);
  }
  const central = cfg.central?.type ?? "none";
  if (central !== "none") {
    throw new CredentialError(`central source '${central}' is not implemented yet (phase 2)`);
  }

  const keyPath = vaultCfg.keyProvider?.keyPath ?? `${path}.key`;
  const dir = dirname(keyPath);
  if (dir) {
    mkdirSync(dir, { recursive: true });
  }
  const provider = existsSync(keyPath)
    ? FileKeyProvider.fromKeyFile(keyPath)
    : FileKeyProvider.generateKeyFile(keyPath);

  return new DefaultCredentialService(LocalVault.open(path, provider, keep));
}
