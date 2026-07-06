/** Key providers (KEK custodians). Phase 1 ships {@link FileKeyProvider}. */
import { readFileSync, writeFileSync } from "fs";

import { KEY_LEN, NONCE_LEN, open, random, seal } from "./crypto";
import { CredentialError } from "./errors";
import { dekWrapAad, KekInfo } from "./format";

/** Wraps/unwraps the vault DEK without exposing the KEK. */
export interface KeyProvider {
  providerId(): string;
  wrapDek(vaultId: string, dek: Buffer): KekInfo;
  unwrapDek(vaultId: string, kek: KekInfo): Buffer;
}

/** KEK held as 32 bytes in a local key file (standalone / offline-fallback custodian). */
export class FileKeyProvider implements KeyProvider {
  private readonly kek: Buffer;

  constructor(kek: Buffer) {
    if (kek.length !== KEY_LEN) {
      throw new CredentialError(`KEK must be ${KEY_LEN} bytes`);
    }
    this.kek = Buffer.from(kek);
  }

  static fromKeyFile(path: string): FileKeyProvider {
    return new FileKeyProvider(readFileSync(path));
  }

  static generateKeyFile(path: string): FileKeyProvider {
    const kek = random(KEY_LEN);
    writeFileSync(path, kek, { mode: 0o600 });
    return new FileKeyProvider(kek);
  }

  providerId(): string {
    return "file";
  }

  wrapDek(vaultId: string, dek: Buffer): KekInfo {
    const nonce = random(NONCE_LEN);
    const wrapped = seal(this.kek, nonce, dekWrapAad(vaultId), dek);
    return {
      provider: "file",
      alg: "AES-256-GCM",
      wrapNonce: nonce.toString("base64"),
      wrappedDek: wrapped.toString("base64"),
    };
  }

  unwrapDek(vaultId: string, kek: KekInfo): Buffer {
    if (!kek.wrapNonce) {
      throw new CredentialError("file KEK: missing wrapNonce");
    }
    const nonce = Buffer.from(kek.wrapNonce, "base64");
    const wrapped = Buffer.from(kek.wrappedDek, "base64");
    return open(this.kek, nonce, dekWrapAad(vaultId), wrapped);
  }
}

/**
 * KEK sourced as a RAW 32-byte key, base64-encoded, from an environment variable (typically a mounted
 * Kubernetes Secret) — the offline-capable software-KEK and the DEFAULT vault custodian on the
 * KUBERNETES platform (FR-CRED-3/FR-CRED-6). Cryptographically IDENTICAL to {@link FileKeyProvider}
 * given the same raw 32-byte KEK: it delegates wrap/unwrap to an internal {@link FileKeyProvider}
 * (same AES-256-GCM, same {@link dekWrapAad} AAD), so a vault wrapped by this provider with KEK `K` is
 * byte-compatible with one wrapped by `FileKeyProvider` with the same `K`. The ONLY differences are
 * {@link providerId} (`"env"`) and the {@link KekInfo} `provider` tag it writes. The KEK never touches
 * disk — it is read from the env var (a mounted Secret), not a key file.
 */
export class EnvKeyProvider implements KeyProvider {
  /** Delegate that owns the identical AES-256-GCM DEK wrap/unwrap crypto. */
  private readonly delegate: FileKeyProvider;

  /** Wrap a raw 32-byte KEK; length is validated by the delegate {@link FileKeyProvider}. */
  constructor(kek: Buffer) {
    this.delegate = new FileKeyProvider(kek);
  }

  /**
   * Read the base64-encoded KEK from environment variable `envVar`, base64-decode it, and validate it
   * is EXACTLY {@link KEY_LEN} (32) bytes.
   *
   * @throws {@link CredentialError} if the env var is unset/empty, not valid base64, or the decoded
   *         key is not 32 bytes.
   */
  static fromEnv(envVar: string): EnvKeyProvider {
    const rawValue = process.env[envVar];
    if (rawValue === undefined || rawValue === "") {
      throw new CredentialError(`env key provider: environment variable '${envVar}' is unset or empty`);
    }
    // Tolerate surrounding whitespace / a trailing newline — common when the value is sourced from a
    // mounted file / Secret (echo|base64, kubectl --from-file). Matches canonical Java (b64.trim()) and
    // Rust (raw.trim()) so the same Secret decodes identically across all four languages.
    const b64 = rawValue.trim();
    // Strict standard base64 (no embedded whitespace) — mirrors Java's basic Base64 decoder. Node's
    // Buffer.from(.,"base64") is lenient and never throws, so validate the shape explicitly first.
    if (b64.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(b64)) {
      throw new CredentialError(`env key provider: environment variable '${envVar}' is not valid base64`);
    }
    const kek = Buffer.from(b64, "base64");
    if (kek.length !== KEY_LEN) {
      throw new CredentialError(
        `env key provider: decoded KEK from '${envVar}' must be ${KEY_LEN} bytes, got ${kek.length}`,
      );
    }
    return new EnvKeyProvider(kek);
  }

  providerId(): string {
    return "env";
  }

  /** Wrap the DEK exactly as {@link FileKeyProvider} does, tagging the {@link KekInfo} `provider:"env"`. */
  wrapDek(vaultId: string, dek: Buffer): KekInfo {
    return { ...this.delegate.wrapDek(vaultId, dek), provider: "env" };
  }

  /** Unwrap the DEK via the delegate (the `provider` tag is irrelevant to the crypto). */
  unwrapDek(vaultId: string, kek: KekInfo): Buffer {
    return this.delegate.unwrapDek(vaultId, kek);
  }
}

/**
 * KMS-wrapped DEK custodian (mirrors the Rust `kms` module): the DEK is encrypted by an AWS KMS CMK
 * (the KEK never leaves KMS) and unwrapped via `kms:Decrypt` — using the default AWS credential chain
 * / TES on Greengrass. The encryption context `{vaultId}` binds the wrapped DEK to the vault id
 * (anti-swap). On-disk this produces `KekInfo{provider:"kms", alg:"aws-kms", wrappedDek:base64(ct),
 * kmsKeyId}` with no `wrapNonce`.
 *
 * ## Sync-vs-async approach
 * `KeyProvider.wrapDek`/`unwrapDek` are synchronous because `LocalVault.open` calls them inline, but
 * KMS calls are async. We resolve the round trip eagerly in {@link config!openFromConfig} (async),
 * then hand `LocalVault.open` a {@link PrewrappedKeyProvider} whose sync methods just return the
 * precomputed values. This keeps `vault.open` and the cross-language on-disk format unchanged.
 */
export class KmsKeyProvider {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private constructor(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly client: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly EncryptCommand: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly DecryptCommand: any,
    private readonly keyId: string,
  ) {}

  /** Load `@aws-sdk/client-kms` (dynamically, so non-KMS components don't pull it) and bind a CMK. */
  static async create(keyId: string, region?: string, endpointUrl?: string): Promise<KmsKeyProvider> {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let mod: any;
    try {
      mod = await import("@aws-sdk/client-kms");
    } catch {
      throw new CredentialError("kms key provider requires the @aws-sdk/client-kms package");
    }
    const client = new mod.KMSClient({ region, endpoint: endpointUrl });
    return new KmsKeyProvider(client, mod.EncryptCommand, mod.DecryptCommand, keyId);
  }

  providerId(): string {
    return "kms";
  }

  /** KMS-encrypt `dek` under the CMK, binding it to `vaultId` via the encryption context. */
  async wrapDek(vaultId: string, dek: Buffer): Promise<KekInfo> {
    let resp;
    try {
      resp = await this.client.send(
        new this.EncryptCommand({
          KeyId: this.keyId,
          Plaintext: dek,
          EncryptionContext: { vaultId },
        }),
      );
    } catch (e) {
      throw new CredentialError(`kms encrypt: ${(e as Error)?.message ?? String(e)}`);
    }
    if (!resp.CiphertextBlob) {
      throw new CredentialError("kms encrypt: no ciphertext");
    }
    return {
      provider: "kms",
      alg: "aws-kms",
      wrappedDek: Buffer.from(resp.CiphertextBlob).toString("base64"),
      kmsKeyId: this.keyId,
    };
  }

  /** KMS-decrypt the wrapped DEK described by `kek`, asserting the `vaultId` encryption context. */
  async unwrapDek(vaultId: string, kek: KekInfo): Promise<Buffer> {
    const ct = Buffer.from(kek.wrappedDek, "base64");
    let resp;
    try {
      resp = await this.client.send(
        new this.DecryptCommand({
          CiphertextBlob: ct,
          KeyId: this.keyId,
          EncryptionContext: { vaultId },
        }),
      );
    } catch (e) {
      throw new CredentialError(`kms decrypt: ${(e as Error)?.message ?? String(e)}`);
    }
    if (!resp.Plaintext) {
      throw new CredentialError("kms decrypt: no plaintext");
    }
    const pt = Buffer.from(resp.Plaintext);
    if (pt.length !== KEY_LEN) {
      throw new CredentialError("kms: unwrapped DEK wrong length");
    }
    return pt;
  }
}

/**
 * In-memory {@link KeyProvider} shim returning a pre-resolved KEK/DEK. Used to bridge the async KMS
 * round trip into `LocalVault.open`'s synchronous `wrapDek`/`unwrapDek` (see {@link KmsKeyProvider}).
 */
export class PrewrappedKeyProvider implements KeyProvider {
  constructor(
    private readonly id: string,
    private readonly kek: KekInfo,
    private readonly dek: Buffer,
  ) {}

  providerId(): string {
    return this.id;
  }

  wrapDek(): KekInfo {
    return this.kek;
  }

  unwrapDek(): Buffer {
    return Buffer.from(this.dek);
  }
}

/**
 * PKCS#11 (HSM/TPM/SoftHSM) DEK custodian — mirrors the Rust `Pkcs11KeyProvider`. A non-extractable
 * AES-256 key on the token is the KEK; the DEK is wrapped with AES-256-GCM *inside* the token (so the
 * KEK never leaves hardware). The GCM AAD binds the wrapped DEK to the vault id (anti-swap), so the
 * on-disk {@link KekInfo} shape is identical to {@link FileKeyProvider} (provider `"pkcs11"`, alg
 * `"AES-256-GCM"`, wrapNonce + wrappedDek).
 *
 * `graphene-pk11` (over `pkcs11js`) is synchronous, so `wrapDek`/`unwrapDek` stay synchronous and the
 * provider plugs straight into `LocalVault.open` — no async shim needed (unlike {@link KmsKeyProvider}).
 * Only the module load is async, done in {@link Pkcs11KeyProvider.create}. The binding is loaded
 * dynamically + listed in optionalDependencies, so non-HSM components don't pull the native addon.
 */
export class Pkcs11KeyProvider implements KeyProvider {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private constructor(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly g: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly session: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly key: any,
  ) {}

  /** Load the module, select the slot whose token has `tokenLabel`, log in, and bind `keyLabel`. */
  static async create(
    modulePath: string,
    tokenLabel: string,
    keyLabel: string,
    pin: string,
  ): Promise<Pkcs11KeyProvider> {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let mod: any;
    // Non-literal specifier so tsc treats this as a dynamic `any` import and does not require the
    // optional native package's types at compile time (graphene-pk11 is an optionalDependency).
    const pkg = "graphene-pk11";
    try {
      mod = await import(pkg);
    } catch {
      throw new CredentialError("pkcs11 key provider requires the graphene-pk11 package");
    }
    const g = mod.default ?? mod;
    const module = g.Module.load(modulePath, "edgecommons-vault");
    try {
      module.initialize();
    } catch (e) {
      // CKR_CRYPTOKI_ALREADY_INITIALIZED — another provider already initialized this module.
      if (!/ALREADY_INITIALIZED|0x191|already/i.test(String((e as Error)?.message ?? e))) {
        throw new CredentialError(`pkcs11 initialize: ${(e as Error)?.message ?? String(e)}`);
      }
    }
    const slots = module.getSlots(true);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let slot: any;
    for (let i = 0; i < slots.length; i++) {
      const s = slots.items(i);
      try {
        if (s.getToken().label.trim() === tokenLabel) {
          slot = s;
          break;
        }
      } catch {
        // Uninitialized / unreadable slot — skip.
      }
    }
    if (!slot) {
      throw new CredentialError(`pkcs11: no token labelled '${tokenLabel}' (module ${modulePath})`);
    }
    const session = slot.open(g.SessionFlag.SERIAL_SESSION | g.SessionFlag.RW_SESSION);
    try {
      session.login(pin);
    } catch (e) {
      // CKR_USER_ALREADY_LOGGED_IN — login state is per-token across sessions; another provider in
      // this process already logged in, which is fine.
      if (!/ALREADY_LOGGED_IN|0x100|already logged/i.test(String((e as Error)?.message ?? e))) {
        throw new CredentialError(`pkcs11 login: ${(e as Error)?.message ?? String(e)}`);
      }
    }
    const found = session.find({ class: g.ObjectClass.SECRET_KEY, label: keyLabel });
    if (found.length === 0) {
      throw new CredentialError(`pkcs11: no key labelled '${keyLabel}'`);
    }
    return new Pkcs11KeyProvider(g, session, found.items(0).toType());
  }

  providerId(): string {
    return "pkcs11";
  }

  wrapDek(vaultId: string, dek: Buffer): KekInfo {
    const iv = random(NONCE_LEN);
    const alg = { name: "AES_GCM", params: new this.g.AesGcmParams(iv, dekWrapAad(vaultId), 128) };
    let ct: Buffer;
    try {
      ct = Buffer.from(this.session.createCipher(alg, this.key).once(dek, Buffer.alloc(dek.length + 16)));
    } catch (e) {
      throw new CredentialError(`pkcs11 wrap: ${(e as Error)?.message ?? String(e)}`);
    }
    return {
      provider: "pkcs11",
      alg: "AES-256-GCM",
      wrapNonce: iv.toString("base64"),
      wrappedDek: ct.toString("base64"),
    };
  }

  unwrapDek(vaultId: string, kek: KekInfo): Buffer {
    if (!kek.wrapNonce) {
      throw new CredentialError("pkcs11 KEK: missing wrapNonce");
    }
    const iv = Buffer.from(kek.wrapNonce, "base64");
    const ct = Buffer.from(kek.wrappedDek, "base64");
    const alg = { name: "AES_GCM", params: new this.g.AesGcmParams(iv, dekWrapAad(vaultId), 128) };
    try {
      return Buffer.from(this.session.createDecipher(alg, this.key).once(ct, Buffer.alloc(ct.length)));
    } catch (e) {
      throw new CredentialError(`pkcs11 unwrap: ${(e as Error)?.message ?? String(e)}`);
    }
  }
}
