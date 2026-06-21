package com.aws.proserve.ggcommons.credentials;

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
        JsonObject cfg = credentialsConfig != null ? credentialsConfig : new JsonObject();
        JsonObject vaultCfg = cfg.has("vault") ? cfg.getAsJsonObject("vault") : new JsonObject();
        String path = vaultCfg.has("path") ? vaultCfg.get("path").getAsString() : "vault";
        int keep = vaultCfg.has("keepVersions") ? vaultCfg.get("keepVersions").getAsInt() : 2;

        JsonObject kp = vaultCfg.has("keyProvider") ? vaultCfg.getAsJsonObject("keyProvider") : new JsonObject();
        String kind = kp.has("type") ? kp.get("type").getAsString() : "file";
        KeyProvider provider = switch (kind) {
            case "file" -> {
                String keyPath = kp.has("keyPath") ? kp.get("keyPath").getAsString() : path + ".key";
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
            case "kms", "greengrass" -> {
                if (!kp.has("kmsKeyId")) {
                    throw new CredentialException("kms key provider requires keyProvider.kmsKeyId");
                }
                String keyId = kp.get("kmsKeyId").getAsString();
                String kmsRegion = kp.has("region") ? kp.get("region").getAsString() : null;
                String kmsEndpoint = kp.has("endpointUrl") ? kp.get("endpointUrl").getAsString() : null;
                yield new KmsKeyProvider(keyId, kmsRegion, kmsEndpoint);
            }
            case "pkcs11" -> {
                if (!kp.has("modulePath")) {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.modulePath");
                }
                if (!kp.has("keyLabel")) {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.keyLabel");
                }
                String modulePath = kp.get("modulePath").getAsString();
                String tokenLabel = kp.has("tokenLabel") ? kp.get("tokenLabel").getAsString() : "";
                String keyLabel = kp.get("keyLabel").getAsString();
                String pin;
                if (kp.has("pinEnv")) {
                    pin = System.getenv(kp.get("pinEnv").getAsString());
                    if (pin == null) {
                        throw new CredentialException(
                                "pkcs11 keyProvider.pinEnv '" + kp.get("pinEnv").getAsString() + "' is not set");
                    }
                } else if (kp.has("pin")) {
                    pin = kp.get("pin").getAsString();
                } else {
                    throw new CredentialException("pkcs11 key provider requires keyProvider.pinEnv or keyProvider.pin");
                }
                yield Pkcs11KeyProvider.create(modulePath, tokenLabel, keyLabel, pin);
            }
            default -> throw new CredentialException(
                    "key provider '" + kind + "' is not supported (supported: 'file', 'kms'/'greengrass', 'pkcs11')");
        };

        LocalVault vault = LocalVault.open(Paths.get(path), provider, keep);
        Object lock = new Object();

        JsonObject central = cfg.has("central") ? cfg.getAsJsonObject("central") : new JsonObject();
        String ctype = central.has("type") ? central.get("type").getAsString() : "none";
        if ("none".equals(ctype)) {
            return new DefaultCredentialService(vault, namespace, lock, null);
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
        return new DefaultCredentialService(vault, namespace, lock, sync);
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
