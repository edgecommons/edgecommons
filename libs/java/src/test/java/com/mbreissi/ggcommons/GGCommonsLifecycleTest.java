/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

/**
 * Lifecycle tests for the root {@link GGCommons} facade — the opt-in subsystem accessors
 * ({@code getCredentials}/{@code getParameters}/{@code getStreams}) and orderly {@link
 * GGCommons#shutdown()} — brought up in STANDALONE mode against an in-process Moquette broker.
 *
 * <p>Two configurations are exercised:
 * <ul>
 *   <li><b>bare</b>: no {@code credentials}/{@code parameters}/{@code streaming} sections, so every
 *       opt-in accessor must return {@code null} (the absent-section contract);</li>
 *   <li><b>with credentials + parameters</b>: both sections present and local-only (a {@code file}
 *       key-provider vault under a temp dir; an {@code env} parameter source with an in-memory
 *       cache), so {@code initCredentials}/{@code initParameters} and their metric bridges run, the
 *       accessors return live services, and shutdown closes them in order.</li>
 * </ul>
 *
 * <p>Streaming is intentionally not configured: it loads the native {@code ggstreamlog} cdylib,
 * which is out of scope for these unit tests; the {@code getStreams() == null} assertion covers the
 * absent-section branch.
 */
class GGCommonsLifecycleTest {

    private static Server broker;
    private static int port;
    private static Path tmp;
    private GGCommons gg;

    @BeforeAll
    static void startBroker() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        Properties props = new Properties();
        props.setProperty("host", "127.0.0.1");
        props.setProperty("port", String.valueOf(port));
        props.setProperty("allow_anonymous", "true");
        props.setProperty("persistence_enabled", "false");
        props.setProperty("data_path", Files.createTempDirectory("moquette-life").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));
        tmp = Files.createTempDirectory("gglife");
    }

    @AfterAll
    static void stopBroker() {
        if (broker != null) {
            broker.stopServer();
        }
    }

    @AfterEach
    void shutdownGg() {
        if (gg != null) {
            gg.shutdown();
            gg = null;
        }
    }

    private File writeMessagingConfig(String name) throws Exception {
        File f = new File(tmp.toFile(), name);
        Files.write(f.toPath(), ("""
                { "messaging": { "local": { "host": "127.0.0.1", "port": %d, "clientId": "%s" } } }""")
                .formatted(port, name).getBytes(StandardCharsets.UTF_8));
        return f;
    }

    private GGCommons bringUp(String component, String thing, File appCfg, File msgCfg) {
        return bringUp(component, thing, appCfg, msgCfg, "HOST");
    }

    private GGCommons bringUp(String component, String thing, File appCfg, File msgCfg, String platform) {
        String[] args = {
                "-t", thing,
                "--platform", platform, "--transport", "MQTT", msgCfg.getAbsolutePath(),
                "-c", "FILE", appCfg.getAbsolutePath()
        };
        return GGCommonsBuilder.create(component).withArgs(args).build();
    }

    @Test
    void optInAccessorsReturnNullWhenSectionsAbsent() throws Exception {
        File appCfg = new File(tmp.toFile(), "bare-config.json");
        Files.write(appCfg.toPath(), """
                { "logging": {"level": "INFO"}, "component": {"global": {}} }"""
                .getBytes(StandardCharsets.UTF_8));
        File msgCfg = writeMessagingConfig("bare-messaging.json");

        gg = bringUp("com.test.BareComponent", "bare-thing", appCfg, msgCfg);

        // Core subsystems are always present.
        assertNotNull(gg.getConfigManager());
        assertNotNull(gg.getMessaging());
        assertNotNull(gg.getMetrics());
        // Opt-in subsystems with no matching config section -> null.
        assertNull(gg.getCredentials(), "credentials must be null when no 'credentials' section");
        assertNull(gg.getParameters(), "parameters must be null when no 'parameters' section");
        assertNull(gg.getStreams(), "streams must be null when no 'streaming' section");
    }

    @Test
    void credentialsAndParametersComeUpAndShutDownCleanly() throws Exception {
        // Local-only vault path + cache path under the temp dir; forward slashes so JSON is valid
        // on Windows too.
        String vaultPath = new File(tmp.toFile(), "vault").getAbsolutePath().replace("\\", "/");

        File appCfg = new File(tmp.toFile(), "creds-config.json");
        Files.write(appCfg.toPath(), ("""
                {
                  "logging": {"level": "INFO"},
                  "credentials": { "vault": { "path": "%s", "keyProvider": { "type": "file" } } },
                  "parameters": { "source": { "type": "env" }, "bootstrapOnStart": false },
                  "component": {"global": {}}
                }""").formatted(vaultPath).getBytes(StandardCharsets.UTF_8));
        File msgCfg = writeMessagingConfig("creds-messaging.json");

        gg = bringUp("com.test.CredsComponent", "creds-thing", appCfg, msgCfg);

        // Both opt-in services are now live.
        assertNotNull(gg.getCredentials(), "credentials service should be initialized");
        assertNotNull(gg.getParameters(), "parameter service should be initialized");
        assertNull(gg.getStreams(), "streaming not configured -> null");

        // The credential service round-trips a secret through the file-backed vault.
        gg.getCredentials().put("unit/test", "value".getBytes(StandardCharsets.UTF_8));
        assertEquals("value", gg.getCredentials().getString("unit/test").orElseThrow());

        // shutdown() closes the credential/parameter bridges + services in order without error.
        GGCommons toClose = gg;
        gg = null; // prevent double-close in @AfterEach
        assertDoesNotThrow(toClose::shutdown);
    }

    @Test
    void kubernetesProfileDefaultDoesNotAutoEnableCredentials() throws Exception {
        // FR-CRED-6 guard: the KUBERNETES profile defaults the key-provider to 'env', but that is a
        // default for an *already-configured* vault — with no 'credentials' section, credentials must
        // stay OFF even on KUBERNETES (the opt-in contract is preserved).
        // Disable the KUBERNETES prometheus/health HTTP servers (explicit config wins) so this
        // credentials-focused test binds no ports; the credentials key-provider default is unaffected.
        File appCfg = new File(tmp.toFile(), "k8s-bare-config.json");
        Files.write(appCfg.toPath(), """
                { "logging": {"level": "INFO"}, "metricEmission": {"target": "log"},
                  "health": {"enabled": false}, "component": {"global": {}} }"""
                .getBytes(StandardCharsets.UTF_8));
        File msgCfg = writeMessagingConfig("k8s-bare-messaging.json");

        gg = bringUp("com.test.K8sBareComponent", "k8s-bare-thing", appCfg, msgCfg, "KUBERNETES");

        assertNull(gg.getCredentials(),
                "no 'credentials' section -> credentials OFF even with the KUBERNETES env default");
    }

    @Test
    void kubernetesDefaultsCredentialsKeyProviderToEnvWhenTypeAbsent() throws Exception {
        // FR-CRED-6 (precedence FR-RT-3): a 'credentials' section present but with NO keyProvider.type
        // resolves to the KUBERNETES profile default 'env'. envVar points at the surefire-injected
        // GGCOMMONS_TEST_VAULT_KEK (base64 of the 0x00..0x1f KEK).
        String vaultPath = new File(tmp.toFile(), "k8s-vault").getAbsolutePath().replace("\\", "/");

        File appCfg = new File(tmp.toFile(), "k8s-creds-config.json");
        Files.write(appCfg.toPath(), ("""
                {
                  "logging": {"level": "INFO"},
                  "metricEmission": {"target": "log"}, "health": {"enabled": false},
                  "credentials": { "vault": { "path": "%s",
                      "keyProvider": { "envVar": "GGCOMMONS_TEST_VAULT_KEK" } } },
                  "component": {"global": {}}
                }""").formatted(vaultPath).getBytes(StandardCharsets.UTF_8));
        File msgCfg = writeMessagingConfig("k8s-creds-messaging.json");

        gg = bringUp("com.test.K8sCredsComponent", "k8s-creds-thing", appCfg, msgCfg, "KUBERNETES");

        assertNotNull(gg.getCredentials(), "credentials section present -> service initialized");
        gg.getCredentials().put("unit/test", "value".getBytes(StandardCharsets.UTF_8));
        assertEquals("value", gg.getCredentials().getString("unit/test").orElseThrow());

        // The on-disk KEK record is tagged provider=env -> the env custodian (not file) was selected
        // by the platform default.
        String raw = Files.readString(Path.of(vaultPath));
        String provider = com.google.gson.JsonParser.parseString(raw).getAsJsonObject()
                .getAsJsonObject("kek").get("provider").getAsString();
        assertEquals("env", provider);
    }

    @Test
    void shutdownIsIdempotentlySafeOnMinimalBringUp() throws Exception {
        File appCfg = new File(tmp.toFile(), "min-config.json");
        Files.write(appCfg.toPath(), """
                { "logging": {"level": "INFO"}, "component": {"global": {}} }"""
                .getBytes(StandardCharsets.UTF_8));
        File msgCfg = writeMessagingConfig("min-messaging.json");

        GGCommons local = bringUp("com.test.MinComponent", "min-thing", appCfg, msgCfg);
        assertDoesNotThrow(local::shutdown);
    }
}
