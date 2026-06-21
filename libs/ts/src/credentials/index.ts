/**
 * Credentials & local vault — a generic encrypted-at-rest secret store for TypeScript components.
 *
 * Named, versioned, opaque-byte secrets in an encrypted local vault that runs standalone or (later
 * phases) is seeded/refreshed from a central cloud vault. The on-disk format is byte-compatible
 * with the Rust/Python/Java ports (see `vault-test-vectors/` and `docs/CREDENTIALS.md`).
 */
export { openFromConfig, CredentialsConfig } from "./config";
export { CredentialError } from "./errors";
export { CredentialService, DefaultCredentialService } from "./service";
export {
  FileKeyProvider,
  KeyProvider,
  KmsKeyProvider,
  Pkcs11KeyProvider,
  PrewrappedKeyProvider,
} from "./keyprovider";
export { LocalVault, PutOptions } from "./vault";
export { CredentialStats, Secret, SecretMeta } from "./types";
export { AwsSecretsManagerSource, CentralSecret, CentralVaultSource } from "./central";
export { SyncEngine, SyncSecret, SyncStats } from "./sync";
export { AwsCredentials, BasicAuth, KafkaSasl, TlsBundle } from "./views";
export { resolveSecretRefs } from "./secretref";
export { CredentialMetricsBridge } from "./bridge";
