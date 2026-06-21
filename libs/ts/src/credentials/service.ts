/** Credential service (the public seam) + the default LocalVault-backed implementation. */
import { AuditSink } from "./audit";
import { CredentialError } from "./errors";
import { SyncEngine } from "./sync";
import { LocalVault, PutOptions } from "./vault";
import { CredentialStats, Secret, SecretMeta } from "./types";
import { AwsCredentials, BasicAuth, KafkaSasl, TlsBundle } from "./views";

/** The public credential interface (depend on this). Obtained via `gg.credentials()`. */
export interface CredentialService {
  get(name: string): Secret | undefined;
  getVersion(name: string, version: string): Secret | undefined;
  exists(name: string): boolean;
  list(prefix?: string): SecretMeta[];
  versions(name: string): string[];
  put(name: string, value: Buffer, opts?: PutOptions): string;
  delete(name: string): boolean;
  /** Force an immediate pull from the central source (no-op without central sync). */
  refresh(): Promise<void>;
  /** Non-sensitive stats for observability (never includes values). */
  stats(): CredentialStats;

  getBytes(name: string): Buffer | undefined;
  getString(name: string): string | undefined;
  getJson(name: string): unknown | undefined;

  // typed views
  getAwsCredentials(name: string): AwsCredentials | undefined;
  getBasicAuth(name: string): BasicAuth | undefined;
  getTlsBundle(name: string): TlsBundle | undefined;
  getKafkaSasl(name: string): KafkaSasl | undefined;
}

/**
 * The default {@link CredentialService}: a {@link LocalVault} that refreshes any cross-process
 * change on each read. `namespace` (`<thingName>/<componentName>`) is prepended transparently to
 * every key and stripped from returned names, so a shared device vault can't collide across
 * components. Node is single-threaded, so no in-process lock is needed.
 */
export class DefaultCredentialService implements CredentialService {
  constructor(
    private readonly vault: LocalVault,
    private readonly namespace = "",
    private readonly sync?: SyncEngine,
    /** Audit sink for access events (`undefined` = auditing off). Set via {@link withAudit};
     * the config path enables it (`credentials.audit.enabled`) with the default logging sink. */
    private auditSink?: AuditSink,
  ) {}

  /** Attach (or clear) the audit sink — access events are emitted to it. Fluent; returns `this`. */
  withAudit(sink: AuditSink | undefined): this {
    this.auditSink = sink;
    return this;
  }

  /** Emit an audit event if an audit sink is configured (no-op otherwise). Never the value. */
  private audit(op: string, name: string, version: string, source: string, outcome: string): void {
    this.auditSink?.record({ op, name, version, source, outcome });
  }

  private full(name: string): string {
    return this.namespace ? `${this.namespace}/${name}` : name;
  }

  private rel(full: string): string {
    const prefix = `${this.namespace}/`;
    return this.namespace && full.startsWith(prefix) ? full.slice(prefix.length) : full;
  }

  private relName(s: Secret): Secret {
    return new Secret(this.rel(s.name), s.version, s.bytes(), s.labels, s.createdMs, s.source, s.contentType);
  }

  get(name: string): Secret | undefined {
    this.vault.reloadIfChanged();
    const raw = this.vault.get(this.full(name));
    const s = raw ? this.relName(raw) : undefined;
    if (s) this.audit("get", name, s.version, s.source, "hit");
    else this.audit("get", name, "-", "-", "miss");
    return s;
  }

  getVersion(name: string, version: string): Secret | undefined {
    this.vault.reloadIfChanged();
    const raw = this.vault.getVersion(this.full(name), version);
    const s = raw ? this.relName(raw) : undefined;
    if (s) this.audit("get", name, s.version, s.source, "hit");
    else this.audit("get", name, version, "-", "miss");
    return s;
  }

  exists(name: string): boolean {
    this.vault.reloadIfChanged();
    return this.vault.exists(this.full(name));
  }

  list(prefix = ""): SecretMeta[] {
    this.vault.reloadIfChanged();
    return this.vault.list(this.full(prefix)).map((m) => ({ ...m, name: this.rel(m.name) }));
  }

  versions(name: string): string[] {
    this.vault.reloadIfChanged();
    return this.vault.versions(this.full(name));
  }

  put(name: string, value: Buffer, opts: PutOptions = {}): string {
    this.vault.reloadIfChanged();
    const version = this.vault.put(this.full(name), value, opts);
    this.audit("put", name, version, "local", "ok");
    return version;
  }

  delete(name: string): boolean {
    this.vault.reloadIfChanged();
    const deleted = this.vault.delete(this.full(name));
    this.audit("delete", name, "-", "-", deleted ? "ok" : "miss");
    return deleted;
  }

  async refresh(): Promise<void> {
    if (this.sync) await this.sync.syncNow();
  }

  stats(): CredentialStats {
    const secretCount = this.list("").length;
    if (!this.sync) {
      return { secretCount, syncFailures: 0, rotations: 0 };
    }
    const s = this.sync.stats();
    return {
      secretCount,
      lastSyncAgeMs: s.lastSuccessMs !== undefined ? Math.max(0, Date.now() - s.lastSuccessMs) : undefined,
      syncFailures: s.failures,
      rotations: s.rotations,
    };
  }

  getBytes(name: string): Buffer | undefined {
    return this.get(name)?.bytes();
  }

  getString(name: string): string | undefined {
    return this.get(name)?.asString();
  }

  getJson(name: string): unknown | undefined {
    return this.get(name)?.asJson();
  }

  // ----- typed views (thin parses over the opaque secret; canonical camelCase JSON) -----

  getAwsCredentials(name: string): AwsCredentials | undefined {
    const j = this.viewJson(name);
    if (!j) return undefined;
    if (typeof j.accessKeyId !== "string" || typeof j.secretAccessKey !== "string") {
      throw new CredentialError(`secret '${name}' is not AWS credentials (missing fields)`);
    }
    return { accessKeyId: j.accessKeyId, secretAccessKey: j.secretAccessKey, sessionToken: j.sessionToken, expiry: j.expiry };
  }

  getBasicAuth(name: string): BasicAuth | undefined {
    const j = this.viewJson(name);
    if (!j) return undefined;
    if (typeof j.username !== "string" || typeof j.password !== "string") {
      throw new CredentialError(`secret '${name}' is not basic auth (missing fields)`);
    }
    return { username: j.username, password: j.password };
  }

  getTlsBundle(name: string): TlsBundle | undefined {
    const j = this.viewJson(name);
    if (!j) return undefined;
    if (typeof j.certPem !== "string" || typeof j.keyPem !== "string") {
      throw new CredentialError(`secret '${name}' is not a TLS bundle (missing fields)`);
    }
    return { certPem: j.certPem, keyPem: j.keyPem, caPem: j.caPem };
  }

  getKafkaSasl(name: string): KafkaSasl | undefined {
    const j = this.viewJson(name);
    if (!j) return undefined;
    if (typeof j.username !== "string" || typeof j.password !== "string") {
      throw new CredentialError(`secret '${name}' is not Kafka SASL (missing fields)`);
    }
    return { mechanism: typeof j.mechanism === "string" ? j.mechanism : "PLAIN", username: j.username, password: j.password };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private viewJson(name: string): any | undefined {
    return this.get(name)?.asJson();
  }
}
