/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config.provider;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.parameters.MountedDirSource;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.ArgumentCaptor;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

import static org.junit.jupiter.api.Assertions.*;
import static org.junit.jupiter.api.Assumptions.assumeTrue;
import static org.mockito.Mockito.*;

/**
 * Unit tests for {@link ConfigMapConfigProvider} (the {@code -c CONFIGMAP} k8s-native source):
 * config load from a mounted-style temp directory, the kubelet dotfile filter (FR-CFG-4),
 * reject-and-keep on an invalid reload (FR-CFG-5), the subPath warning detection (FR-CFG-3), and the
 * directory-watch RE-ARM verified by simulating the kubelet atomic {@code ..data} swap (FR-CFG-2).
 *
 * <p>Every test {@code close()}s the provider so its {@link com.breissinger.ggcommons.utils.DirectoryWatcher}
 * daemon thread does not leak. The {@link ConfigManager} is a Mockito mock so {@code applyConfig(...)}
 * can be observed without exercising the real schema validator or logging reconfiguration.
 */
class ConfigMapConfigProviderTest {

    private static void write(Path file, String json) throws IOException {
        Files.write(file, json.getBytes(StandardCharsets.UTF_8));
    }

    private static String configJson(int version) {
        return "{\"component\":{\"name\":\"x\"},\"version\":" + version + "}";
    }

    // ---------- load ----------

    @Test
    void loadsConfigFromMountedDirectory(@TempDir Path mount) throws IOException {
        ConfigManager cm = mock(ConfigManager.class);
        write(mount.resolve("config.json"), configJson(7));

        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            JsonObject loaded = provider.loadConfiguration();
            assertNotNull(loaded);
            assertEquals(7, loaded.get("version").getAsInt());
            assertTrue(provider.getConfigSource().contains(mount.toString()));
            assertTrue(provider.getConfigSource().contains("config.json"));
        } finally {
            provider.close();
        }
    }

    @Test
    void loadThrowsForMissingKeyOnInitialLoad(@TempDir Path mount) {
        // The initial load must fail loudly (parity with FILE), unlike a reload (reject-and-keep).
        ConfigManager cm = mock(ConfigManager.class);
        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            RuntimeException ex = assertThrows(RuntimeException.class, provider::loadConfiguration);
            assertTrue(ex.getMessage().contains("config.json"));
        } finally {
            provider.close();
        }
    }

    @Test
    void appliesDefaultMountDirAndKeyWhenNullArgs() {
        // Defaults: /etc/ggcommons + config.json (no NPE; the dir need not exist to construct).
        ConfigManager cm = mock(ConfigManager.class);
        ConfigMapConfigProvider provider = new ConfigMapConfigProvider(cm, null, null);
        try {
            // The default mount dir basename survives OS path-separator differences ("/etc/ggcommons"
            // renders with backslashes on Windows), so assert on the basename + key.
            assertTrue(provider.getConfigSource().contains("ggcommons"));
            assertTrue(provider.getConfigSource().contains(ConfigMapConfigProvider.DEFAULT_KEY));
        } finally {
            provider.close();
        }
    }

    // ---------- dotfile filter (FR-CFG-4) ----------

    @Test
    void dotfileFilterIdentifiesProjectionArtifacts() {
        // The shared filter reused from MountedDirSource skips the kubelet symlink farm.
        assertTrue(MountedDirSource.isProjectionArtifact("..data"));
        assertTrue(MountedDirSource.isProjectionArtifact("..2026_06_25_12_00_00.123456789"));
        assertTrue(MountedDirSource.isProjectionArtifact("..data_tmp"));
        assertFalse(MountedDirSource.isProjectionArtifact("config.json"));
    }

    @Test
    void rejectsKeyThatIsAProjectionArtifact(@TempDir Path mount) {
        // A projection-artifact key (..data) must never be read as config.
        ConfigManager cm = mock(ConfigManager.class);
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigMapConfigProvider(cm, mount.toString(), "..data"));
    }

    // ---------- subPath warning (FR-CFG-3) ----------

    @Test
    void constructsWhenSubPathMountHasNoDataLink(@TempDir Path mount) throws IOException {
        // No '..data' symlink -> looks like a subPath mount; provider warns but still constructs+loads.
        ConfigManager cm = mock(ConfigManager.class);
        write(mount.resolve("config.json"), configJson(1));
        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            assertEquals(1, provider.loadConfiguration().get("version").getAsInt());
        } finally {
            provider.close();
        }
    }

    // ---------- reject-and-keep on reload (FR-CFG-5) ----------

    @Test
    void onChangeAppliesValidReload(@TempDir Path mount) throws IOException {
        ConfigManager cm = mock(ConfigManager.class);
        write(mount.resolve("config.json"), configJson(3));

        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            provider.onChange();
            ArgumentCaptor<JsonObject> captor = ArgumentCaptor.forClass(JsonObject.class);
            verify(cm).applyConfig(captor.capture());
            assertEquals(3, captor.getValue().get("version").getAsInt());
        } finally {
            provider.close();
        }
    }

    @Test
    void onChangeKeepsPreviousOnMalformedJson(@TempDir Path mount) throws IOException {
        // A malformed reload (e.g. a bad ConfigMap edit) must not crash the pod: keep previous,
        // never call applyConfig with garbage.
        ConfigManager cm = mock(ConfigManager.class);
        write(mount.resolve("config.json"), "{ this is : not valid json ]");

        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            assertDoesNotThrow(provider::onChange);
            verify(cm, never()).applyConfig(any());
        } finally {
            provider.close();
        }
    }

    @Test
    void onChangeKeepsPreviousWhenFileVanishesMidSwap(@TempDir Path mount) {
        // Reading the key during a swap window (file briefly absent) must not crash: keep previous.
        ConfigManager cm = mock(ConfigManager.class);
        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            assertDoesNotThrow(provider::onChange);
            verify(cm, never()).applyConfig(any());
        } finally {
            provider.close();
        }
    }

    @Test
    void onChangeKeepsPreviousOnEmptyFile(@TempDir Path mount) throws IOException {
        // An empty file parses to null JSON -> keep previous (no applyConfig(null)).
        ConfigManager cm = mock(ConfigManager.class);
        write(mount.resolve("config.json"), "");
        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            assertDoesNotThrow(provider::onChange);
            verify(cm, never()).applyConfig(any());
        } finally {
            provider.close();
        }
    }

    // ---------- directory-watch re-arm across swaps (FR-CFG-2) ----------

    @Test
    void directoryWatchReloadsRepeatedlyAcrossEdits(@TempDir Path mount) throws Exception {
        // The directory watch must keep firing across successive ConfigMap edits — i.e. it re-arms and
        // is not a one-shot watch. Runs on every OS (in-place writes; the faithful symlink-swap inode
        // replacement is exercised by the Linux-only test below).
        ConfigManager cm = mock(ConfigManager.class);
        Path key = mount.resolve("config.json");
        write(key, configJson(1));
        // Presence of '..data' makes the mount look whole-volume (no subPath warning path).
        Files.createDirectory(mount.resolve("..data"));

        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            Thread.sleep(2_000); // let the directory watch arm before mutating
            write(key, configJson(2));
            Thread.sleep(1_500);
            write(key, configJson(3));

            ArgumentCaptor<JsonObject> captor = ArgumentCaptor.forClass(JsonObject.class);
            // At least two reloads (the watch survived the first edit and re-fired on the second);
            // the final applied config carries version 3.
            verify(cm, timeout(20_000).atLeast(2)).applyConfig(captor.capture());
            assertEquals(3, captor.getAllValues().get(captor.getAllValues().size() - 1)
                    .get("version").getAsInt());
        } finally {
            provider.close();
        }
    }

    @Test
    void hotReloadSurvivesKubeletDataSymlinkSwap(@TempDir Path mount) throws Exception {
        // The faithful kubelet shape: config.json -> ..data/config.json, and ..data is a symlink the
        // kubelet swaps atomically. Requires symlink support (skipped on Windows without privilege).
        ConfigManager cm = mock(ConfigManager.class);

        Path firstData = mount.resolve("..2026_a");
        Files.createDirectory(firstData);
        write(firstData.resolve("config.json"), configJson(1));
        boolean symlinksWork;
        try {
            Files.createSymbolicLink(mount.resolve("..data"), Path.of("..2026_a"));
            Files.createSymbolicLink(mount.resolve("config.json"), Path.of("..data/config.json"));
            symlinksWork = true;
        } catch (IOException | UnsupportedOperationException e) {
            symlinksWork = false;
        }
        assumeTrue(symlinksWork, "symlinks not supported on this host; kubelet swap simulation skipped");

        ConfigMapConfigProvider provider =
                new ConfigMapConfigProvider(cm, mount.toString(), "config.json");
        try {
            assertEquals(1, provider.loadConfiguration().get("version").getAsInt());
            Thread.sleep(2_000); // let the directory watch arm before the swap

            // Kubelet swap: new timestamped dir, stage ..data_tmp -> it, atomic rename onto ..data.
            Path secondData = mount.resolve("..2026_b");
            Files.createDirectory(secondData);
            write(secondData.resolve("config.json"), configJson(2));
            Files.createSymbolicLink(mount.resolve("..data_tmp"), Path.of("..2026_b"));
            Files.move(mount.resolve("..data_tmp"), mount.resolve("..data"),
                    StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);

            ArgumentCaptor<JsonObject> captor = ArgumentCaptor.forClass(JsonObject.class);
            verify(cm, timeout(15_000).atLeastOnce()).applyConfig(captor.capture());
            assertEquals(2, captor.getValue().get("version").getAsInt());
        } finally {
            provider.close();
        }
    }
}
