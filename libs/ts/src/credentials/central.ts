/** Central vault sources — the upstream a vault is seeded/refreshed from (AWS Secrets Manager). */
import { CredentialError } from "./errors";

export interface CentralSecret {
  bytes: Buffer;
  centralVersionId: string;
  labels: Record<string, string>;
}

/** The upstream source a vault syncs from (pull-only). */
export interface CentralVaultSource {
  fetch(name: string): Promise<CentralSecret | undefined>;
}

/**
 * Central source backed by AWS Secrets Manager (`@aws-sdk/client-secrets-manager`, loaded
 * dynamically so non-sync components don't pull it). Auth = default chain (TES on Greengrass);
 * `endpointUrl` overrides for an emulator (floci/LocalStack) or VPC endpoint.
 */
export class AwsSecretsManagerSource implements CentralVaultSource {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private constructor(private readonly client: any, private readonly GetSecretValueCommand: any) {}

  static async create(region?: string, endpointUrl?: string): Promise<AwsSecretsManagerSource> {
    let mod;
    try {
      mod = await import("@aws-sdk/client-secrets-manager");
    } catch {
      throw new CredentialError(
        "central source 'awsSecretsManager' requires the @aws-sdk/client-secrets-manager package",
      );
    }
    const client = new mod.SecretsManagerClient({ region, endpoint: endpointUrl });
    return new AwsSecretsManagerSource(client, mod.GetSecretValueCommand);
  }

  async fetch(name: string): Promise<CentralSecret | undefined> {
    try {
      const r = await this.client.send(new this.GetSecretValueCommand({ SecretId: name }));
      let bytes: Buffer;
      if (r.SecretString != null) {
        bytes = Buffer.from(r.SecretString, "utf-8");
      } else if (r.SecretBinary != null) {
        bytes = Buffer.from(r.SecretBinary);
      } else {
        return undefined;
      }
      return { bytes, centralVersionId: r.VersionId ?? "", labels: {} };
    } catch (e) {
      if ((e as { name?: string })?.name === "ResourceNotFoundException") {
        return undefined;
      }
      throw new CredentialError(`get secret '${name}': ${(e as Error)?.message ?? String(e)}`);
    }
  }
}
