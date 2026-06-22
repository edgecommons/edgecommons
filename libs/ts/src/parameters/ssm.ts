/**
 * AWS SSM Parameter Store source — TypeScript port of the Rust `parameters::ssm`.
 *
 * Reads parameters from AWS SSM via `GetParameter` / `GetParametersByPath` (with decryption, so
 * `SecureString`s resolve and are flagged `secure`). The `@aws-sdk/client-ssm` package is loaded
 * dynamically (it is an optionalDependency) so components that use only `env`/`mountedDir` never
 * pull the AWS SDK. Auth = the default credential chain (TES on Greengrass, ambient creds in
 * STANDALONE); `endpointUrl` overrides for floci/LocalStack/VPC endpoints.
 */
import { ParameterError } from "./errors";
import { ParamValue, ParameterSource } from "./source";

/** AWS SSM Parameter Store {@link ParameterSource}. */
export class AwsSsmSource implements ParameterSource {
  private constructor(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly client: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly GetParameterCommand: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly GetParametersByPathCommand: any,
    private readonly withDecryption: boolean,
  ) {}

  /** Load `@aws-sdk/client-ssm` (dynamically) and build the SSM client. */
  static async create(
    region?: string,
    endpointUrl?: string,
    withDecryption = true,
  ): Promise<AwsSsmSource> {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let mod: any;
    // Non-literal specifier so tsc treats this as a dynamic `any` import and does not require the
    // optional package's types at compile time (@aws-sdk/client-ssm is an optionalDependency).
    const pkg = "@aws-sdk/client-ssm";
    try {
      mod = await import(pkg);
    } catch {
      throw new ParameterError("parameter source 'awsSsm' requires the @aws-sdk/client-ssm package");
    }
    const client = new mod.SSMClient({ region, endpoint: endpointUrl });
    return new AwsSsmSource(client, mod.GetParameterCommand, mod.GetParametersByPathCommand, withDecryption);
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private toValue(p: any): ParamValue | undefined {
    if (p?.Value == null) return undefined;
    const secure = p.Type === "SecureString";
    const version = p.Version != null ? String(p.Version) : undefined;
    return { value: Buffer.from(String(p.Value), "utf-8"), secure, version };
  }

  async fetch(name: string): Promise<ParamValue | undefined> {
    try {
      const r = await this.client.send(
        new this.GetParameterCommand({ Name: name, WithDecryption: this.withDecryption }),
      );
      return r.Parameter ? this.toValue(r.Parameter) : undefined;
    } catch (e) {
      if ((e as { name?: string })?.name === "ParameterNotFound") return undefined;
      throw new ParameterError(`ssm get_parameter: ${(e as Error)?.message ?? String(e)}`);
    }
  }

  async fetchByPath(path: string, recursive: boolean): Promise<Array<[string, ParamValue]>> {
    const out: Array<[string, ParamValue]> = [];
    let next: string | undefined;
    for (;;) {
      let resp;
      try {
        resp = await this.client.send(
          new this.GetParametersByPathCommand({
            Path: path,
            Recursive: recursive,
            WithDecryption: this.withDecryption,
            NextToken: next,
          }),
        );
      } catch (e) {
        throw new ParameterError(`ssm get_parameters_by_path: ${(e as Error)?.message ?? String(e)}`);
      }
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      for (const p of (resp.Parameters ?? []) as any[]) {
        const v = this.toValue(p);
        if (p?.Name && v) out.push([String(p.Name), v]);
      }
      next = resp.NextToken;
      if (!next) break;
    }
    return out;
  }

  sourceId(): string {
    return "awsSsm";
  }
}
