/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.health;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.Executors;
import java.util.function.BooleanSupplier;

/**
 * A minimal, dependency-free HTTP/1.1 health endpoint for Kubernetes probes (FR-HB-1), built on the
 * JDK's built-in {@link com.sun.net.httpserver.HttpServer} (no web framework, no new dependency, so
 * the shaded self-contained JAR is unaffected). Binds {@code 0.0.0.0} on the configured port and
 * serves three {@code GET} routes:
 *
 * <ul>
 *   <li><b>{@code GET <livenessPath>}</b> (default {@code /livez}) &rarr; {@code 200 "ok"} <em>while
 *       the process is alive</em>. The handler executing <em>is</em> the liveness proof; it
 *       <b>never</b> checks the broker or any external dependency, so a broker outage cannot fail
 *       liveness (which would cause kubelet restart storms). Backed by the supplied {@code liveness}
 *       supplier, which the library wires to a constant {@code true}.</li>
 *   <li><b>{@code GET <readinessPath>}</b> (default {@code /readyz}) &rarr; {@code 200 "ok"} only when
 *       the {@code readiness} supplier returns {@code true} (messaging connected &amp;&amp; the
 *       component is ready &amp;&amp; not shutting down); otherwise {@code 503 "not ready"}.</li>
 *   <li><b>{@code GET <startupPath>}</b> (default {@code /startupz}) &rarr; reuses the readiness
 *       semantics (200 when ready, else 503).</li>
 * </ul>
 *
 * <p>Any other path &rarr; {@code 404}. A non-{@code GET} method on a known path &rarr; {@code 405}.
 * Responses are tiny {@code text/plain} bodies. The server runs on its own daemon dispatcher thread
 * plus a single-thread daemon handler executor, so it never blocks JVM exit; {@link #close()} stops
 * it. The same routes/semantics are mirrored in the Python/Rust/TS libraries for parity.
 */
public final class HealthServer
{
    private static final Logger LOGGER = LogManager.getLogger(HealthServer.class);

    private static final byte[] OK = "ok".getBytes(StandardCharsets.UTF_8);
    private static final byte[] NOT_READY = "not ready".getBytes(StandardCharsets.UTF_8);
    private static final byte[] NOT_FOUND = "not found".getBytes(StandardCharsets.UTF_8);
    private static final byte[] METHOD_NOT_ALLOWED = "method not allowed".getBytes(StandardCharsets.UTF_8);

    private final HttpServer server;
    private final int port;
    private final String livenessPath;
    private final String readinessPath;
    private final String startupPath;
    private final BooleanSupplier liveness;
    private final BooleanSupplier readiness;

    /**
     * Creates and immediately starts the health server.
     *
     * @param port          the TCP port to bind on {@code 0.0.0.0} ({@code 0} binds an ephemeral port,
     *                      useful for tests; the actual port is then available via {@link #getPort()})
     * @param livenessPath  the liveness route path (e.g. {@code /livez})
     * @param readinessPath the readiness route path (e.g. {@code /readyz})
     * @param startupPath   the startup route path (e.g. {@code /startupz}; reuses readiness semantics)
     * @param liveness      supplier consulted for the liveness route; must not check external deps
     * @param readiness     supplier consulted for the readiness and startup routes
     * @throws IOException if the port cannot be bound
     */
    public HealthServer(int port, String livenessPath, String readinessPath, String startupPath,
                        BooleanSupplier liveness, BooleanSupplier readiness) throws IOException
    {
        this.livenessPath = livenessPath;
        this.readinessPath = readinessPath;
        this.startupPath = startupPath;
        this.liveness = liveness;
        this.readiness = readiness;

        // Bind 0.0.0.0 so the endpoint is reachable from the kubelet across the pod network.
        this.server = HttpServer.create(new InetSocketAddress("0.0.0.0", port), 0);
        // Single root context: route by exact path in one handler so unknown paths return 404
        // (an HttpServer context is a longest-prefix match, which would otherwise also catch
        // sub-paths/typos as a known route).
        this.server.createContext("/", this::handle);
        // Daemon handler threads so the server never keeps the JVM alive; close() stops it cleanly.
        this.server.setExecutor(Executors.newSingleThreadExecutor(runnable -> {
            Thread thread = new Thread(runnable, "edgecommons-health");
            thread.setDaemon(true);
            return thread;
        }));
        this.server.start();
        this.port = this.server.getAddress().getPort();
    }

    /**
     * Routes a single request. Liveness is decoupled from readiness (a broker outage must not fail
     * liveness); readiness and startup share the same supplier.
     */
    private void handle(HttpExchange exchange) throws IOException
    {
        try
        {
            String path = exchange.getRequestURI().getPath();
            String method = exchange.getRequestMethod();

            int status;
            byte[] body;
            if (path.equals(livenessPath))
            {
                if (!"GET".equalsIgnoreCase(method))
                {
                    status = 405;
                    body = METHOD_NOT_ALLOWED;
                }
                else
                {
                    // The handler running already proves the process is alive; the supplier is a
                    // constant true in the library wiring and MUST NOT consult the broker.
                    boolean alive = liveness.getAsBoolean();
                    status = alive ? 200 : 503;
                    body = alive ? OK : NOT_READY;
                }
            }
            else if (path.equals(readinessPath) || path.equals(startupPath))
            {
                if (!"GET".equalsIgnoreCase(method))
                {
                    status = 405;
                    body = METHOD_NOT_ALLOWED;
                }
                else
                {
                    boolean ready = readiness.getAsBoolean();
                    status = ready ? 200 : 503;
                    body = ready ? OK : NOT_READY;
                }
            }
            else
            {
                status = 404;
                body = NOT_FOUND;
            }

            exchange.getResponseHeaders().set("Content-Type", "text/plain; charset=utf-8");
            exchange.sendResponseHeaders(status, body.length);
            try (OutputStream os = exchange.getResponseBody())
            {
                os.write(body);
            }
        }
        finally
        {
            exchange.close();
        }
    }

    /**
     * Returns the actual bound port. Equals the requested port, or the ephemeral port chosen by the
     * OS when {@code 0} was requested.
     *
     * @return the bound TCP port
     */
    public int getPort()
    {
        return port;
    }

    /**
     * Stops the server, releasing the listening socket and its threads. Idempotent and safe to call
     * during shutdown. Stops immediately (no drain delay) since probe handlers complete instantly.
     */
    public void close()
    {
        try
        {
            server.stop(0);
            LOGGER.debug("Health server on port {} stopped", port);
        }
        catch (Exception e)
        {
            LOGGER.warn("Error stopping health server: {}", e.getMessage());
        }
    }
}
