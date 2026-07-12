/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.uns;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;
import java.util.Objects;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;
import java.util.function.LongSupplier;
import java.util.function.LongUnaryOperator;

/**
 * The library-owned {@code _bcast} republish listener — the UNS "late-join lever"
 * (DESIGN-uns §9.3 layer 2 / §9.4, DESIGN-uns-bridge §2.5): every component subscribes, on its
 * PRIMARY (local/IPC) connection, the two per-device broadcast command topics for its own device —
 *
 * <pre>
 *   ecv1/{device}/_bcast/main/cmd/republish-state
 *   ecv1/{device}/_bcast/main/cmd/republish-cfg
 * </pre>
 *
 * and, on receipt, re-announces out of band: {@code republish-state} re-emits the heartbeat's
 * {@code state} keepalive ({@code {"status":"RUNNING","uptimeSecs":n}}) and {@code republish-cfg}
 * re-runs the effective-config ({@code cfg}) publisher. Both re-announces go through the
 * privileged {@link com.mbreissi.edgecommons.messaging.ReservedPublisher} seam (via the injected
 * actions), which is why this is library plumbing — component code cannot publish the reserved
 * {@code state}/{@code cfg} classes itself. The {@code uns-bridge} publishes these broadcasts on
 * every site-connection re-establishment so the site view rehydrates without broker retain; the
 * edge-console uses {@code republish-cfg} for config review.
 *
 * <p><b>Normative behavior (mirrored by the Python/Rust/TS listeners; constants pinned by
 * {@code uns-test-vectors/bcast.json}):</b>
 * <ul>
 *   <li><b>Topics</b> — built through the library topic builder with the reserved {@code _bcast}
 *       pseudo-component identity: single-level hierarchy {@code [{device: <own device>}]},
 *       component {@value #BCAST_COMPONENT}, instance {@code main}, class {@code cmd}, channel =
 *       the verb. Always <b>rootless</b> (the identity is single-level, so {@code includeRoot} is
 *       a D-U25 no-op — the broadcast topic shape is device-bus-wide, independent of any
 *       component's own hierarchy/root mode).</li>
 *   <li><b>Trigger validation</b> — the envelope's {@code header.name} must equal the topic's
 *       verb ({@value #REPUBLISH_STATE} / {@value #REPUBLISH_CFG}); the header {@code version},
 *       {@code body} and any {@code reply_to} are ignored (fire-and-forget notification, no
 *       reply). A missing header, a mismatched name, or any parse anomaly is ignored (DEBUG log)
 *       — a malformed or foreign {@code _bcast} payload must never crash a component.</li>
 *   <li><b>Jitter</b> — an accepted trigger fires after a uniformly random delay in
 *       {@code [0, }{@value #JITTER_WINDOW_MS}{@code ]} ms (the §9.3 "wait a random 0 to 2 s"
 *       anti-stampede window: a whole fleet receives the broadcast at once). The randomness and
 *       clock are injected seams so the behavior unit-tests deterministically.</li>
 *   <li><b>Coalescing / cooldown (per verb, independent)</b> — a trigger is accepted only when no
 *       re-announce for that verb is pending AND at least {@value #COOLDOWN_MS} ms have elapsed
 *       since the last <em>accepted</em> trigger for that verb (measured from acceptance, not from
 *       the jittered fire). Everything else coalesces into the pending/recent re-announce, so a
 *       looping or duplicated broadcast amplifies to at most one re-announce per verb per
 *       cooldown window.</li>
 *   <li><b>No config surface</b> — always on; core plumbing, not a feature toggle. (The
 *       {@code republish-state} <em>action</em> still respects {@code heartbeat.enabled}: a
 *       component whose operator disabled the state keepalive does not re-announce state.)</li>
 * </ul>
 *
 * <p>Lifecycle: constructed and {@link #start() started} by the {@code EdgeCommons} runtime after
 * initialization completes; {@link #close()} unsubscribes both topics (before messaging closes —
 * the unsubscribe-before-exit rule) and stops the jitter scheduler. When the component identity is
 * not resolved (mock/test bring-up), the listener disables itself with a WARN, mirroring the
 * heartbeat and the effective-config publisher.
 */
public final class RepublishListener implements AutoCloseable {

    private static final Logger LOGGER = LogManager.getLogger(RepublishListener.class);

    /** The reserved broadcast pseudo-component token (UNS-CANONICAL-DESIGN §4.3). */
    public static final String BCAST_COMPONENT = "_bcast";

    /** The re-announce-state broadcast verb (channel + envelope {@code header.name}). */
    public static final String REPUBLISH_STATE = "republish-state";

    /** The re-announce-effective-config broadcast verb (channel + envelope {@code header.name}). */
    public static final String REPUBLISH_CFG = "republish-cfg";

    /**
     * The anti-stampede jitter window in ms: an accepted broadcast re-announces after a uniformly
     * random delay in {@code [0, JITTER_WINDOW_MS]} (DESIGN-uns §9.3: "a random 0 to 2s").
     * Normative for all four languages.
     */
    public static final long JITTER_WINDOW_MS = 2_000L;

    /**
     * The per-verb coalescing cooldown in ms, measured from the last ACCEPTED trigger: at most one
     * re-announce per verb per this window, so a looping/duplicated broadcast never amplifies.
     * Normative for all four languages.
     */
    public static final long COOLDOWN_MS = 5_000L;

    /**
     * The delayed-execution seam (the injected-clock discipline): production wraps a
     * single-thread scheduler; tests inject a recorder and run tasks synchronously.
     */
    @FunctionalInterface
    interface Delayer {
        void schedule(Runnable task, long delayMillis);
    }

    /** One broadcast verb's subscription + rate-limit state (guarded by {@code this}). */
    private static final class Command {
        final String verb;
        final Runnable action;
        /** The resolved concrete topic; null until {@link #start()} builds it. */
        String topic;
        /** A re-announce is scheduled but has not fired yet. */
        boolean pending;
        /** Whether {@link #lastAcceptedMs} holds a real acceptance time. */
        boolean triggered;
        /** Clock millis of the last ACCEPTED trigger (the cooldown reference point). */
        long lastAcceptedMs;

        Command(String verb, Runnable action) {
            this.verb = verb;
            this.action = action;
        }
    }

    private final ConfigManager configManager;
    private final MessagingClient messagingClient;
    private final List<Command> commands;
    private final Delayer delayer;
    private final LongSupplier clockMillis;
    private final LongUnaryOperator jitter;
    /** Non-null only when this listener created (and therefore owns) the scheduler. */
    private final ScheduledExecutorService ownedScheduler;

    private boolean started = false;
    private boolean closed = false;

    /**
     * Production wiring: a daemon single-thread jitter scheduler, the system clock, and a
     * thread-local uniform random jitter.
     *
     * @param configManager   the component's config manager (own-device identity resolution)
     * @param messagingClient the messaging client whose PRIMARY connection carries the
     *                        subscriptions
     * @param stateRepublish  the {@code republish-state} action (the heartbeat's out-of-band
     *                        state keepalive re-emit)
     * @param cfgRepublish    the {@code republish-cfg} action (the effective-config publisher's
     *                        {@code publishNow})
     */
    public RepublishListener(ConfigManager configManager, MessagingClient messagingClient,
                             Runnable stateRepublish, Runnable cfgRepublish) {
        this(configManager, messagingClient, stateRepublish, cfgRepublish,
                Executors.newSingleThreadScheduledExecutor(runnable -> {
                    Thread thread = new Thread(runnable, "RepublishListener-scheduler");
                    thread.setDaemon(true);
                    return thread;
                }),
                System::currentTimeMillis,
                window -> ThreadLocalRandom.current().nextLong(window + 1));
    }

    /** Full-injection constructor for deterministic tests (fake delayer/clock/jitter). */
    RepublishListener(ConfigManager configManager, MessagingClient messagingClient,
                      Runnable stateRepublish, Runnable cfgRepublish,
                      Delayer delayer, LongSupplier clockMillis, LongUnaryOperator jitter) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        this.commands = List.of(
                new Command(REPUBLISH_STATE,
                        Objects.requireNonNull(stateRepublish, "stateRepublish must not be null")),
                new Command(REPUBLISH_CFG,
                        Objects.requireNonNull(cfgRepublish, "cfgRepublish must not be null")));
        this.delayer = Objects.requireNonNull(delayer, "delayer must not be null");
        this.clockMillis = Objects.requireNonNull(clockMillis, "clockMillis must not be null");
        this.jitter = Objects.requireNonNull(jitter, "jitter must not be null");
        this.ownedScheduler = null;
    }

    /** Chains the production scheduler into the injection constructor, keeping ownership of it. */
    private RepublishListener(ConfigManager configManager, MessagingClient messagingClient,
                              Runnable stateRepublish, Runnable cfgRepublish,
                              ScheduledExecutorService scheduler,
                              LongSupplier clockMillis, LongUnaryOperator jitter) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        this.commands = List.of(
                new Command(REPUBLISH_STATE,
                        Objects.requireNonNull(stateRepublish, "stateRepublish must not be null")),
                new Command(REPUBLISH_CFG,
                        Objects.requireNonNull(cfgRepublish, "cfgRepublish must not be null")));
        this.delayer = (task, delayMillis) -> scheduler.schedule(task, delayMillis, TimeUnit.MILLISECONDS);
        this.clockMillis = clockMillis;
        this.jitter = jitter;
        this.ownedScheduler = scheduler;
    }

    /**
     * Builds the two own-device {@code _bcast} topics and subscribes them on the PRIMARY
     * connection. Best-effort and idempotent: with no resolved component identity (mock/test
     * bring-up) — or on any subscription failure — the listener logs and disables itself; the
     * component must come up regardless.
     */
    public synchronized void start() {
        if (started || closed) {
            return;
        }
        MessageIdentity identity = configManager.getComponentIdentity();
        if (identity == null) {
            LOGGER.warn("No resolved component identity - the _bcast republish listener is disabled");
            return;
        }
        try {
            // The reserved _bcast pseudo-component pinned to this component's own device. The
            // identity is single-level, so the topic is rootless by construction (D-U25) - the
            // broadcast shape is shared by every component on the device bus, whatever their own
            // hierarchy/root mode.
            MessageIdentity bcast = new MessageIdentity(
                    List.of(new MessageIdentity.HierEntry("device", identity.getDevice())),
                    BCAST_COMPONENT, MessageIdentity.DEFAULT_INSTANCE);
            Uns uns = new Uns(bcast, false);
            for (Command command : commands) {
                String topic = uns.topic(UnsClass.CMD, command.verb);
                messagingClient.subscribe(topic, (receivedTopic, message) -> handle(command, message));
                command.topic = topic;
            }
            started = true;
            LOGGER.info("Republish listener subscribed on '{}' and '{}'",
                    commands.get(0).topic, commands.get(1).topic);
        } catch (Exception e) {
            LOGGER.warn("Failed to start the _bcast republish listener (continuing without it): {}",
                    e.toString());
        }
    }

    /**
     * One received broadcast: validate the envelope (the {@code header.name} must equal the
     * topic's verb), then run the accept/coalesce decision. Never throws — a malformed or foreign
     * {@code _bcast} payload is ignored at DEBUG.
     */
    private void handle(Command command, Message message) {
        try {
            if (message == null || message.getHeader() == null
                    || !command.verb.equals(message.getHeader().getName())) {
                LOGGER.debug("Ignoring foreign/malformed _bcast payload on '{}'", command.topic);
                return;
            }
            onBroadcast(command);
        } catch (Exception e) {
            LOGGER.debug("Ignoring malformed _bcast payload on '{}': {}", command.topic, e.toString());
        }
    }

    /**
     * The accept/coalesce decision (per verb): coalesce while a re-announce is pending or within
     * {@value #COOLDOWN_MS} ms of the last accepted trigger; otherwise accept and schedule the
     * re-announce after a jittered delay in {@code [0, }{@value #JITTER_WINDOW_MS}{@code ]} ms.
     */
    private void onBroadcast(Command command) {
        long delayMillis;
        synchronized (this) {
            if (closed) {
                return;
            }
            long now = clockMillis.getAsLong();
            if (command.pending) {
                LOGGER.debug("'{}' broadcast coalesced (a re-announce is already pending)", command.verb);
                return;
            }
            if (command.triggered && now - command.lastAcceptedMs < COOLDOWN_MS) {
                LOGGER.debug("'{}' broadcast coalesced (within the {} ms cooldown)",
                        command.verb, COOLDOWN_MS);
                return;
            }
            command.pending = true;
            command.triggered = true;
            command.lastAcceptedMs = now;
            delayMillis = jitter.applyAsLong(JITTER_WINDOW_MS);
        }
        LOGGER.debug("'{}' broadcast accepted - re-announcing in {} ms", command.verb, delayMillis);
        delayer.schedule(() -> fire(command), delayMillis);
    }

    /** The jittered re-announce: best-effort (a failing publisher must not kill the scheduler). */
    private void fire(Command command) {
        synchronized (this) {
            command.pending = false;
            if (closed) {
                return;
            }
        }
        try {
            command.action.run();
        } catch (Exception e) {
            LOGGER.warn("'{}' re-announce failed: {}", command.verb, e.toString());
        }
    }

    /**
     * Stops the listener: unsubscribes both {@code _bcast} topics (while messaging is still up —
     * the unsubscribe-before-exit rule), drops any pending re-announce, and shuts down the owned
     * jitter scheduler. Idempotent.
     */
    @Override
    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;
        if (started) {
            for (Command command : commands) {
                if (command.topic != null) {
                    try {
                        messagingClient.unsubscribe(command.topic);
                    } catch (Exception e) {
                        LOGGER.debug("Republish-listener unsubscribe of '{}' failed: {}",
                                command.topic, e.toString());
                    }
                }
            }
        }
        if (ownedScheduler != null) {
            ownedScheduler.shutdownNow();
        }
    }
}
