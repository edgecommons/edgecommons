/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.mbreissi.edgecommons.uns.UnsScope;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Duration;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.BooleanSupplier;
import java.util.function.LongSupplier;
import java.util.List;
import java.util.function.Supplier;

import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;

/**
 * The library-owned component <b>command inbox</b> — the minimal {@code commands()} facade
 * (DESIGN-uns §7.3 / §9.5, the edge-console slice S2): every component subscribes, on its PRIMARY
 * (local/IPC) connection, its own {@code main}-instance command-inbox wildcard
 *
 * <pre>
 *   ecv1/{device}/{component}/main/cmd/#
 * </pre>
 *
 * and dispatches incoming {@code cmd} envelopes to handlers by <b>verb</b> — the topic's channel
 * (everything after {@code cmd/}, {@code /}-namespaced verbs included), which the envelope's
 * {@code header.name} must equal. A request carrying {@code header.reply_to} gets a structured
 * reply on that topic with the request's {@code correlation_id} (the {@code uns-bridge} rewrites
 * {@code reply_to} across brokers, so console→component request/reply works transparently over
 * the site bus); a {@code cmd} without {@code reply_to} is fire-and-forget (the handler runs, no
 * reply). Obtain the facade via {@code EdgeCommons.getCommands()} and register custom verbs with
 * {@link #register(String, CommandHandler)}.
 *
 * <p><b>Normative behavior (mirrored by the Python/Rust/TS inboxes; pinned by
 * {@code uns-test-vectors/commands.json}):</b>
 * <ul>
 *   <li><b>Reply body shape</b> — success {@code {"ok": true, "result": <verb-specific object>}};
 *       error {@code {"ok": false, "error": {"code": <CODE>, "message": <text>}}}. The reply
 *       envelope's {@code header.name} is the verb, {@code header.version} is
 *       {@value #CMD_MESSAGE_VERSION}, and it carries the <b>responder's</b> {@code identity}
 *       (and {@code tags}, when configured — metadata, not normative).</li>
 *   <li><b>Built-in verbs</b> (registered by the library, cannot be shadowed or unregistered):
 *       {@value #PING} → {@code {"status": "RUNNING", "uptimeSecs": n}} (liveness/echo, the state
 *       keepalive's RUNNING body shape); {@value #RELOAD_CONFIG} → re-fetch/re-apply the
 *       configuration from the active config source ({@code {"reloaded": true}} or
 *       {@value #ERR_RELOAD_FAILED}); {@value #GET_CONFIGURATION} → return the current
 *       <b>redacted effective config</b> as {@code {"config": <redacted config>}} — the same
 *       redacted snapshot the {@code cfg} push class publishes, as a reply (<b>Flow B</b>: the
 *       console pulls a component's own config; unrelated to the Flow-A
 *       {@code ecv1/{device}/config/main/cmd/get-configuration} rendezvous where a component
 *       fetches its config FROM a config server).</li>
 *   <li><b>Unknown verb</b> — a well-formed request whose verb has no handler gets an
 *       {@value #ERR_UNKNOWN_VERB} error reply (fire-and-forget unknowns are ignored at
 *       DEBUG).</li>
 *   <li><b>Malformed</b> — a missing header, a {@code header.name} that does not equal the
 *       topic's verb, or any parse anomaly is ignored at DEBUG, <b>never replied to and never a
 *       crash</b> (the G-S1 precedent; replying would race foreign conventions that use a
 *       different header name on a {@code cmd} topic).</li>
 *   <li><b>Delegated verbs</b> — {@value #SET_CONFIG_VERB} is owned by the
 *       {@code CONFIG_COMPONENT} config source's own subscription on the same inbox path; the
 *       inbox always ignores it (DEBUG) so the two subscribers never double-handle.</li>
 *   <li><b>Handler errors</b> — a {@link CommandException} keeps its code; any other exception
 *       maps to {@value #ERR_HANDLER_ERROR}. Fire-and-forget failures are logged only.</li>
 *   <li><b>No config surface</b> — always on; core plumbing, not a feature toggle.</li>
 * </ul>
 *
 * <p>Lifecycle: constructed and {@link #start() started} by the {@code EdgeCommons} runtime after
 * initialization completes; {@link #close()} unsubscribes the inbox (before messaging closes —
 * the unsubscribe-before-exit rule). When the component identity is not resolved (mock/test
 * bring-up), the inbox disables itself with a WARN, mirroring the heartbeat, the effective-config
 * publisher and the republish listener. Only the {@code main}-instance inbox is subscribed in
 * this slice; per-instance inboxes ride the full {@code commands()} facade (Phase 5).
 */
public final class CommandInbox implements AutoCloseable {

    private static final Logger LOGGER = LogManager.getLogger(CommandInbox.class);
    private static final Gson GSON = new Gson();

    /** The liveness/echo built-in verb. */
    public static final String PING = "ping";

    /** The re-fetch/re-apply-configuration built-in verb. */
    public static final String RELOAD_CONFIG = "reload-config";

    /** The return-my-redacted-effective-config built-in verb (Flow B). */
    public static final String GET_CONFIGURATION = "get-configuration";

    /** The descriptor-discovery built-in verb. */
    public static final String DESCRIBE = "describe";

    /**
     * The universal component status verb: {@code {"status":"RUNNING","uptimeSecs":n[,"instances":[…]]}}.
     *
     * <p>{@value #PING} answers only for the component as a whole. {@code status} is its per-instance
     * superset: it returns the same sample the {@code state} keepalive pushes in {@code instances[]},
     * sourced from the one component-supplied {@code InstanceConnectivityProvider}. Push and pull can
     * therefore never disagree — a console can subscribe, or ask, and get the same answer.
     *
     * <p>Every component implements it by registering that provider; a component with no instances
     * (a plain service) simply omits the section. It is deliberately <b>not</b> named {@code sb/status}:
     * a processor or a sink has no southbound, and this verb is universal.
     */
    public static final String STATUS = "status";

    /** The descriptor command schema version. */
    public static final String DESCRIBE_SCHEMA_VERSION = "edgecommons.component.describe.v1";

    /** The panel descriptor schema version. */
    public static final String PANELS_SCHEMA_VERSION = "edgecommons.panels.v2";

    /** The command request/reply envelope version. */
    public static final String CMD_MESSAGE_VERSION = "1.0";

    /** Error code: the request's verb has no registered handler on this component. */
    public static final String ERR_UNKNOWN_VERB = "UNKNOWN_VERB";

    /** Error code: the handler threw an uncoded exception. */
    public static final String ERR_HANDLER_ERROR = "HANDLER_ERROR";

    /** Error code: {@value #RELOAD_CONFIG} could not re-fetch or the document was rejected. */
    public static final String ERR_RELOAD_FAILED = "RELOAD_FAILED";

    /** Error code: {@value #GET_CONFIGURATION} found no effective configuration to return. */
    public static final String ERR_NO_CONFIG = "NO_CONFIG";

    /** Error code: a deferred command was sent without a guarded reply target. */
    public static final String ERR_REPLY_REQUIRED = "REPLY_REQUIRED";

    /** Error code: bounded deferred-reply capacity is exhausted. */
    public static final String ERR_DEFERRED_REPLY_CAPACITY = "RESOURCE_LIMIT";

    /** Error code attempted for open deferred replies during component shutdown. */
    public static final String ERR_COMPONENT_STOPPING = "COMPONENT_STOPPING";

    /** Hard bound on active provisional/open/settling deferred replies. */
    public static final int MAX_DEFERRED_REPLIES = 1024;

    /** Maximum accepted post-accept continuations (running plus queued). */
    public static final int MAX_POST_ACCEPT_CONTINUATIONS = 256;

    private static final int POST_ACCEPT_CONTINUATION_WORKERS = 4;

    /** Camera-design upper bound (31 minutes) for any one deferred reply lifetime. */
    public static final long MAX_DEFERRED_REPLY_LIFETIME_MS = 1_860_000L;

    private static final long DEFERRED_REPLY_ATTEMPT_TIMEOUT_MS = 5_000L;
    private static final long DEFERRED_REPLY_RETRY_INITIAL_MS = 100L;
    private static final long DEFERRED_REPLY_RETRY_MAX_MS = 1_000L;
    private static final long DEFERRED_REPLY_SHUTDOWN_TIMEOUT_MS = 1_000L;
    /** Default bounded wait for MQTT SUBACK / Greengrass subscription operation completion. */
    public static final Duration DEFAULT_START_TIMEOUT = Duration.ofSeconds(10);
    /** Hard bound for deliveries received after transport acknowledgement but before activation. */
    public static final int MAX_PENDING_STARTUP_DELIVERIES = 256;
    private static final int MAX_START_ERROR_CHARS = 256;

    /**
     * The {@code set-config} push verb — delegated: the {@code CONFIG_COMPONENT} config source
     * maintains its own subscription for it on the same inbox path
     * ({@code ConfigComponentProvider}), so the inbox must never dispatch or error-reply it.
     */
    public static final String SET_CONFIG_VERB = "set-config";

    /** The built-in verbs (registered at construction; shadowing/unregistering is rejected). */
    public static final Set<String> BUILT_IN_VERBS = Set.of(PING, RELOAD_CONFIG,
            GET_CONFIGURATION, DESCRIBE, STATUS);

    /** Verbs owned by other library subscriptions on the same inbox path — always ignored. */
    public static final Set<String> DELEGATED_VERBS = Set.of(SET_CONFIG_VERB);

    private final ConfigManager configManager;
    private final MessagingClient messagingClient;
    /** verb → handler; built-ins seeded at construction, custom verbs via {@link #register}. */
    private final Map<String, CommandHandler> handlers = new ConcurrentHashMap<>();
    /** verb → explicit-outcome handler; custom verbs via {@link #registerOutcome}. */
    private final Map<String, OutcomeCommandHandler> outcomeHandlers = new ConcurrentHashMap<>();
    /** panel id → descriptor; custom panels via {@link #registerPanel(JsonObject)}. */
    private final Map<String, JsonObject> panels = Collections.synchronizedMap(new LinkedHashMap<>());

    /** The instance-scoped inbox filter ({@code …/+/cmd/#}); null until {@link #start()} builds it. */
    private String inboxFilter;
    /** The component-scoped inbox filter ({@code …/cmd/#}, D‑U28); null until {@link #start()} builds it. */
    private String componentInboxFilter;
    /** The filter minus the trailing {@code #} — the verb-extraction prefix ({@code …/cmd/}). */
    private String inboxPrefix;

    private volatile StartupStatus currentStartupStatus =
            new StartupStatus(StartupState.STOPPED, "");
    private long startupGeneration = 0;
    /** Guarded by this inbox's monitor; non-null only while one generation activates/drains. */
    private ActivationGate activationGate;
    private volatile boolean closed = false;

    private record PendingDelivery(String topic, Message message) { }

    private static final class ActivationGate {
        private final long generation;
        private final String prefix;
        private final ArrayDeque<PendingDelivery> pending = new ArrayDeque<>();
        private int retained;
        private boolean draining;

        private ActivationGate(long generation, String prefix) {
            this.generation = generation;
            this.prefix = prefix;
        }
    }

    /** Observable command-inbox startup state used by readiness and operator diagnostics. */
    public enum StartupState {
        STARTING,
        ACTIVE,
        FAILED,
        STOPPED
    }

    /** Immutable current lifecycle status; {@code error} is sanitized and bounded. */
    public record StartupStatus(StartupState state, String error) { }

    /** Active deferred entries; terminal entries are removed and return their capacity permit. */
    private final Map<UUID, DeferredEntry> deferredEntries = new ConcurrentHashMap<>();
    private final Semaphore deferredCapacity = new Semaphore(MAX_DEFERRED_REPLIES);
    private final ScheduledExecutorService deferredTimer =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread thread = new Thread(r, "edgecommons-deferred-reply-timer");
                thread.setDaemon(true);
                return thread;
            });
    /** Activation drains run off the bounded start path; one virtual task per start generation. */
    private final ExecutorService activationDispatchers =
            Executors.newVirtualThreadPerTaskExecutor();
    private final ExecutorService deferredPublishers = Executors.newVirtualThreadPerTaskExecutor();
    /**
     * Bounded worker set for application work that begins only after an activated token is
     * accepted by this inbox. It is intentionally separate from reply publication so slow camera
     * work cannot starve confirmed deferred settlement.
     */
    private final ExecutorService postAcceptContinuations = new ThreadPoolExecutor(
            POST_ACCEPT_CONTINUATION_WORKERS,
            POST_ACCEPT_CONTINUATION_WORKERS,
            0L,
            TimeUnit.MILLISECONDS,
            new ArrayBlockingQueue<>(MAX_POST_ACCEPT_CONTINUATIONS
                    - POST_ACCEPT_CONTINUATION_WORKERS),
            Thread.ofVirtual().name("edgecommons-post-accept-", 0L).factory(),
            new ThreadPoolExecutor.AbortPolicy());

    private final AtomicLong deferredProvisioned = new AtomicLong();
    private final AtomicLong deferredSettled = new AtomicLong();
    private final AtomicLong deferredDiscarded = new AtomicLong();
    private final AtomicLong deferredExpired = new AtomicLong();
    private final AtomicLong deferredOpenExpired = new AtomicLong();
    private final AtomicLong deferredCancelledOnShutdown = new AtomicLong();
    private final AtomicLong deferredCapacityRejected = new AtomicLong();

    /** Observable lifecycle state of one opaque deferred reply. */
    public enum DeferredReplyState {
        PROVISIONAL,
        OPEN,
        SETTLING,
        SETTLED,
        DISCARDED,
        EXPIRED,
        CANCELLED_ON_SHUTDOWN
    }

    /** Result of asking an open token to settle. */
    public enum SettlementResult {
        /** This caller won the OPEN → SETTLING compare-and-set; publication is now retried. */
        ACCEPTED,
        /** Another settlement caller already won, whether publication is pending or complete. */
        ALREADY_SETTLED,
        /** The explicit expiration won before this settlement request. */
        EXPIRED,
        /** Component shutdown cancelled the token. */
        CANCELLED_ON_SHUTDOWN,
        /** The token was never activated or was explicitly discarded. */
        NOT_OPEN
    }

    /** Bounded registry counters for health/metrics and deterministic tests. */
    public record DeferredReplySnapshot(
            int capacity,
            int active,
            long provisioned,
            long settled,
            long discarded,
            long expired,
            long openExpired,
            long cancelledOnShutdown,
            long capacityRejected) { }

    /**
     * Opaque inbox-issued deferred-reply handle. It exposes lifecycle operations, but neither the
     * retained reply topic nor a direct publish capability.
     */
    public static final class DeferredReply {
        private final CommandInbox owner;
        private final DeferredEntry entry;

        private DeferredReply(CommandInbox owner, DeferredEntry entry) {
            this.owner = owner;
            this.entry = entry;
        }

        /** Activates this provisional token after application durable acceptance commits. */
        public boolean activate() {
            return owner.activateDeferred(entry);
        }

        /** Discards a still-provisional token after durable acceptance fails. */
        public boolean discard() {
            return owner.discardDeferred(entry);
        }

        /** Begins one standard success reply; exactly one concurrent settler can be accepted. */
        public SettlementResult settleSuccess(JsonObject result) {
            return owner.settleDeferred(entry, successBody(result));
        }

        /** Begins one standard coded error reply; exactly one concurrent settler can be accepted. */
        public SettlementResult settleError(String code, String message) {
            if (code == null || code.isEmpty()) {
                throw new IllegalArgumentException("deferred error code must be non-empty");
            }
            return owner.settleDeferred(entry, errorBody(code, message));
        }

        /** Current token state, including terminal state after registry cleanup. */
        public DeferredReplyState state() {
            return entry.state.get();
        }

        @Override
        public String toString() {
            return "DeferredReply[opaque,state=" + state() + "]";
        }
    }

    /** Retained guarded metadata only; never the full caller request/body. */
    private static final class DeferredEntry {
        private final UUID id;
        private final String verb;
        private final String correlationId;
        private final String replyTo;
        private final String requestUuid;
        private final Message requestMetadata;
        private final long expiresAtNanos;
        private final AtomicReference<DeferredReplyState> state =
                new AtomicReference<>(DeferredReplyState.PROVISIONAL);
        private final AtomicBoolean cleaned = new AtomicBoolean(false);
        private final AtomicInteger attempts = new AtomicInteger();
        private volatile Message reply;
        private volatile ScheduledFuture<?> expirationTask;

        private DeferredEntry(UUID id, String verb, String correlationId, String replyTo,
                              String requestUuid, long expiresAtNanos) {
            this.id = id;
            this.verb = verb;
            this.correlationId = correlationId;
            this.replyTo = replyTo;
            this.requestUuid = requestUuid;
            this.expiresAtNanos = expiresAtNanos;
            this.requestMetadata = MessageBuilder.create(verb, CMD_MESSAGE_VERSION)
                    .withCorrelationId(correlationId)
                    .withReplyTo(replyTo)
                    .build();
        }
    }

    /**
     * Creates the inbox and registers the built-in verbs. The verb <em>actions</em> are
     * injected seams so the built-ins unit-test deterministically; the {@code EdgeCommons} runtime
     * wires the real ones.
     *
     * @param configManager   the component's config manager (own identity resolution; reply
     *                        envelopes are config-stamped with the responder's identity/tags)
     * @param messagingClient the messaging client whose PRIMARY connection carries the inbox
     * @param uptimeSecs      the {@value #PING} uptime source (production: the heartbeat's
     *                        monotonic uptime, {@code Heartbeat::getUptimeSecs})
     * @param configReload    the {@value #RELOAD_CONFIG} action — re-fetch + re-apply from the
     *                        active config source, {@code true} on success (production:
     *                        {@code ConfigManager::reloadFromProvider})
     * @param redactedConfig  the {@value #GET_CONFIGURATION} source — the current redacted
     *                        effective config, or {@code null} when unavailable (production:
     *                        {@code EffectiveConfigPublisher::redactedEffectiveConfig})
     */
    public CommandInbox(ConfigManager configManager, MessagingClient messagingClient,
                        LongSupplier uptimeSecs, BooleanSupplier configReload,
                        Supplier<JsonObject> redactedConfig) {
        this(configManager, messagingClient, uptimeSecs, configReload, redactedConfig, List::of);
    }

    /**
     * As {@link #CommandInbox(ConfigManager, MessagingClient, LongSupplier, BooleanSupplier, Supplier)},
     * plus the {@value #STATUS} source.
     *
     * @param instanceConnectivity the {@value #STATUS} source — the live per-instance connectivity
     *                             sample (production: {@code Heartbeat::sampleInstanceConnectivity},
     *                             i.e. the very same provider the {@code state} keepalive pushes, so
     *                             the pulled answer and the pushed one cannot diverge)
     */
    public CommandInbox(ConfigManager configManager, MessagingClient messagingClient,
                        LongSupplier uptimeSecs, BooleanSupplier configReload,
                        Supplier<JsonObject> redactedConfig,
                        Supplier<List<InstanceConnectivity>> instanceConnectivity) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        Objects.requireNonNull(uptimeSecs, "uptimeSecs must not be null");
        Objects.requireNonNull(configReload, "configReload must not be null");
        Objects.requireNonNull(redactedConfig, "redactedConfig must not be null");
        Objects.requireNonNull(instanceConnectivity, "instanceConnectivity must not be null");

        // ping -> the state keepalive's RUNNING body shape: proves the component is not just
        // alive (the keepalive does that) but RESPONSIVE to addressed commands.
        handlers.put(PING, request -> {
            JsonObject result = new JsonObject();
            result.addProperty("status", "RUNNING");
            result.addProperty("uptimeSecs", uptimeSecs.getAsLong());
            return result;
        });
        // status -> ping's per-instance superset. Same body, plus the instances[] the state keepalive
        // pushes, from the same provider. A component with no instances omits the section, so a plain
        // service answers exactly as ping does.
        handlers.put(STATUS, request -> {
            JsonObject result = new JsonObject();
            result.addProperty("status", "RUNNING");
            result.addProperty("uptimeSecs", uptimeSecs.getAsLong());
            List<InstanceConnectivity> conns = instanceConnectivity.get();
            if (conns != null && !conns.isEmpty()) {
                JsonArray instances = new JsonArray();
                for (InstanceConnectivity c : conns) {
                    if (c != null) {
                        instances.add(c.toJson());
                    }
                }
                if (!instances.isEmpty()) {
                    result.add("instances", instances);
                }
            }
            return result;
        });
        // reload-config -> re-fetch from the active config source and re-apply (listeners fire,
        // so a successful reload also re-announces the cfg push as a side effect).
        handlers.put(RELOAD_CONFIG, request -> {
            if (!configReload.getAsBoolean()) {
                throw new CommandException(ERR_RELOAD_FAILED, "the configuration could not be"
                        + " re-fetched from the active config source or the document was"
                        + " rejected - see the component log");
            }
            JsonObject result = new JsonObject();
            result.addProperty("reloaded", true);
            return result;
        });
        // get-configuration (Flow B) -> the cfg class's body shape, as a reply.
        handlers.put(GET_CONFIGURATION, request -> {
            JsonObject config = redactedConfig.get();
            if (config == null) {
                throw new CommandException(ERR_NO_CONFIG,
                        "no effective configuration is available");
            }
            JsonObject result = new JsonObject();
            result.add("config", config);
            return result;
        });
        // describe -> descriptor-discovery manifest for console component-detail panels.
        handlers.put(DESCRIBE, request -> describe());
    }

    /**
     * Registers a custom verb handler — the minimal {@code commands()} registration seam. The
     * verb is one or more {@code /}-separated channel tokens ({@code "restart-pipeline"},
     * {@code "sb/status"}), each validated against the §2.2 token rule. Registration is allowed
     * before or after {@link #start()} (the inbox is a single wildcard subscription — no
     * per-verb subscribe).
     *
     * <p><b>Precedence:</b> no shadowing, ever — registering a {@linkplain #BUILT_IN_VERBS
     * built-in}, a {@linkplain #DELEGATED_VERBS delegated} or an already-registered verb throws.
     * Replace a custom handler by {@link #unregister(String)} first.
     *
     * @param verb    the verb (the {@code cmd} channel, {@code /}-namespaces allowed)
     * @param handler the handler to dispatch it to
     * @throws IllegalArgumentException when the verb is built-in/delegated/already registered
     * @throws com.mbreissi.edgecommons.uns.UnsValidationException when a verb token violates the
     *                                                           §2.2 token rule
     */
    public synchronized void register(String verb, CommandHandler handler) {
        Objects.requireNonNull(verb, "verb must not be null");
        Objects.requireNonNull(handler, "handler must not be null");
        validateCustomVerbRegistration(verb);
        if (handlers.putIfAbsent(verb, handler) != null) {
            throw new IllegalArgumentException("verb '" + verb + "' is already registered -"
                    + " unregister it first to replace the handler");
        }
        LOGGER.debug("Command verb '{}' registered", verb);
    }

    /**
     * Registers an explicit-outcome handler without changing the legacy {@link #register} API.
     * Immediate outcomes use the same wrappers as legacy handlers; a valid activated deferred
     * token suppresses the automatic reply.
     */
    public synchronized void registerOutcome(String verb, OutcomeCommandHandler handler) {
        Objects.requireNonNull(verb, "verb must not be null");
        Objects.requireNonNull(handler, "handler must not be null");
        validateCustomVerbRegistration(verb);
        if (outcomeHandlers.putIfAbsent(verb, handler) != null) {
            throw new IllegalArgumentException("verb '" + verb + "' is already registered -"
                    + " unregister it first to replace the handler");
        }
        LOGGER.debug("Outcome command verb '{}' registered", verb);
    }

    private void validateCustomVerbRegistration(String verb) {
        for (String token : verb.split("/", -1)) {
            Uns.checkToken(token, "verb token");
        }
        if (BUILT_IN_VERBS.contains(verb)) {
            throw new IllegalArgumentException("verb '" + verb + "' is a built-in verb and"
                    + " cannot be shadowed");
        }
        if (DELEGATED_VERBS.contains(verb)) {
            throw new IllegalArgumentException("verb '" + verb + "' is owned by another library"
                    + " subsystem and cannot be registered");
        }
        if (handlers.containsKey(verb) || outcomeHandlers.containsKey(verb)) {
            throw new IllegalArgumentException("verb '" + verb + "' is already registered -"
                    + " unregister it first to replace the handler");
        }
    }

    /**
     * Removes a previously registered custom verb handler. Unknown verbs are a no-op; built-in
     * verbs cannot be unregistered.
     *
     * @param verb the custom verb to remove
     * @throws IllegalArgumentException when the verb is a built-in
     */
    public synchronized void unregister(String verb) {
        Objects.requireNonNull(verb, "verb must not be null");
        if (BUILT_IN_VERBS.contains(verb)) {
            throw new IllegalArgumentException("verb '" + verb + "' is a built-in verb and"
                    + " cannot be unregistered");
        }
        if (handlers.remove(verb) != null || outcomeHandlers.remove(verb) != null) {
            LOGGER.debug("Command verb '{}' unregistered", verb);
        }
    }

    /** The currently registered verbs (built-ins + custom) — a snapshot copy. */
    public Set<String> verbs() {
        Set<String> verbs = ConcurrentHashMap.newKeySet();
        verbs.addAll(handlers.keySet());
        verbs.addAll(outcomeHandlers.keySet());
        return Set.copyOf(verbs);
    }

    /**
     * Provisions an opaque deferred-reply handle for a validated request. The handle starts in
     * {@link DeferredReplyState#PROVISIONAL}; application code must activate it only after its
     * durable acceptance record commits, or discard it on commit failure.
     *
     * @param request validated received request
     * @param lifetime positive expiration no greater than
     *                 {@link #MAX_DEFERRED_REPLY_LIFETIME_MS}
     * @throws CommandException when reply is absent, shutdown has begun, or capacity is full
     * @throws IllegalArgumentException when correlation, verb, or lifetime is invalid
     * @throws com.mbreissi.edgecommons.messaging.ReservedTopicException when {@code reply_to}
     *         targets a library-owned class
     */
    public synchronized DeferredReply defer(Message request, Duration lifetime)
            throws CommandException {
        if (closed) {
            throw new CommandException(ERR_COMPONENT_STOPPING,
                    "the command inbox is stopping");
        }
        if (request == null || request.getHeader() == null
                || request.getHeader().getReplyTo() == null
                || request.getHeader().getReplyTo().isEmpty()) {
            throw new CommandException(ERR_REPLY_REQUIRED,
                    "deferred commands require a non-empty reply_to");
        }
        Objects.requireNonNull(lifetime, "lifetime must not be null");
        final long lifetimeMillis;
        try {
            lifetimeMillis = lifetime.toMillis();
        } catch (ArithmeticException e) {
            throw new IllegalArgumentException("deferred reply lifetime is too large", e);
        }
        if (lifetimeMillis <= 0 || lifetimeMillis > MAX_DEFERRED_REPLY_LIFETIME_MS) {
            throw new IllegalArgumentException("deferred reply lifetime must be between 1 and "
                    + MAX_DEFERRED_REPLY_LIFETIME_MS + " milliseconds");
        }

        String verb = request.getHeader().getName();
        String correlationId = request.getHeader().getCorrelationId();
        if (verb == null || verb.isEmpty()) {
            throw new IllegalArgumentException("deferred request requires a non-empty verb");
        }
        if (correlationId == null || correlationId.isEmpty()) {
            throw new IllegalArgumentException(
                    "deferred request requires a non-empty correlation id");
        }
        messagingClient.validateReplyTarget(request);

        if (!deferredCapacity.tryAcquire()) {
            deferredCapacityRejected.incrementAndGet();
            throw new CommandException(ERR_DEFERRED_REPLY_CAPACITY,
                    "deferred reply registry capacity is exhausted");
        }

        boolean installed = false;
        DeferredEntry pendingEntry = null;
        try {
            UUID id;
            DeferredEntry entry;
            do {
                id = UUID.randomUUID();
                long expiresAt = System.nanoTime()
                        + TimeUnit.MILLISECONDS.toNanos(lifetimeMillis);
                entry = new DeferredEntry(id, verb, correlationId,
                        request.getHeader().getReplyTo(), request.getHeader().getUuid(), expiresAt);
            } while (deferredEntries.putIfAbsent(id, entry) != null);
            pendingEntry = entry;

            DeferredEntry installedEntry = entry;
            entry.expirationTask = deferredTimer.schedule(
                    () -> expireDeferred(installedEntry), lifetimeMillis, TimeUnit.MILLISECONDS);
            installed = true;
            deferredProvisioned.incrementAndGet();
            return new DeferredReply(this, entry);
        } catch (RejectedExecutionException e) {
            throw new CommandException(ERR_COMPONENT_STOPPING,
                    "the command inbox cannot provision a deferred reply while stopping");
        } finally {
            if (!installed) {
                if (pendingEntry != null) {
                    deferredEntries.remove(pendingEntry.id, pendingEntry);
                    pendingEntry.state.set(DeferredReplyState.CANCELLED_ON_SHUTDOWN);
                    pendingEntry.cleaned.set(true);
                }
                deferredCapacity.release();
            }
        }
    }

    /** Current bounded-registry counters. */
    public DeferredReplySnapshot deferredReplySnapshot() {
        return new DeferredReplySnapshot(
                MAX_DEFERRED_REPLIES,
                deferredEntries.size(),
                deferredProvisioned.get(),
                deferredSettled.get(),
                deferredDiscarded.get(),
                deferredExpired.get(),
                deferredOpenExpired.get(),
                deferredCancelledOnShutdown.get(),
                deferredCapacityRejected.get());
    }

    /**
     * Registers a component-detail panel descriptor for {@value #DESCRIBE}. The core library
     * validates only the stable discovery contract: a panel is a JSON object with non-empty string
     * {@code id} and {@code title}, and {@code id} is unique. All other descriptor fields are
     * carried through for the console-owned renderer.
     *
     * @param panel the panel descriptor to register
     * @throws NullPointerException     when {@code panel} is null
     * @throws IllegalArgumentException when {@code id}/{@code title} is missing, non-string, empty,
     *                                  or {@code id} is already registered
     */
    public synchronized void registerPanel(JsonObject panel) {
        Objects.requireNonNull(panel, "panel must not be null");
        String id = requiredPanelString(panel, "id");
        requiredPanelString(panel, "title");
        if (panels.containsKey(id)) {
            throw new IllegalArgumentException("panel id '" + id + "' is already registered");
        }
        panels.put(id, panel.deepCopy());
    }

    /** The currently registered panel descriptors — a snapshot copy. */
    public synchronized List<JsonObject> panels() {
        List<JsonObject> snapshot = new ArrayList<>();
        for (JsonObject panel : panels.values()) {
            snapshot.add(panel.deepCopy());
        }
        return List.copyOf(snapshot);
    }

    private static String requiredPanelString(JsonObject panel, String field) {
        if (!panel.has(field) || !panel.get(field).isJsonPrimitive()
                || !panel.get(field).getAsJsonPrimitive().isString()) {
            throw new IllegalArgumentException("panel." + field
                    + " must be a non-empty string");
        }
        String value = panel.get(field).getAsString();
        if (value.isEmpty()) {
            throw new IllegalArgumentException("panel." + field
                    + " must be a non-empty string");
        }
        return value;
    }

    private JsonObject describe() {
        JsonObject result = new JsonObject();
        result.addProperty("schemaVersion", DESCRIBE_SCHEMA_VERSION);

        MessageIdentity identity = configManager.getComponentIdentity();
        if (identity != null) {
            result.add("component", identity.toDict());
        }

        JsonArray commands = new JsonArray();
        verbs().stream().sorted().forEach(verb -> {
            JsonObject entry = new JsonObject();
            entry.addProperty("verb", verb);
            entry.addProperty("builtIn", BUILT_IN_VERBS.contains(verb));
            commands.add(entry);
        });
        result.add("commands", commands);

        JsonArray views = new JsonArray();
        for (JsonObject panel : panels()) {
            views.add(panel);
        }
        JsonObject panelSet = new JsonObject();
        panelSet.addProperty("schemaVersion", PANELS_SCHEMA_VERSION);
        panelSet.addProperty("provider", identity == null ? "component" : identity.getComponent());
        panelSet.addProperty("renderer", "descriptor");
        if (views.size() > 0) {
            panelSet.addProperty("defaultView",
                    views.get(0).getAsJsonObject().get("id").getAsString());
        }
        panelSet.add("views", views);
        result.add("panels", panelSet);

        JsonObject digestSource = new JsonObject();
        digestSource.add("commands", commands.deepCopy());
        digestSource.add("panels", panelSet.deepCopy());
        result.addProperty("digest", sha256Digest(digestSource));

        return result;
    }

    private static String sha256Digest(JsonObject source) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] bytes = digest.digest(stableJson(source).getBytes(StandardCharsets.UTF_8));
            return "sha256:" + HexFormat.of().formatHex(bytes);
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException("SHA-256 digest algorithm is unavailable", e);
        }
    }

    private static String stableJson(JsonElement element) {
        if (element == null || element.isJsonNull() || element.isJsonPrimitive()) {
            return element == null ? "null" : element.toString();
        }
        if (element.isJsonArray()) {
            StringBuilder out = new StringBuilder("[");
            JsonArray array = element.getAsJsonArray();
            for (int i = 0; i < array.size(); i++) {
                if (i > 0) {
                    out.append(',');
                }
                out.append(stableJson(array.get(i)));
            }
            return out.append(']').toString();
        }
        StringBuilder out = new StringBuilder("{");
        boolean first = true;
        for (Map.Entry<String, JsonElement> entry : element.getAsJsonObject().entrySet().stream()
                .sorted(Map.Entry.comparingByKey()).toList()) {
            if (!first) {
                out.append(',');
            }
            out.append(GSON.toJson(entry.getKey())).append(':').append(stableJson(entry.getValue()));
            first = false;
        }
        return out.append('}').toString();
    }

    /**
     * Builds the own-inbox wildcard ({@code ecv1/{device}/{component}/main/cmd/#}, through the
     * topic builder under this component's identity + root mode) and subscribes it on the PRIMARY
     * connection. Idempotent while STARTING/ACTIVE. With no resolved component identity or on a
     * subscription failure, the observable state is FAILED and the sanitized error is retained;
     * runtime readiness remains false until a later successful start generation.
     */
    public void start() {
        start(DEFAULT_START_TIMEOUT);
    }

    /**
     * Starts one lifecycle generation. ACTIVE is published only after the transport proves MQTT
     * SUBACK or Greengrass subscription-operation completion. A failed or stale attempt performs
     * best-effort partial-subscription cleanup and may be retried deterministically.
     */
    public StartupStatus start(Duration timeout) {
        Objects.requireNonNull(timeout, "timeout must not be null");
        if (timeout.isZero() || timeout.isNegative()) {
            throw new IllegalArgumentException("command inbox start timeout must be positive");
        }

        final long generation;
        final String filter;
        final String componentFilter;
        final String prefix;
        final ActivationGate gate;
        synchronized (this) {
            if (closed) {
                return new StartupStatus(StartupState.STOPPED, "command inbox is closed");
            }
            if (currentStartupStatus.state() == StartupState.ACTIVE
                    || currentStartupStatus.state() == StartupState.STARTING) {
                return startupStatus();
            }
            currentStartupStatus = new StartupStatus(StartupState.STARTING, "");
            generation = ++startupGeneration;

            MessageIdentity identity = configManager.getComponentIdentity();
            if (identity == null) {
                failStartLocked(generation, "no resolved component identity");
                LOGGER.warn("No resolved component identity - the command inbox is disabled");
                return startupStatus();
            }
            try {
                Uns uns = new Uns(identity, configManager.isTopicIncludeRoot());
                String site = identity.getHier().size() >= 2
                        ? identity.getHier().get(0).value() : null;
                // D‑U28: the component identity is component-scoped (no instance), so a plain filter
                // renders the instance slot as '+' (instance-scoped: .../+/cmd/#); the component-scope
                // overload omits the instance slot (.../cmd/#). Subscribe both.
                UnsScope scope = new UnsScope(
                        site, identity.getDevice(), identity.getComponent(), identity.getInstance());
                filter = uns.filter(UnsClass.CMD, scope);
                componentFilter = uns.filter(UnsClass.CMD, scope, false);
                prefix = filter.substring(0, filter.length() - 1);
                inboxFilter = filter;
                componentInboxFilter = componentFilter;
                inboxPrefix = prefix;
                gate = new ActivationGate(generation, prefix);
                activationGate = gate;
            } catch (RuntimeException e) {
                failStartLocked(generation, e.toString());
                LOGGER.warn("Failed to prepare the command inbox: {}", e.toString());
                return startupStatus();
            }
        }

        try {
            messagingClient.subscribeAcknowledged(filter,
                    (topic, message) -> receiveDuringActivation(gate, topic, message),
                    -1, MessagingClient.DEFAULT_MAX_MESSAGES, timeout);
            messagingClient.subscribeAcknowledged(componentFilter,
                    (topic, message) -> receiveDuringActivation(gate, topic, message),
                    -1, MessagingClient.DEFAULT_MAX_MESSAGES, timeout);
        } catch (Exception e) {
            unsubscribeQuietly(filter);
            unsubscribeQuietly(componentFilter);
            synchronized (this) {
                failStartLocked(generation, e.toString());
                StartupStatus failed = startupStatus();
                LOGGER.warn("Failed to start the command inbox: {}", failed.error());
                return failed;
            }
        }

        boolean stale;
        synchronized (this) {
            stale = closed || startupGeneration != generation
                    || currentStartupStatus.state() != StartupState.STARTING;
            if (!stale) {
                if (gate.pending.isEmpty()) {
                    activationGate = null;
                } else {
                    gate.draining = true;
                    try {
                        activationDispatchers.execute(() -> drainActivationGate(gate));
                    } catch (RejectedExecutionException e) {
                        failStartLocked(generation,
                                "command activation dispatcher rejected startup work");
                        stale = true;
                    }
                }
                if (!stale) {
                    currentStartupStatus = new StartupStatus(StartupState.ACTIVE, "");
                    LOGGER.info("Command inbox subscribed on '{}' and '{}' (verbs: {})",
                            filter, componentFilter, verbs());
                }
            }
        }
        if (stale) {
            unsubscribeQuietly(filter);
            unsubscribeQuietly(componentFilter);
        }
        return startupStatus();
    }

    /** Current observable lifecycle state. */
    public StartupStatus startupStatus() {
        return currentStartupStatus;
    }

    /** Stops the active generation without permanently closing the inbox; a later start may retry. */
    public void stop() {
        String filter;
        String componentFilter;
        synchronized (this) {
            startupGeneration++;
            currentStartupStatus = new StartupStatus(StartupState.STOPPED, "");
            filter = inboxFilter;
            componentFilter = componentInboxFilter;
            inboxFilter = null;
            componentInboxFilter = null;
            inboxPrefix = null;
            clearActivationGateLocked();
        }
        unsubscribeQuietly(filter);
        unsubscribeQuietly(componentFilter);
    }

    private void failStartLocked(long generation, String error) {
        if (startupGeneration != generation
                || currentStartupStatus.state() != StartupState.STARTING) {
            return;
        }
        inboxFilter = null;
        componentInboxFilter = null;
        inboxPrefix = null;
        clearActivationGateLocked();
        currentStartupStatus = new StartupStatus(
                StartupState.FAILED, sanitizeStartupError(error));
    }

    private void clearActivationGateLocked() {
        if (activationGate != null) {
            activationGate.pending.clear();
            activationGate.retained = 0;
            activationGate.draining = false;
            activationGate = null;
        }
    }

    private static String sanitizeStartupError(String error) {
        String value = error == null ? "" : error;
        StringBuilder safe = new StringBuilder(Math.min(value.length(), MAX_START_ERROR_CHARS));
        for (int i = 0; i < value.length() && safe.length() < MAX_START_ERROR_CHARS; i++) {
            char c = value.charAt(i);
            safe.append(Character.isISOControl(c) ? ' ' : c);
        }
        return safe.toString()
                .replaceAll("(?i)(password|passwd|token|secret)\\s*[=:]\\s*[^,; ]+", "$1=***")
                .replaceAll("://[^/@ ]+@", "://***@")
                .replaceAll("\\s+", " ")
                .trim();
    }

    private void unsubscribeQuietly(String filter) {
        if (filter == null) {
            return;
        }
        try {
            messagingClient.unsubscribe(filter);
        } catch (Exception e) {
            LOGGER.debug("Command-inbox unsubscribe of '{}' failed: {}", filter, e.toString());
        }
    }

    /**
     * Transport callback for one start generation. Deliveries racing acknowledged subscribe are
     * retained in a small bounded gate until ACTIVE is atomically published. Failed/stale attempts
     * clear the gate and can never dispatch those messages.
     */
    private void receiveDuringActivation(ActivationGate gate, String topic, Message message) {
        boolean dispatchNow = false;
        synchronized (this) {
            if (closed || startupGeneration != gate.generation) {
                return;
            }
            StartupState state = currentStartupStatus.state();
            if (state == StartupState.STARTING
                    || (state == StartupState.ACTIVE
                    && activationGate == gate && gate.draining)) {
                if (activationGate != gate) {
                    return;
                }
                if (gate.retained >= MAX_PENDING_STARTUP_DELIVERIES) {
                    LOGGER.warn("Dropping command delivery on '{}' because the bounded startup "
                            + "activation queue is full ({})", topic,
                            MAX_PENDING_STARTUP_DELIVERIES);
                    return;
                }
                gate.pending.addLast(new PendingDelivery(topic, message));
                gate.retained++;
                return;
            }
            dispatchNow = state == StartupState.ACTIVE;
        }
        if (dispatchNow) {
            dispatchDelivery(gate.generation, gate.prefix, topic, message);
        }
    }

    /** Drains pre-ACTIVE messages in arrival order while new callbacks continue to enqueue. */
    private void drainActivationGate(ActivationGate gate) {
        while (true) {
            List<PendingDelivery> batch;
            synchronized (this) {
                if (closed || startupGeneration != gate.generation
                        || currentStartupStatus.state() != StartupState.ACTIVE
                        || activationGate != gate) {
                    gate.pending.clear();
                    gate.retained = 0;
                    return;
                }
                if (gate.pending.isEmpty()) {
                    gate.draining = false;
                    activationGate = null;
                    return;
                }
                batch = new ArrayList<>(gate.pending);
                gate.pending.clear();
            }
            for (PendingDelivery delivery : batch) {
                try {
                    dispatchDelivery(gate.generation, gate.prefix,
                            delivery.topic(), delivery.message());
                } finally {
                    synchronized (this) {
                        if (gate.retained > 0) {
                            gate.retained--;
                        }
                    }
                }
            }
        }
    }

    /**
     * One received {@code cmd} envelope: extract the verb from the topic, validate the envelope
     * ({@code header.name} must equal the verb), dispatch, reply. Never throws — a malformed or
     * foreign payload is ignored at DEBUG.
     */
    private void dispatchDelivery(long generation, String prefix, String topic, Message message) {
        try {
            synchronized (this) {
                if (closed || currentStartupStatus.state() != StartupState.ACTIVE
                        || startupGeneration != generation) {
                    return;
                }
            }
            // D‑U28: the instance slot is optional, so a command arrives on either
            // ".../{instance}/cmd/{verb}" or ".../cmd/{verb}". Locate the "/cmd/" class marker and take
            // the verb after it — unambiguous for both scopes (an instance is never a class token).
            if (topic == null) {
                return;
            }
            int cmdMarker = topic.indexOf("/cmd/");
            if (cmdMarker < 0) {
                // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
                LOGGER.debug("Ignoring cmd delivery without a '/cmd/' segment: '{}'", topic);
                return;
            }
            String verb = topic.substring(cmdMarker + 5);   // 5 = "/cmd/".length()
            if (verb.isEmpty()) {
                return;
            }
            if (DELEGATED_VERBS.contains(verb)) {
                LOGGER.debug("Ignoring delegated verb '{}' (owned by another library"
                        + " subscription)", verb);
                return;
            }
            if (message == null || message.getHeader() == null
                    || !verb.equals(message.getHeader().getName())) {
                // Malformed/foreign: never replied to (a reply would race foreign conventions
                // using a different header name on a cmd topic), never a crash.
                LOGGER.debug("Ignoring malformed/foreign cmd payload on '{}' (header.name must"
                        + " equal the topic verb)", topic);
                return;
            }
            dispatch(verb, message);
        } catch (Exception e) {
            LOGGER.debug("Ignoring malformed cmd payload on '{}': {}", topic, e.toString());
        }
    }

    /** Dispatches a well-formed request to its handler and replies (when {@code reply_to} set). */
    private void dispatch(String verb, Message request) {
        boolean wantsReply = request.getHeader().getReplyTo() != null
                && !request.getHeader().getReplyTo().isEmpty();
        OutcomeCommandHandler outcomeHandler = outcomeHandlers.get(verb);
        CommandHandler handler = handlers.get(verb);
        if (handler == null && outcomeHandler == null) {
            if (wantsReply) {
                LOGGER.debug("Unknown verb '{}' - sending {} error reply", verb, ERR_UNKNOWN_VERB);
                sendReply(request, verb, errorBody(ERR_UNKNOWN_VERB,
                        "verb '" + verb + "' is not registered on this component"));
            } else {
                LOGGER.debug("Ignoring unknown fire-and-forget verb '{}'", verb);
            }
            return;
        }

        if (outcomeHandler != null) {
            dispatchOutcome(verb, request, wantsReply, outcomeHandler);
            return;
        }

        JsonObject result;
        try {
            result = handler.handle(request);
        } catch (CommandException e) {
            if (wantsReply) {
                sendReply(request, verb, errorBody(e.getCode(), e.getMessage()));
            } else {
                LOGGER.warn("Fire-and-forget verb '{}' failed ({}): {}", verb, e.getCode(),
                        e.getMessage());
            }
            return;
        } catch (Exception e) {
            if (wantsReply) {
                sendReply(request, verb, errorBody(ERR_HANDLER_ERROR, e.toString()));
            } else {
                LOGGER.warn("Fire-and-forget verb '{}' failed: {}", verb, e.toString());
            }
            return;
        }
        if (wantsReply) {
            sendReply(request, verb, successBody(result));
        }
    }

    private void dispatchOutcome(String verb, Message request, boolean wantsReply,
                                 OutcomeCommandHandler handler) {
        final CommandOutcome outcome;
        try {
            outcome = handler.handle(request);
            if (outcome == null) {
                throw new IllegalStateException("outcome handler returned null");
            }
        } catch (CommandException e) {
            handleOutcomeError(verb, request, wantsReply, e.getCode(), e.getMessage());
            return;
        } catch (Exception e) {
            handleOutcomeError(verb, request, wantsReply, ERR_HANDLER_ERROR, e.toString());
            return;
        }

        switch (outcome) {
            case CommandOutcome.ImmediateSuccess success -> {
                if (wantsReply) {
                    sendReply(request, verb, successBody(success.result()));
                }
            }
            case CommandOutcome.ImmediateError error ->
                    handleOutcomeError(verb, request, wantsReply,
                            error.code(), error.message());
            case CommandOutcome.Deferred deferred -> {
                DeferredReply token = deferred.token();
                if (validReturnedDeferred(token, request, verb)) {
                    Runnable continuation = deferred.postAcceptContinuation();
                    if (continuation != null) {
                        // This stronger check is deliberately adjacent to scheduling. A legacy
                        // deferred result remains backward compatible with a token already
                        // settling, but a post-accept continuation may begin only from OPEN.
                        if (token.entry.state.get() != DeferredReplyState.OPEN) {
                            handleOutcomeError(verb, request, wantsReply, ERR_HANDLER_ERROR,
                                    "post-accept continuation requires an open deferred token");
                            return;
                        }
                        startPostAcceptContinuation(token, continuation);
                    }
                    // Deliberately no automatic reply. The delivery callback returns immediately;
                    // its normal subscription-concurrency permit is no longer held by job work.
                    return;
                }
                if (token.owner == this
                        && token.entry.state.get() == DeferredReplyState.PROVISIONAL) {
                    discardDeferred(token.entry);
                }
                handleOutcomeError(verb, request, wantsReply, ERR_HANDLER_ERROR,
                        "handler returned an invalid, inactive, or foreign deferred token");
            }
        }
    }

    /**
     * Starts application work only after the current dispatch has accepted its exact open token.
     * Queue rejection settles the token with the standard guarded error path instead of leaking
     * an open registry entry or running work synchronously on the command delivery thread.
     */
    private void startPostAcceptContinuation(DeferredReply token, Runnable continuation) {
        try {
            postAcceptContinuations.execute(() -> {
                try {
                    continuation.run();
                } catch (RuntimeException e) {
                    LOGGER.warn("Post-accept deferred continuation failed: {}", e.toString());
                    token.settleError(ERR_HANDLER_ERROR,
                            "the deferred command continuation failed");
                }
            });
        } catch (RejectedExecutionException e) {
            LOGGER.warn("Post-accept deferred continuation capacity exhausted");
            token.settleError(ERR_HANDLER_ERROR,
                    "the deferred command continuation could not be started");
        }
    }

    private void handleOutcomeError(String verb, Message request, boolean wantsReply,
                                    String code, String message) {
        if (wantsReply) {
            sendReply(request, verb, errorBody(code, message));
        } else {
            LOGGER.warn("Fire-and-forget outcome verb '{}' failed ({}): {}",
                    verb, code, message);
        }
    }

    private boolean validReturnedDeferred(DeferredReply token, Message request, String verb) {
        if (token == null || token.owner != this) {
            return false;
        }
        DeferredReplyState state = token.entry.state.get();
        if (state != DeferredReplyState.OPEN
                && state != DeferredReplyState.SETTLING
                && state != DeferredReplyState.SETTLED) {
            return false;
        }
        return Objects.equals(token.entry.verb, verb)
                && Objects.equals(token.entry.replyTo, request.getHeader().getReplyTo())
                && Objects.equals(token.entry.correlationId,
                        request.getHeader().getCorrelationId())
                && Objects.equals(token.entry.requestUuid, request.getHeader().getUuid());
    }

    /** The success reply body {@code {"ok": true, "result": ...}}. */
    private static JsonObject successBody(JsonObject result) {
        JsonObject body = new JsonObject();
        body.addProperty("ok", true);
        body.add("result", result == null ? new JsonObject() : result.deepCopy());
        return body;
    }

    /** The error reply body {@code {"ok": false, "error": {"code", "message"}}}. */
    private static JsonObject errorBody(String code, String message) {
        JsonObject error = new JsonObject();
        error.addProperty("code", code);
        error.addProperty("message", message == null ? "" : message);
        JsonObject body = new JsonObject();
        body.addProperty("ok", false);
        body.add("error", error);
        return body;
    }

    private boolean activateDeferred(DeferredEntry entry) {
        if (entry.state.compareAndSet(
                DeferredReplyState.PROVISIONAL, DeferredReplyState.OPEN)) {
            return true;
        }
        return false;
    }

    private boolean discardDeferred(DeferredEntry entry) {
        if (entry.state.compareAndSet(
                DeferredReplyState.PROVISIONAL, DeferredReplyState.DISCARDED)) {
            deferredDiscarded.incrementAndGet();
            cleanupDeferred(entry);
            return true;
        }
        return false;
    }

    private SettlementResult settleDeferred(DeferredEntry entry, JsonObject body) {
        Message reply;
        try {
            reply = MessageBuilder.create(entry.verb, CMD_MESSAGE_VERSION)
                    .withCommand(body.deepCopy())
                    .withConfig(configManager)
                    .build();
        } catch (Exception e) {
            LOGGER.warn("Could not build deferred command reply for verb '{}': {}",
                    entry.verb, e.toString());
            return SettlementResult.NOT_OPEN;
        }

        if (!entry.state.compareAndSet(
                DeferredReplyState.OPEN, DeferredReplyState.SETTLING)) {
            return settlementResultFor(entry.state.get());
        }
        entry.reply = reply;
        scheduleDeferredAttempt(entry, 0L);
        return SettlementResult.ACCEPTED;
    }

    private static SettlementResult settlementResultFor(DeferredReplyState state) {
        return switch (state) {
            case SETTLING, SETTLED -> SettlementResult.ALREADY_SETTLED;
            case EXPIRED -> SettlementResult.EXPIRED;
            case CANCELLED_ON_SHUTDOWN -> SettlementResult.CANCELLED_ON_SHUTDOWN;
            case PROVISIONAL, DISCARDED, OPEN -> SettlementResult.NOT_OPEN;
        };
    }

    private void scheduleDeferredAttempt(DeferredEntry entry, long delayMillis) {
        if (entry.state.get() != DeferredReplyState.SETTLING) {
            return;
        }
        try {
            deferredTimer.schedule(() -> {
                try {
                    deferredPublishers.execute(() -> publishDeferredAttempt(entry));
                } catch (RejectedExecutionException e) {
                    cancelDeferredOnShutdown(entry);
                }
            }, Math.max(0L, delayMillis), TimeUnit.MILLISECONDS);
        } catch (RejectedExecutionException e) {
            cancelDeferredOnShutdown(entry);
        }
    }

    private void publishDeferredAttempt(DeferredEntry entry) {
        if (entry.state.get() != DeferredReplyState.SETTLING) {
            return;
        }
        long remainingNanos = entry.expiresAtNanos - System.nanoTime();
        if (remainingNanos <= 0) {
            expireSettlingDeferred(entry);
            return;
        }

        long remainingMillis = Math.max(1L,
                TimeUnit.NANOSECONDS.toMillis(remainingNanos - 1L) + 1L);
        long attemptMillis = Math.min(DEFERRED_REPLY_ATTEMPT_TIMEOUT_MS, remainingMillis);
        int attempt = entry.attempts.incrementAndGet();
        try {
            messagingClient.replyConfirmed(
                    entry.requestMetadata, entry.reply, Duration.ofMillis(attemptMillis));
            if (entry.state.compareAndSet(
                    DeferredReplyState.SETTLING, DeferredReplyState.SETTLED)) {
                deferredSettled.incrementAndGet();
                cleanupDeferred(entry);
            }
        } catch (Exception e) {
            if (entry.state.get() != DeferredReplyState.SETTLING) {
                return;
            }
            remainingNanos = entry.expiresAtNanos - System.nanoTime();
            if (remainingNanos <= 0) {
                expireSettlingDeferred(entry);
                return;
            }
            long exponent = Math.min(10L, Math.max(0L, attempt - 1L));
            long retryMillis = Math.min(DEFERRED_REPLY_RETRY_MAX_MS,
                    DEFERRED_REPLY_RETRY_INITIAL_MS << exponent);
            long untilExpiration = Math.max(1L,
                    TimeUnit.NANOSECONDS.toMillis(remainingNanos));
            retryMillis = Math.min(retryMillis, untilExpiration);
            LOGGER.debug("Deferred reply attempt {} for verb '{}' failed; retrying in {} ms: {}",
                    attempt, entry.verb, retryMillis, e.toString());
            scheduleDeferredAttempt(entry, retryMillis);
        }
    }

    private void expireDeferred(DeferredEntry entry) {
        while (true) {
            DeferredReplyState state = entry.state.get();
            if (state == DeferredReplyState.PROVISIONAL) {
                if (entry.state.compareAndSet(state, DeferredReplyState.EXPIRED)) {
                    deferredExpired.incrementAndGet();
                    cleanupDeferred(entry);
                    return;
                }
                continue;
            }
            if (state == DeferredReplyState.OPEN) {
                if (entry.state.compareAndSet(state, DeferredReplyState.EXPIRED)) {
                    recordOpenExpiration(entry);
                    return;
                }
                continue;
            }
            if (state == DeferredReplyState.SETTLING) {
                // The strict attempt is already bounded by this same expiration. It owns the
                // final SETTLED-vs-EXPIRED decision when that acknowledgement wait completes.
                return;
            }
            return;
        }
    }

    private void expireSettlingDeferred(DeferredEntry entry) {
        if (entry.state.compareAndSet(
                DeferredReplyState.SETTLING, DeferredReplyState.EXPIRED)) {
            recordOpenExpiration(entry);
        }
    }

    private void recordOpenExpiration(DeferredEntry entry) {
        deferredExpired.incrementAndGet();
        deferredOpenExpired.incrementAndGet();
        LOGGER.warn("DEFERRED_REPLY_EXPIRED: open deferred reply for verb '{}' expired after {}"
                        + " confirmed publication attempt(s)",
                entry.verb, entry.attempts.get());
        cleanupDeferred(entry);
    }

    private void cancelDeferredOnShutdown(DeferredEntry entry) {
        while (true) {
            DeferredReplyState state = entry.state.get();
            if (state == DeferredReplyState.SETTLED
                    || state == DeferredReplyState.DISCARDED
                    || state == DeferredReplyState.EXPIRED
                    || state == DeferredReplyState.CANCELLED_ON_SHUTDOWN) {
                return;
            }
            if (entry.state.compareAndSet(state,
                    DeferredReplyState.CANCELLED_ON_SHUTDOWN)) {
                deferredCancelledOnShutdown.incrementAndGet();
                cleanupDeferred(entry);
                return;
            }
        }
    }

    private void cleanupDeferred(DeferredEntry entry) {
        if (!entry.cleaned.compareAndSet(false, true)) {
            return;
        }
        deferredEntries.remove(entry.id, entry);
        ScheduledFuture<?> expirationTask = entry.expirationTask;
        if (expirationTask != null) {
            expirationTask.cancel(false);
        }
        deferredCapacity.release();
    }

    /**
     * Publishes a reply to the request's {@code reply_to} through the existing reply mechanism
     * (the provider stamps the request's {@code correlation_id} onto the reply). The reply is
     * config-stamped, so it carries the responder's {@code identity} (+ {@code tags}).
     * Best-effort: a failing reply (e.g. a hostile reserved-class {@code reply_to} rejected by
     * the guard) is logged and swallowed.
     */
    private void sendReply(Message request, String verb, JsonObject body) {
        try {
            Message reply = MessageBuilder.create(verb, CMD_MESSAGE_VERSION)
                    .withCommand(body)
                    .withConfig(configManager)
                    .build();
            messagingClient.reply(request, reply);
        } catch (Exception e) {
            LOGGER.warn("Command reply for verb '{}' failed: {}", verb, e.toString());
        }
    }

    /**
     * Stops the inbox: unsubscribes the inbox wildcard (while messaging is still up — the
     * unsubscribe-before-exit rule) and stops dispatching. Idempotent.
     */
    @Override
    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;
        startupGeneration++;
        currentStartupStatus = new StartupStatus(StartupState.STOPPED, "");
        String filterToUnsubscribe = inboxFilter;
        String componentFilterToUnsubscribe = componentInboxFilter;
        inboxFilter = null;
        componentInboxFilter = null;
        inboxPrefix = null;
        clearActivationGateLocked();

        deferredTimer.shutdownNow();
        activationDispatchers.shutdownNow();
        deferredPublishers.shutdownNow();
        postAcceptContinuations.shutdownNow();

        // Messaging is intentionally still available here. Active OPEN tokens get one bounded
        // standard COMPONENT_STOPPING attempt before their reply capability is cancelled. The
        // attempts run concurrently so 1,024 open tokens do not multiply the shutdown deadline.
        ExecutorService stoppingReplies = Executors.newVirtualThreadPerTaskExecutor();
        for (DeferredEntry entry : List.copyOf(deferredEntries.values())) {
            if (entry.state.compareAndSet(
                    DeferredReplyState.OPEN, DeferredReplyState.SETTLING)) {
                stoppingReplies.submit(() -> attemptStoppingReply(entry));
            } else {
                cancelDeferredOnShutdown(entry);
            }
        }
        stoppingReplies.shutdown();
        try {
            if (!stoppingReplies.awaitTermination(
                    DEFERRED_REPLY_SHUTDOWN_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                stoppingReplies.shutdownNow();
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            stoppingReplies.shutdownNow();
        }
        // A strict provider that failed to honor interruption must not retain registry capacity.
        for (DeferredEntry entry : List.copyOf(deferredEntries.values())) {
            cancelDeferredOnShutdown(entry);
        }

        unsubscribeQuietly(filterToUnsubscribe);
    }

    private void attemptStoppingReply(DeferredEntry entry) {
        try {
            Message reply = MessageBuilder.create(entry.verb, CMD_MESSAGE_VERSION)
                    .withCommand(errorBody(ERR_COMPONENT_STOPPING,
                            "the component stopped before the deferred command could reply"))
                    .withConfig(configManager)
                    .build();
            long remainingNanos = entry.expiresAtNanos - System.nanoTime();
            if (remainingNanos > 0) {
                long remainingMillis = Math.max(1L,
                        TimeUnit.NANOSECONDS.toMillis(remainingNanos - 1L) + 1L);
                messagingClient.replyConfirmed(entry.requestMetadata, reply,
                        Duration.ofMillis(Math.min(
                                DEFERRED_REPLY_SHUTDOWN_TIMEOUT_MS, remainingMillis)));
            }
        } catch (Exception e) {
            LOGGER.debug("Deferred COMPONENT_STOPPING reply for verb '{}' failed: {}",
                    entry.verb, e.toString());
        } finally {
            if (entry.state.compareAndSet(
                    DeferredReplyState.SETTLING,
                    DeferredReplyState.CANCELLED_ON_SHUTDOWN)) {
                deferredCancelledOnShutdown.incrementAndGet();
                cleanupDeferred(entry);
            }
        }
    }
}
