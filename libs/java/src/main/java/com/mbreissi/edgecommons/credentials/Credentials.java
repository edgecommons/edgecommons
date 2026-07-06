package com.mbreissi.edgecommons.credentials;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

/**
 * Factory: build a {@link CredentialService} from the {@code credentials} config section.
 *
 * <p>Supports the {@code file} key provider and the {@code awsSecretsManager} central source.
 * {@code namespace} (<thingName>/<componentName>) is applied transparently to every key.
 */
public final class Credentials {
    private Credentials() {}

    public static CredentialService open(JsonObject credentialsConfig) {
        return open(credentialsConfig, "");
    }

    public static CredentialService open(JsonObject credentialsConfig, String namespace) {
        return open(credentialsConfig, namespace, null);
    }

    /**
     * Open the credential service, applying a platform-derived default key-provider type when the
     * {@code credentials.vault.keyProvider.type} field is absent (FR-CRED-6, precedence FR-RT-3).
     *
     * <p>The effective key-provider type follows: explicit {@code keyProvider.type} ▸
     * {@code defaultKeyProviderType} (the platform-profile default — {@code "env"} on KUBERNETES) ▸
     * the library default {@code "file"}. This method is reached <b>only when a {@code credentials}
     * config section is present</b>, so it never auto-enables credentials — it only changes the
     * default provider <em>type</em> for an already-configured vault.
     *
     * @param credentialsConfig      the {@code credentials} config section
     * @param namespace              the per-component key namespace
     * @param defaultKeyProviderType the platform-profile default provider type, or {@code null} for
     *                               none (then the library default {@code "file"} applies)
     * @return the constructed credential service
     */
    public static CredentialService open(JsonObject credentialsConfig, String namespace,
                                         String defaultKeyProviderType) {
        JsonObject cfg = credentialsConfig != null ? credentialsConfig : new JsonObject();
        JsonObject vaultCfg = cfg.has("vault") ? cfg.getAsJsonObject("vault") : new JsonObject();
        String path = vaultCfg.has("path") ? vaultCfg.get("path").getAsString() : "vault";
        int keep = vaultCfg.has("keepVersions") ? vaultCfg.get("keepVersions").getAsInt() : 2;

        JsonObject kp = vaultCfg.has("keyProvider") ? vaultCfg.getAsJsonObject("keyProvider") : new JsonObject();
        KeyProvider provider = buildKeyProvider(kp, path + ".key", defaultKeyProviderType);

        LocalVault vault = LocalVault.open(Paths.get(path), provider, keep);
        Object lock = new Object();

        // Access auditing on by default (config can disable) — logs op/name/version/source/outcome,
        // never the value.
        JsonObject auditCfg = cfg.has("audit") ? cfg.getAsJsonObject("audit") : new JsonObject();
        boolean auditEnabled = !auditCfg.has("enabled") || auditCfg.get("enabled").getAsBoolean();
        AuditSink audit = auditEnabled ? new LogAuditSink() : null;

        JsonObject central = cfg.has("central") ? cfg.getAsJsonObject("central") : new JsonObject();
        String ctype = central.has("type") ? central.get("type").getAsString() : "none";
        if ("none".equals(ctype)) {
            return new DefaultCredentialService(vault, namespace, lock, null).withAudit(audit);
        }
        if (!"awsSecretsManager".equals(ctype)) {
            throw new CredentialException("central source '" + ctype + "' is not supported");
        }

        String region = central.has("region") ? central.get("region").getAsString() : null;
        String endpoint = central.has("endpointUrl") ? central.get("endpointUrl").getAsString() : null;
        long interval = central.has("refreshIntervalSecs") ? central.get("refreshIntervalSecs").getAsLong() : 300;
        boolean bootstrap = !central.has("bootstrapOnStart") || central.get("bootstrapOnStart").getAsBoolean();

        AwsSecretsManagerSource source = new AwsSecretsManagerSource(region, endpoint);
        SyncEngine sync = new SyncEngine(vault, lock, source, namespace, syncSecrets(central), interval, bootstrap);
        return new DefaultCredentialService(vault, namespace, lock, sync).withAudit(audit);
    }

    /**
     * Build a {@link KeyProvider} (the KEK custodian) from a {@code keyProvider} config object.
     *
     * <p>Supports {@code file} (default), {@code env} (base64 32-byte KEK from an env var),
     * {@code kms}/{@code greengrass} (KMS-via-TES) and {@code pkcs11} (HSM/TPM). Mirrors the Rust
     * {@code build_key_provider}. Shared by the credentials vault and the {@code parameters}
     * persistent cache so both apply identical key-provider semantics. This 2-arg form keeps the
     * library default {@code file} when {@code keyProvider.type} is absent (parameters cache behavior
     * is unchanged); see {@link #buildKeyProvider(JsonObject, String, String)} for the platform-default
     * variant.
     *
     * @param kp             the {@code keyProvider} config object (may be empty → defaults to {@code file})
     * @param defaultKeyPath the on-disk key path used by the {@code file} provider when
     *                       {@code keyProvider.keyPath} is absent (e.g. {@code <vaultPath>.key})
     * @return the constructed {@link KeyProvider}
     */
    public static KeyProvider buildKeyProvider(JsonObject kp, String defaultKeyPath) {
        return buildKeyProvider(kp, defaultKeyPath, null);
    }

    /**
     * Build a {@link KeyProvider} as {@link #buildKeyProvider(JsonObject, String)}, but applying
     * {@code defaultType} when {@code keyProvider.type} is absent (FR-CRED-6, precedence FR-RT-3).
     *
     * <p>The effective type is: explicit {@code keyProvider.type} ▸ {@code defaultType} (the
     * platform-profile default — {@code "env"} on KUBERNETES) ▸ the library default {@code "file"}.
     * Adds the {@code env} arm: a base64 32-byte KEK read from the env var named by
     * {@code keyProvider.envVar} (default {@link EnvKeyProvider#DEFAULT_ENV_VAR}). All other arms are
     * unchanged.
     *
     * @param kp             the {@code keyProvider} config object (may be empty)
     * @param defaultKeyPath the {@code file}-provider key path used when {@code keyProvider.keyPath}
     *                       is absent
     * @param defaultType    the default provider type when {@code keyProvider.type} is absent, or
     *                       {@code null} for none (then the library default {@code "file"} applies)
     * @return the constructed {@link KeyProvider}
     */
    public static KeyProvider buildKeyProvider(JsonObject kp, String defaultKeyPath, String defaultType) {
        JsonObject cfg = kp != null ? kp : new JsonObject();
        String fallback = (defaultType != null && !defaultType.isEmpty()) ? defaultType : "file";
        String kind = cfg.has("type") && !cfg.get("type").isJsonNull() ? cfg.get("type").getAsString() : fallback;
        return switch (kind) {
            case "file" -> {
                String keyPath = cfg.has("keyPath") ? cfg.get("keyPath").getAsString() : defaultKeyPath;
                Path keyFile = Paths.get(keyPath);
                try {
                    if (keyFile.getParent() != null) {
                        Files.createDirectories(keyFile.getParent());
                    }
                } catch (IOException e) {
                    throw new CredentialException("create key dir: " + e.getMessage(), e);
                }
                yield Files.exists(keyFile)
                        ? FileKeyProvider.fromKeyFile(keyFile)
                        : FileKeyProvider.generateKeyFile(keyFile);
            }
            case "env" -> {
                String envVar = cfg.has("envVar") && !cfg.get("envVar").isJsonNull()
                        ? cfg.get("envVar").getAsString()
                        : EnvKeyProvider.DEFAULT_ENV_VAR;
                yield EnvKeyProvider.fromEnv(envVar);
            }
            case "kms", "greengrass" -> {
                if (!cfg.has("kmsKeyId")) {
                    throw new CredentialException("kms key provider requires keyProvider.kmsKeyId");
                }
                String keyId = cfg.get("kmsKeyId").getAsString();
                String kmsRegion = cfg.has("region") ? cfg.get("region").getAsString() : null;
                String kmsEndpoint = cfg.has("endpointUrl") ? cfg.get("endpointUrl").getAsString() : null;
                yield new KmsKeyProvider(keyId, kmsRegion, kmsEndpoint);
            }
            case "pkcs11" -> {
                if (!cfg.has("modulePath")) {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.modulePath");
                }
                if (!cfg.has("keyLabel")) {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.keyLabel");
                }
                String modulePath = cfg.get("modulePath").getAsString();
                String tokenLabel = cfg.has("tokenLabel") ? cfg.get("tokenLabel").getAsString() : "";
                String keyLabel = cfg.get("keyLabel").getAsString();
                String pin;
                if (cfg.has("pinEnv")) {
                    pin = System.getenv(cfg.get("pinEnv").getAsString());
                    if (pin == null) {
                        throw new CredentialException(
                                "pkcs11 keyProvider.pinEnv '" + cfg.get("pinEnv").getAsString() + "' is not set");
                    }
                } else if (cfg.has("pin")) {
                    pin = cfg.get("pin").getAsString();
                } else {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.pinEnv or keyProvider.pin");
                }
                yield Pkcs11KeyProvider.create(modulePath, tokenLabel, keyLabel, pin);
            }
            default -> throw new CredentialException(
                    "key provider '" + kind + "' is not supported (supported: 'file', 'env', 'kms'/'greengrass', 'pkcs11')");
        };
    }

    /** Parse {@code central.sync.secrets} — each entry a bare string or {@code {name, from}}. */
    private static List<SyncEngine.SyncSecret> syncSecrets(JsonObject central) {
        List<SyncEngine.SyncSecret> out = new ArrayList<>();
        if (!central.has("sync")) {
            return out;
        }
        JsonObject sync = central.getAsJsonObject("sync");
        if (!sync.has("secrets")) {
            return out;
        }
        for (JsonElement el : sync.getAsJsonArray("secrets")) {
            if (el.isJsonPrimitive()) {
                out.add(new SyncEngine.SyncSecret(el.getAsString(), null));
            } else if (el.isJsonObject()) {
                JsonObject o = el.getAsJsonObject();
                String name = o.get("name").getAsString();
                String from = o.has("from") ? o.get("from").getAsString() : null;
                out.add(new SyncEngine.SyncSecret(name, from));
            }
        }
        return out;
    }
}
