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
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.BooleanSupplier;
import java.util.function.LongSupplier;
import java.util.function.Supplier;

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

    /** The liveness/echo built-in verb. */
    public static final String PING = "ping";

    /** The re-fetch/re-apply-configuration built-in verb. */
    public static final String RELOAD_CONFIG = "reload-config";

    /** The return-my-redacted-effective-config built-in verb (Flow B). */
    public static final String GET_CONFIGURATION = "get-configuration";

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

    /**
     * The {@code set-config} push verb — delegated: the {@code CONFIG_COMPONENT} config source
     * maintains its own subscription for it on the same inbox path
     * ({@code ConfigComponentProvider}), so the inbox must never dispatch or error-reply it.
     */
    public static final String SET_CONFIG_VERB = "set-config";

    /** The built-in verbs (registered at construction; shadowing/unregistering is rejected). */
    public static final Set<String> BUILT_IN_VERBS = Set.of(PING, RELOAD_CONFIG, GET_CONFIGURATION);

    /** Verbs owned by other library subscriptions on the same inbox path — always ignored. */
    public static final Set<String> DELEGATED_VERBS = Set.of(SET_CONFIG_VERB);

    private final ConfigManager configManager;
    private final MessagingClient messagingClient;
    /** verb → handler; built-ins seeded at construction, custom verbs via {@link #register}. */
    private final Map<String, CommandHandler> handlers = new ConcurrentHashMap<>();

    /** The subscribed inbox filter ({@code …/cmd/#}); null until {@link #start()} builds it. */
    private String inboxFilter;
    /** The filter minus the trailing {@code #} — the verb-extraction prefix ({@code …/cmd/}). */
    private String inboxPrefix;

    private boolean started = false;
    private boolean closed = false;

    /**
     * Creates the inbox and registers the three built-in verbs. The verb <em>actions</em> are
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
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        Objects.requireNonNull(uptimeSecs, "uptimeSecs must not be null");
        Objects.requireNonNull(configReload, "configReload must not be null");
        Objects.requireNonNull(redactedConfig, "redactedConfig must not be null");

        // ping -> the state keepalive's RUNNING body shape: proves the component is not just
        // alive (the keepalive does that) but RESPONSIVE to addressed commands.
        handlers.put(PING, request -> {
            JsonObject result = new JsonObject();
            result.addProperty("status", "RUNNING");
            result.addProperty("uptimeSecs", uptimeSecs.getAsLong());
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
    public void register(String verb, CommandHandler handler) {
        Objects.requireNonNull(verb, "verb must not be null");
        Objects.requireNonNull(handler, "handler must not be null");
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
        if (handlers.putIfAbsent(verb, handler) != null) {
            throw new IllegalArgumentException("verb '" + verb + "' is already registered -"
                    + " unregister it first to replace the handler");
        }
        LOGGER.debug("Command verb '{}' registered", verb);
    }

    /**
     * Removes a previously registered custom verb handler. Unknown verbs are a no-op; built-in
     * verbs cannot be unregistered.
     *
     * @param verb the custom verb to remove
     * @throws IllegalArgumentException when the verb is a built-in
     */
    public void unregister(String verb) {
        Objects.requireNonNull(verb, "verb must not be null");
        if (BUILT_IN_VERBS.contains(verb)) {
            throw new IllegalArgumentException("verb '" + verb + "' is a built-in verb and"
                    + " cannot be unregistered");
        }
        if (handlers.remove(verb) != null) {
            LOGGER.debug("Command verb '{}' unregistered", verb);
        }
    }

    /** The currently registered verbs (built-ins + custom) — a snapshot copy. */
    public Set<String> verbs() {
        return Set.copyOf(handlers.keySet());
    }

    /**
     * Builds the own-inbox wildcard ({@code ecv1/{device}/{component}/main/cmd/#}, through the
     * topic builder under this component's identity + root mode) and subscribes it on the PRIMARY
     * connection. Best-effort and idempotent: with no resolved component identity (mock/test
     * bring-up) — or on any subscription failure — the inbox logs and disables itself; the
     * component must come up regardless.
     */
    public synchronized void start() {
        if (started || closed) {
            return;
        }
        MessageIdentity identity = configManager.getComponentIdentity();
        if (identity == null) {
            LOGGER.warn("No resolved component identity - the command inbox is disabled");
            return;
        }
        try {
            Uns uns = new Uns(identity, configManager.isTopicIncludeRoot());
            // Pin every scope token to this component's own identity: the site value is consulted
            // only under an effective root mode (D-U25 makes it a no-op otherwise).
            String site = identity.getHier().size() >= 2 ? identity.getHier().get(0).value() : null;
            String filter = uns.filter(UnsClass.CMD, new UnsScope(
                    site, identity.getDevice(), identity.getComponent(), identity.getInstance()));
            this.inboxFilter = filter;
            // ".../cmd/#" -> ".../cmd/" - the verb is the topic's remainder after this prefix.
            // Assigned BEFORE subscribing so a delivery racing the subscribe call sees it.
            this.inboxPrefix = filter.substring(0, filter.length() - 1);
            messagingClient.subscribe(filter, this::handle);
            started = true;
            LOGGER.info("Command inbox subscribed on '{}' (verbs: {})", filter, handlers.keySet());
        } catch (Exception e) {
            LOGGER.warn("Failed to start the command inbox (continuing without it): {}",
                    e.toString());
        }
    }

    /**
     * One received {@code cmd} envelope: extract the verb from the topic, validate the envelope
     * ({@code header.name} must equal the verb), dispatch, reply. Never throws — a malformed or
     * foreign payload is ignored at DEBUG.
     */
    private void handle(String topic, Message message) {
        try {
            synchronized (this) {
                if (closed) {
                    return;
                }
            }
            if (topic == null || !topic.startsWith(inboxPrefix)) {
                // ".../cmd/#" also matches the bare ".../cmd" parent level - nothing to dispatch.
                LOGGER.debug("Ignoring cmd delivery outside the inbox prefix: '{}'", topic);
                return;
            }
            String verb = topic.substring(inboxPrefix.length());
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
        CommandHandler handler = handlers.get(verb);
        if (handler == null) {
            if (wantsReply) {
                LOGGER.debug("Unknown verb '{}' - sending {} error reply", verb, ERR_UNKNOWN_VERB);
                sendReply(request, verb, errorBody(ERR_UNKNOWN_VERB,
                        "verb '" + verb + "' is not registered on this component"));
            } else {
                LOGGER.debug("Ignoring unknown fire-and-forget verb '{}'", verb);
            }
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
            JsonObject body = new JsonObject();
            body.addProperty("ok", true);
            body.add("result", result == null ? new JsonObject() : result);
            sendReply(request, verb, body);
        }
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
        if (started && inboxFilter != null) {
            try {
                messagingClient.unsubscribe(inboxFilter);
            } catch (Exception e) {
                LOGGER.debug("Command-inbox unsubscribe of '{}' failed: {}", inboxFilter,
                        e.toString());
            }
        }
    }
}
