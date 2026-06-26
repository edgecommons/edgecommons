/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.health;

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Real loopback HTTP tests for {@link HealthServer} (FR-HB-1). Binds an ephemeral port (port 0) and
 * issues actual {@code GET}s against {@code 127.0.0.1} to verify status codes, bodies, route mapping,
 * the liveness/broker decoupling, the readiness 200/503 transitions, the startup-mirrors-readiness
 * behavior, the 404 for unknown paths, and the 405 for a non-GET method.
 */
class HealthServerTest {

    private HealthServer server;
    private final HttpClient http = HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(2))
            .build();

    @AfterEach
    void tearDown() {
        if (server != null) {
            server.close();
            server = null;
        }
    }

    private HttpResponse<String> get(String path) throws Exception {
        HttpRequest req = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:" + server.getPort() + path))
                .timeout(Duration.ofSeconds(2))
                .GET()
                .build();
        return http.send(req, HttpResponse.BodyHandlers.ofString());
    }

    @Test
    void livezReturns200AndIsIndependentOfReadiness() throws Exception {
        // Readiness is permanently false (simulating a disconnected broker); liveness must still be 200.
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, () -> false);

        HttpResponse<String> resp = get("/livez");
        assertEquals(200, resp.statusCode(), "liveness must be 200 even when not ready (broker down)");
        assertEquals("ok", resp.body());
    }

    @Test
    void readyzFlipsBetween503AndWithReadinessState() throws Exception {
        AtomicBoolean ready = new AtomicBoolean(false);
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, ready::get);

        // Not ready yet (startup) -> 503.
        HttpResponse<String> notReady = get("/readyz");
        assertEquals(503, notReady.statusCode());
        assertEquals("not ready", notReady.body());

        // Becomes ready -> 200.
        ready.set(true);
        HttpResponse<String> isReady = get("/readyz");
        assertEquals(200, isReady.statusCode());
        assertEquals("ok", isReady.body());

        // Flipped back (e.g. shutdown) -> 503 again.
        ready.set(false);
        assertEquals(503, get("/readyz").statusCode());
    }

    @Test
    void startupzMirrorsReadiness() throws Exception {
        AtomicBoolean ready = new AtomicBoolean(false);
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, ready::get);

        assertEquals(503, get("/startupz").statusCode());
        ready.set(true);
        assertEquals(200, get("/startupz").statusCode());
    }

    @Test
    void unknownPathReturns404() throws Exception {
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, () -> true);

        HttpResponse<String> resp = get("/nope");
        assertEquals(404, resp.statusCode());

        // A near-miss sub-path of a known route must NOT be treated as the route (exact-match routing).
        assertEquals(404, get("/livez/extra").statusCode());
        assertEquals(404, get("/readyzz").statusCode());
    }

    @Test
    void nonGetMethodOnKnownPathReturns405() throws Exception {
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, () -> true);

        HttpRequest post = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:" + server.getPort() + "/livez"))
                .timeout(Duration.ofSeconds(2))
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();
        HttpResponse<String> resp = http.send(post, HttpResponse.BodyHandlers.ofString());
        assertEquals(405, resp.statusCode());
    }

    @Test
    void honorsCustomPathsAndReportsBoundPort() throws Exception {
        AtomicBoolean ready = new AtomicBoolean(true);
        server = new HealthServer(0, "/healthz/live", "/healthz/ready", "/healthz/start",
                () -> true, ready::get);

        assertTrue(server.getPort() > 0, "ephemeral port must be reported");
        assertEquals(200, get("/healthz/live").statusCode());
        assertEquals(200, get("/healthz/ready").statusCode());
        assertEquals(200, get("/healthz/start").statusCode());
        // The defaults are not registered when custom paths are used.
        assertEquals(404, get("/livez").statusCode());
    }

    @Test
    void closeIsIdempotent() throws Exception {
        server = new HealthServer(0, "/livez", "/readyz", "/startupz", () -> true, () -> true);
        server.close();
        server.close();  // second close must not throw
        server = null;   // prevent @AfterEach double close (already covered, but keep it tidy)
    }
}
