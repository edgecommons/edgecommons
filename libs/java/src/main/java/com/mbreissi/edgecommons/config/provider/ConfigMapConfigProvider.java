/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config.provider;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.parameters.MountedDirSource;
import com.mbreissi.edgecommons.platform.PlatformResolver;
import com.mbreissi.edgecommons.utils.DirectoryWatcher;
import com.mbreissi.edgecommons.utils.FileWatcher;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * The Kubernetes-native config source: reads the component configuration from a mounted <b>ConfigMap
 * directory</b> and hot-reloads it across the kubelet's atomic {@code ..data} symlink swap
 * (DESIGN-subsystems §1, FR-CFG-1..5). It is the default config source on the {@code KUBERNETES}
 * platform and the canonical analogue of {@link FileConfigProvider} — it reuses the same
 * {@link ConfigManager#applyConfig} reload seam, but watches the mount <em>directory</em> via
 * {@link DirectoryWatcher} instead of the file inode.
 *
 * <p>Selected via {@code -c CONFIGMAP [mountDir] [key]}; defaults are mount dir {@value #DEFAULT_MOUNT_DIR}
 * and key {@value #DEFAULT_KEY} (so a pod with a ConfigMap mounted at {@code /etc/edgecommons} loads
 * {@code config.json} with no {@code -c} flag).
 *
 * <h2>Why not {@link FileConfigProvider}?</h2>
 * A mounted ConfigMap is a directory of symlinks the kubelet swaps atomically. Watching the user-visible
 * {@code config.json} fires once and dies after the swap ({@code IN_DELETE_SELF}); worse, the swap shows
 * up as events on the {@code ..data} entry, not on {@code config.json}. {@link DirectoryWatcher} solves
 * both: it watches the persistent mount directory, reacts to <em>any</em> entry event, and re-arms if the
 * watch is invalidated (FR-CFG-2).
 *
 * <h2>Content dedupe</h2>
 * A filesystem event is not a configuration change. The kubelet touches projected volumes on its sync
 * loop, so an unchanged mount produces a steady stream of entry events; the {@code ..data} swap itself
 * produces a burst of them. Every event therefore re-reads the key, but the reload seam is entered
 * <em>only when the re-read document differs from the last one handed to the {@link ConfigManager}</em>
 * — compared as parsed JSON, so a whitespace-only or key-reordered rewrite is not a change. The
 * baseline is seeded by the initial {@link #loadConfiguration()}, so the first event after startup on
 * an unchanged mount reloads nothing. This also collapses a swap burst into a single reload, which is
 * why no debounce timer is needed.
 *
 * <h2>Reject-and-keep</h2>
 * On a reload, a malformed file (mid-swap read, or a bad ConfigMap edit) must never crash a running pod
 * (FR-CFG-5). A parse failure is logged and the previous config is kept; a parseable-but-schema-invalid
 * document is rejected-and-kept by {@link ConfigManager#applyConfig} itself. The <em>initial</em> load
 * still fails loudly, exactly like {@link FileConfigProvider}.
 *
 * <h2>The {@code subPath} caveat (FR-CFG-3)</h2>
 * A ConfigMap mounted with {@code subPath} is <b>never</b> updated by the kubelet — there is no
 * {@code ..data} symlink farm and hot-reload is silently dead. This provider warns when it detects a
 * mount with no {@code ..data} entry. Mount the whole volume, not a {@code subPath}; for a forced
 * {@code subPath}/immutable/env mount use a restart-on-change controller (e.g. Stakater Reloader).
 *
 * <p>Kubelet projection artifacts ({@code ..data}, {@code ..2026_...} timestamped dirs) are never parsed
 * as config: the configured key is rejected if it is itself such an artifact, reusing the dotfile filter
 * in {@link MountedDirSource#isProjectionArtifact} (FR-CFG-4).
 */
final class ConfigMapConfigProvider extends ConfigProvider implements FileWatcher.FileChangeHandler {

    private static final Logger LOGGER = LogManager.getLogger(ConfigMapConfigProvider.class);

    /**
     * Default ConfigMap mount directory when {@code -c CONFIGMAP} is given no path argument. Single
     * source of truth lives in {@link PlatformResolver}, which reuses it to default the MQTT
     * messaging-config path under CONFIGMAP+MQTT (FR-MSG-1) — the two must stay identical.
     */
    static final String DEFAULT_MOUNT_DIR = PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR;
    /** Default config key (file name within the mount) when none is given. */
    static final String DEFAULT_KEY = PlatformResolver.CONFIGMAP_DEFAULT_KEY;
    /** The kubelet's atomic-swap symlink; its presence indicates a whole-volume (reloadable) mount. */
    static final String KUBELET_DATA_LINK = "..data";

    private final Path mountDir;
    private final String key;
    private final Path configFile;
    private final DirectoryWatcher watcher;
    private boolean started;
    /**
     * The last document handed to the {@link ConfigManager} — seeded by {@link #loadConfiguration()}
     * and updated on each reload, so a re-read that parses to the same document is not re-applied
     * (see the class docs). {@code volatile}: written by the watcher thread, read by both.
     */
    private volatile JsonObject lastEmitted;

    /**
     * Creates a ConfigMap config provider.
     *
     * @param configManager the parent config manager whose {@code applyConfig} is the reload seam
     * @param mountDir      the ConfigMap mount directory, or {@code null} for {@value #DEFAULT_MOUNT_DIR}
     * @param key           the config file name within the mount, or {@code null} for {@value #DEFAULT_KEY}
     * @throws IllegalArgumentException if {@code key} is a kubelet projection artifact (a {@code ..}-entry)
     */
    ConfigMapConfigProvider(ConfigManager configManager, String mountDir, String key) {
        super(configManager);
        this.mountDir = Paths.get(mountDir != null ? mountDir : DEFAULT_MOUNT_DIR);
        this.key = key != null ? key : DEFAULT_KEY;
        if (MountedDirSource.isProjectionArtifact(this.key)) {
            throw new IllegalArgumentException(
                    "ConfigMap key must not be a kubelet projection artifact (a '..'/'.' entry): " + this.key);
        }
        this.configFile = this.mountDir.resolve(this.key);
        warnIfSubPathMount();
        this.watcher = new DirectoryWatcher(this.mountDir, this);
        this.watcher.setDaemon(true);
    }

    @Override
    public synchronized void start() {
        if (!started) {
            this.watcher.start();
            started = true;
        }
    }

    /**
     * Warns when the mount appears to be a {@code subPath} (or otherwise non-projected) mount that will
     * never hot-reload — detected by the absence of the kubelet {@code ..data} symlink (FR-CFG-3).
     */
    private void warnIfSubPathMount() {
        if (!Files.exists(this.mountDir.resolve(KUBELET_DATA_LINK))) {
            LOGGER.warn("ConfigMap mount '{}' has no '{}' symlink — this looks like a subPath/immutable "
                            + "mount, which the kubelet never updates, so hot-reload is disabled. Mount the "
                            + "whole volume (not a subPath), or use a restart-on-change controller.",
                    this.mountDir, KUBELET_DATA_LINK);
        }
    }

    @Override
    public JsonObject loadConfiguration() {
        LOGGER.debug("Loading configuration from ConfigMap '{}'", configFile);
        try {
            byte[] bytes = Files.readAllBytes(configFile);
            JsonObject loaded = gson.fromJson(new String(bytes, StandardCharsets.UTF_8), JsonObject.class);
            // Seed the dedupe baseline, so the first kubelet touch after startup does not re-apply the
            // document the component already holds.
            this.lastEmitted = loaded;
            return loaded;
        } catch (JsonSyntaxException | IOException e) {
            LOGGER.fatal("Error reading ConfigMap configuration '{}': {}", configFile, e.toString());
            throw new RuntimeException("Error reading ConfigMap configuration '" + configFile + "': " + e, e);
        }
    }

    @Override
    public String getConfigSource() {
        return String.format("ConfigMap (mountDir: %s, key: %s)", mountDir, key);
    }

    @Override
    public void close() {
        if (started) {
            watcher.stopThread();
        }
    }

    /**
     * Reload callback: re-read the ConfigMap key and, <em>if its content differs from the last document
     * handed to the {@link ConfigManager}</em>, apply it. An entry event that leaves the content
     * unchanged (the kubelet's projected-volume churn, or a later event of the same {@code ..data} swap
     * burst) is swallowed. Reject-and-keep on a transient/malformed read (a mid-swap window or a bad
     * edit) so a running pod never crashes on reload (FR-CFG-5): the read is delegated to the config
     * manager, which logs it and keeps the previous configuration.
     */
    @Override
    public void onChange() {
        JsonObject candidate = readCandidate();
        if (candidate != null && candidate.equals(lastEmitted)) {
            LOGGER.debug("ConfigMap entry event with unchanged content; no reload applied");
            return;
        }
        LOGGER.info("ConfigMap changed: applying new config from {}", configFile);
        if (candidate != null) {
            this.lastEmitted = candidate;
        }
        parentConfigManager.reloadFromProvider();
    }

    /**
     * Re-reads and parses the ConfigMap key for the dedupe comparison only.
     *
     * @return the parsed document, or {@code null} if it could not be read or parsed (a mid-swap
     *         window, an empty file, or a bad edit) — the caller then delegates to the config manager,
     *         which applies the reject-and-keep path (FR-CFG-5)
     */
    private JsonObject readCandidate() {
        try {
            byte[] bytes = Files.readAllBytes(configFile);
            return gson.fromJson(new String(bytes, StandardCharsets.UTF_8), JsonObject.class);
        } catch (JsonSyntaxException | IOException e) {
            LOGGER.debug("ConfigMap re-read for change detection failed ({}); delegating to reload",
                    e.toString());
            return null;
        }
    }
}
