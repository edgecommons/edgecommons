package com.aws.proserve.ggcommons.credentials;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

import com.google.gson.JsonObject;

/**
 * Factory: build a {@link CredentialService} from the {@code credentials} config section.
 *
 * <p>Phase 1 supports the {@code file} key provider and {@code central.type: none}. Other providers
 * / sources throw a clear "phase 2" {@link CredentialException}.
 */
public final class Credentials {
    private Credentials() {}

    public static CredentialService open(JsonObject credentialsConfig) {
        JsonObject cfg = credentialsConfig != null ? credentialsConfig : new JsonObject();
        JsonObject vault = cfg.has("vault") ? cfg.getAsJsonObject("vault") : new JsonObject();
        String path = vault.has("path") ? vault.get("path").getAsString() : "vault";
        int keep = vault.has("keepVersions") ? vault.get("keepVersions").getAsInt() : 2;

        JsonObject kp = vault.has("keyProvider") ? vault.getAsJsonObject("keyProvider") : new JsonObject();
        String kind = kp.has("type") ? kp.get("type").getAsString() : "file";
        if (!"file".equals(kind)) {
            throw new CredentialException("key provider '" + kind + "' is not implemented yet (phase 1 supports 'file')");
        }

        String central = cfg.has("central") && cfg.getAsJsonObject("central").has("type")
                ? cfg.getAsJsonObject("central").get("type").getAsString() : "none";
        if (!"none".equals(central)) {
            throw new CredentialException("central source '" + central + "' is not implemented yet (phase 2)");
        }

        String keyPath = kp.has("keyPath") ? kp.get("keyPath").getAsString() : path + ".key";
        Path keyFile = Paths.get(keyPath);
        try {
            if (keyFile.getParent() != null) {
                Files.createDirectories(keyFile.getParent());
            }
        } catch (IOException e) {
            throw new CredentialException("create key dir: " + e.getMessage(), e);
        }
        KeyProvider provider = Files.exists(keyFile)
                ? FileKeyProvider.fromKeyFile(keyFile)
                : FileKeyProvider.generateKeyFile(keyFile);

        return new DefaultCredentialService(LocalVault.open(Paths.get(path), provider, keep));
    }
}
