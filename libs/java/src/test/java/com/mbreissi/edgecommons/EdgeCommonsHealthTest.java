/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.config.HealthConfiguration;
import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

import java.net.ServerSocket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for the EdgeCommons readiness model + SIGTERM wiring + health-server enablement
 * (FR-HB-1 / FR-HB-2). A bare {@link EdgeCommons} is built via the protected no-arg constructor and the
 * (same-package, protected) {@code messagingClient}/{@code configManager} fields are injected with
 * mocks — no real broker, no full init() — so the readiness logic and shutdown path are exercised in
 * isolation. The health-server enablement matrix uses {@link EdgeCommons#resolveHealthEnabled} and a
 * real {@link EdgeCommons#startHealthServer} binding on an ephemeral port.
 */
class EdgeCommonsHealthTest {

    private final HttpClient http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(2)).build();
    private EdgeCommons gg;

    @AfterEach
    void cleanUp() {
        if (gg != null) {
            gg.shutdown();
            gg = null;
        }
    }

    /** Builds a bare EdgeCommons with injected mock messaging + config (no init()). */
    private EdgeCommons bare(MockMessagingService msg, MockConfigurationService cfg) {
        EdgeCommons g = new EdgeCommons();
        g.messagingClient = msg;
        g.configManager = cfg;
        return g;
    }

    private static int freePort() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            return s.getLocalPort();
        }
    }

    private int statusOf(int port, String path) throws Exception {
        HttpRequest req = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:" + port + path))
                .timeout(Duration.ofSeconds(2)).GET().build();
        return http.send(req, HttpResponse.BodyHandlers.ofString()).statusCode();
    }

    // ---- readiness model ----

    @Test
    void livenessIsAlwaysTrue() {
        gg = bare(new MockMessagingService(), new MockConfigurationService());
        assertTrue(gg.isLive(), "process liveness is always true");
    }

    @Test
    void readyzIsTrueOnlyWhenConnectedAndReadyAndNotShuttingDown() {
        MockMessagingService msg = new MockMessagingService();  // connected = true by default
        gg = bare(msg, new MockConfigurationService());

        // connected && readyFlag(default true) && !shuttingDown -> ready.
        assertTrue(gg.isReadyz());

        // App gates readiness off -> not ready (even though connected).
        gg.setReady(false);
        assertFalse(gg.isReadyz());
        gg.setReady(true);
        assertTrue(gg.isReadyz());

        // Disconnected messaging -> not ready (regardless of readyFlag).
        msg.setConnected(false);
        assertFalse(gg.isReadyz());
        msg.setConnected(true);
        assertTrue(gg.isReadyz());
    }

    @Test
    void readyzIsFalseWhenNoMessagingWired() {
        EdgeCommons g = new EdgeCommons();  // no messagingClient
        g.configManager = new MockConfigurationService();
        assertFalse(g.messagingConnected());
        assertFalse(g.isReadyz());
    }

    @Test
    void readyzRequiresAcknowledgedActiveCommandInbox() {
        MockMessagingService msg = new MockMessagingService();
        MockConfigurationService cfg = new MockConfigurationService();
        gg = bare(msg, cfg);
        gg.commandInbox = new CommandInbox(cfg, msg, () -> 0L, () -> true, JsonObject::new);

        assertFalse(gg.isReadyz(), "STOPPED command inbox must gate readiness");
        gg.commandInbox.start(Duration.ofSeconds(1));
        assertTrue(gg.isReadyz(), "acknowledged ACTIVE command inbox releases the runtime gate");
        gg.commandInbox.stop();
        assertFalse(gg.isReadyz(), "stopping the inbox must immediately fail readiness");
    }

    @Test
    void livenessStays200WhenMessagingDisconnected() throws Exception {
        // A broker outage must NOT fail liveness (restart-storm guard).
        MockMessagingService msg = new MockMessagingService();
        msg.setConnected(false);
        MockConfigurationService cfg = new MockConfigurationService();
        int port = freePort();
        cfg.setHealthConfig(new HealthConfiguration(
                JsonParser.parseString("{ \"port\": " + port + " }").getAsJsonObject()));
        gg = bare(msg, cfg);
        gg.startHealthServer(Platform.KUBERNETES);

        assertEquals(200, statusOf(port, "/livez"), "livez 200 even when disconnected");
        assertEquals(503, statusOf(port, "/readyz"), "readyz 503 when disconnected");
        assertEquals(503, statusOf(port, "/startupz"), "startupz mirrors readiness");
    }

    // ---- SIGTERM / shutdown ----

    @Test
    void shutdownSignalFlipsReadinessTo503AndClosesMessagingIdempotently() throws Exception {
        MockMessagingService msg = new MockMessagingService();
        MockConfigurationService cfg = new MockConfigurationService();
        int port = freePort();
        cfg.setHealthConfig(new HealthConfiguration(
                JsonParser.parseString("{ \"port\": " + port + " }").getAsJsonObject()));
        EdgeCommons g = bare(msg, cfg);
        g.startHealthServer(Platform.KUBERNETES);

        // Ready before the signal.
        assertEquals(200, statusOf(port, "/readyz"));

        // Simulate SIGTERM: flips readiness to 503 and runs the (idempotent) close chain.
        g.onShutdownSignal();
        assertFalse(g.isReadyz(), "shutdown must flip readiness to not-ready");
        assertEquals(1, msg.getCloseCount(), "messaging (which unsubscribes all) must be closed once");

        // A second signal / an app-driven shutdown must be a no-op (idempotent).
        g.onShutdownSignal();
        g.shutdown();
        assertEquals(1, msg.getCloseCount(), "shutdown must be idempotent");
    }

    @Test
    void shutdownIsIdempotentWhenCalledByAppThenHook() {
        MockMessagingService msg = new MockMessagingService();
        EdgeCommons g = bare(msg, new MockConfigurationService());
        g.shutdown();              // app-driven
        g.onShutdownSignal();      // hook fires later
        assertEquals(1, msg.getCloseCount());
    }

    // ---- health-server enablement precedence (FR-HB-1 / FR-RT-3) ----

    @Test
    void resolveHealthEnabledPrecedence() {
        HealthConfiguration unset = new HealthConfiguration(null);
        HealthConfiguration on = new HealthConfiguration(
                JsonParser.parseString("{ \"enabled\": true }").getAsJsonObject());
        HealthConfiguration off = new HealthConfiguration(
                JsonParser.parseString("{ \"enabled\": false }").getAsJsonObject());

        // Explicit config wins in both directions, on any platform.
        assertTrue(EdgeCommons.resolveHealthEnabled(on, Platform.HOST));
        assertTrue(EdgeCommons.resolveHealthEnabled(on, Platform.GREENGRASS));
        assertFalse(EdgeCommons.resolveHealthEnabled(off, Platform.KUBERNETES));

        // Unset -> platform-profile default: on for KUBERNETES, off elsewhere (incl. null).
        assertTrue(EdgeCommons.resolveHealthEnabled(unset, Platform.KUBERNETES));
        assertFalse(EdgeCommons.resolveHealthEnabled(unset, Platform.HOST));
        assertFalse(EdgeCommons.resolveHealthEnabled(unset, Platform.GREENGRASS));
        assertFalse(EdgeCommons.resolveHealthEnabled(unset, null));
    }

    @Test
    void healthServerIsOffByDefaultOnHostAndGreengrass() throws Exception {
        MockConfigurationService cfg = new MockConfigurationService();
        cfg.setHealthConfig(new HealthConfiguration(
                JsonParser.parseString("{ \"port\": " + freePort() + " }").getAsJsonObject()));
        gg = bare(new MockMessagingService(), cfg);

        gg.startHealthServer(Platform.HOST);
        assertNull(gg.healthServer, "health server must be OFF by default on HOST");

        gg.startHealthServer(Platform.GREENGRASS);
        assertNull(gg.healthServer, "health server must be OFF by default on GREENGRASS");
    }

    @Test
    void healthServerIsOnByDefaultOnKubernetes() throws Exception {
        MockConfigurationService cfg = new MockConfigurationService();
        int port = freePort();
        cfg.setHealthConfig(new HealthConfiguration(
                JsonParser.parseString("{ \"port\": " + port + " }").getAsJsonObject()));
        gg = bare(new MockMessagingService(), cfg);

        gg.startHealthServer(Platform.KUBERNETES);
        assertNotNull(gg.healthServer, "health server must be ON by default on KUBERNETES");
        assertEquals(200, statusOf(port, "/readyz"));
    }

    @Test
    void explicitEnabledTrueStartsServerOnHost() throws Exception {
        MockConfigurationService cfg = new MockConfigurationService();
        int port = freePort();
        cfg.setHealthConfig(new HealthConfiguration(JsonParser.parseString(
                "{ \"enabled\": true, \"port\": " + port + " }").getAsJsonObject()));
        gg = bare(new MockMessagingService(), cfg);

        gg.startHealthServer(Platform.HOST);
        assertNotNull(gg.healthServer, "explicit health.enabled=true must start the server on HOST");
        assertEquals(200, statusOf(port, "/livez"));
    }

    @Test
    void explicitEnabledFalseDisablesServerOnKubernetes() {
        MockConfigurationService cfg = new MockConfigurationService();
        cfg.setHealthConfig(new HealthConfiguration(JsonParser.parseString(
                "{ \"enabled\": false }").getAsJsonObject()));
        gg = bare(new MockMessagingService(), cfg);

        gg.startHealthServer(Platform.KUBERNETES);
        assertNull(gg.healthServer, "explicit health.enabled=false must disable the server on KUBERNETES");
    }
}
