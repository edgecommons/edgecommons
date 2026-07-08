/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config.provider;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * The {@code CONFIG_COMPONENT} config source: fetches this component's configuration from an
 * external configuration-manager component over the UNS config rendezvous (UNS-CANONICAL-DESIGN
 * §4.3, D-U19 Flow A) and receives pushed updates on the component's own command inbox.
 *
 * <h2>Wire contract (a convention shared with the config server)</h2>
 * <ul>
 *   <li><b>Flow A — GET</b>: a request to {@value #GET_TOPIC_TEMPLATE} (with {@code {device}} =
 *       the sanitized resolved thing name). {@code config} is a <b>reserved-by-convention logical
 *       component name</b> — the config server is the sole subscriber and replies via
 *       {@code reply_to} with the configuration as the message body. Because this request runs
 *       during config bootstrap — <em>before</em> the {@link ConfigManager} (and therefore the
 *       component identity) exists — it carries no envelope identity; the requester
 *       <b>self-identifies in the body</b> with {@code {"component": "<short name>"}} (§1.5).</li>
 *   <li><b>set-config push</b>: the server pushes a fire-and-forget {@code cmd} (no
 *       {@code reply_to} — a notification-style command) to the component's own inbox
 *       {@value #SET_CONFIG_TOPIC_TEMPLATE} (with {@code {component}} = the sanitized short
 *       component name, instance {@code main}); the body is the new configuration, applied via
 *       {@link ConfigManager#applyConfig}.</li>
 * </ul>
 *
 * <h2>Pre-identity bootstrap (why this class must not touch ConfigManager)</h2>
 * This provider is constructed by {@code ConfigManagerFactory} with a <b>null</b>
 * {@link ConfigManager} — the manager does not exist until this provider has loaded the config.
 * The topics are therefore minted locally from the resolved thing name and the component name
 * handed to the constructor (the same inputs {@code ConfigManager} later uses), never from
 * {@code ConfigManager}/{@code getComponentIdentity()}/{@code Uns}. Both tokens pass through the
 * normative UNS token sanitizer ({@link ConfigManager#sanitize} — a static utility, not an
 * instance dependency). The manager is back-filled onto {@link #parentConfigManager} by the
 * {@code ConfigManager} constructor once it exists; a {@code set-config} push racing ahead of
 * that attach is logged and dropped (the server's pushes are not applicable before the initial
 * configuration is loaded anyway).
 *
 * <p>These are {@code cmd}-class topics — not library-reserved — so they publish through the
 * ordinary messaging surface (no {@code ReservedPublisher} seam) and pass the reserved-topic
 * guard.
 */
public final class ConfigComponentProvider extends ConfigProvider {
    private static final Logger LOGGER = LogManager.getLogger(ConfigComponentProvider.class);

    /**
     * Flow-A GET request topic (§4.3): the config server's rendezvous under the
     * reserved-by-convention logical component name {@code config}, instance {@code main}.
     */
    public static final String GET_TOPIC_TEMPLATE = "ecv1/{device}/config/main/cmd/get-configuration";

    /**
     * The pushed {@code set-config} command's topic — this component's OWN inbox (§4.3): the
     * server-to-component push replacing the legacy {@code .../updated} subscription.
     */
    public static final String SET_CONFIG_TOPIC_TEMPLATE = "ecv1/{device}/{component}/main/cmd/set-config";

    private final String source;
    private final String setConfigTopic;
    /** The sanitized short component name — the body self-identification token (§1.5). */
    private final String componentToken;
    private final MessagingClient messagingClient;


    /**
     * Creates the config-source client and subscribes to the component's {@code set-config} inbox.
     *
     * @param configManager   the parent config manager — <b>null during bootstrap</b> (the
     *                        production path: this provider loads the config the manager is then
     *                        built from); back-filled by the {@code ConfigManager} constructor
     * @param componentName   the component name from the platform inputs (full reverse-DNS or
     *                        already-short); reduced to the short name and sanitized
     * @param thingName       the resolved thing name (from the {@code PlatformResolver} identity
     *                        chain — the same source {@code ConfigManager} later uses); sanitized
     *                        into the {@code {device}} token
     * @param messagingClient the messaging client the GET request and the push subscription ride on
     */
    ConfigComponentProvider(ConfigManager configManager, String componentName, String thingName,
                            MessagingClient messagingClient) {
        super(configManager);
        this.messagingClient = messagingClient;
        // Mint the UNS tokens locally (NO ConfigManager instance / Uns dependency — identity is
        // not resolved yet): device = sanitized resolved thing name, component = sanitized short
        // name, mirroring the {ThingName}/{ComponentName} template semantics and §1.5 steps 4-5.
        String deviceToken = ConfigManager.sanitize(thingName);
        this.componentToken = ConfigManager.sanitize(shortComponentName(componentName));
        this.source = mintTopic(GET_TOPIC_TEMPLATE, deviceToken, componentToken);
        this.setConfigTopic = mintTopic(SET_CONFIG_TOPIC_TEMPLATE, deviceToken, componentToken);
        messagingClient.subscribe(setConfigTopic, (topic, msg) -> {
            ConfigManager manager = parentConfigManager;
            if (manager == null) {
                // Bootstrap race: the push arrived before the ConfigManager was constructed and
                // attached. There is nothing to apply it to; the initial GET (in flight) delivers
                // the current config, and the server can re-push after startup.
                LOGGER.warn("Dropping set-config push on '{}' received before configuration "
                        + "bootstrap completed.", topic);
                return;
            }
            manager.applyConfigFromProvider((JsonObject) msg.getBody());
        });
    }

    @Override
    public JsonObject loadConfiguration() {
        // This bootstrap request now carries the framework-owned request() deadline
        // (UNS-CANONICAL-DESIGN §5; the provider's built-in 30 s, since the config-model default
        // is not loaded yet). When the deadline fires it settles the request — the reply
        // subscription is unsubscribed and the future completes exceptionally with a
        // TimeoutException — so a retry must issue a FRESH request (waiting again on the settled
        // future could never succeed). Both timeout signals (the framework deadline surfacing as
        // ExecutionException(TimeoutException) and get()'s own TimeoutException when the deadline
        // is disabled) take the same 3-attempt retry path the previous implementation had.
        int attemptCount = 0;
        while (true) {
            // The requester self-identifies in the BODY (§1.5): during bootstrap there is no
            // ConfigManager, so the envelope carries no identity element — the config server
            // routes on {"component"} instead. (withConfig(null) builds an identity-less message.)
            JsonObject requestPayload = new JsonObject();
            requestPayload.addProperty("component", componentToken);
            Message request = MessageBuilder.create("GetConfiguration", "1.0")
                    .withPayload(requestPayload)
                    .withConfig(this.parentConfigManager)
                    .build();
            final ReplyFuture replyFuture = messagingClient.request(source, request);
            try {
                Message replyMessage = replyFuture.get(30, TimeUnit.SECONDS);
                return (JsonObject) replyMessage.getBody();
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                LOGGER.fatal("Encountered InterruptedException. Unable to load configuration using Greengrass IPC.");
                throw new RuntimeException("Interrupted while loading configuration using Greengrass IPC.", e);
            } catch (ExecutionException e) {
                if (!(e.getCause() instanceof TimeoutException)) {
                    LOGGER.fatal("Encountered ExecutionException. Unable to load configuration using Greengrass IPC.");
                    throw new RuntimeException("Failed to load configuration using Greengrass IPC.", e);
                }
                // The framework deadline fired (and already cleaned up the reply subscription).
                attemptCount = onTimeout(attemptCount, e);
            } catch (TimeoutException e) {
                // get() expired before any framework deadline (e.g. deadline disabled): settle and
                // clean up the abandoned request before re-issuing.
                messagingClient.cancelRequest(replyFuture);
                attemptCount = onTimeout(attemptCount, e);
            }
        }
    }

    /** The shared 3-attempt timeout policy: increments, throws on the 3rd attempt, else warns. */
    private static int onTimeout(int attemptCount, Exception e) {
        attemptCount++;
        if (attemptCount == 3) {
            LOGGER.fatal("Failed to retrieve configuration from configuration manager component after {} tries.", attemptCount);
            throw new RuntimeException("Failed to retrieve configuration from configuration manager component after " + attemptCount + " tries.", e);
        }
        LOGGER.warn("Failed to retrieve configuration from configuration manager component.  Retrying ({})", attemptCount);
        return attemptCount;
    }

    @Override
    public String getConfigSource() {
        return String.format("Config Manager Component (get: %s, set-config inbox: %s)",
                source, setConfigTopic);
    }

    /**
     * Reduces a component name to its short form (the segment after the last {@code .}), the
     * existing {@code {ComponentName}} semantics — mirroring {@code ConfigManagerFactory}'s
     * derivation, which cannot be used here because it runs after this provider loads the config.
     */
    private static String shortComponentName(String componentName) {
        return componentName != null && componentName.contains(".")
                ? componentName.substring(componentName.lastIndexOf('.') + 1)
                : componentName;
    }

    /**
     * Mints a concrete rendezvous topic from a template by substituting the pre-sanitized
     * {@code {device}}/{@code {component}} tokens. A deliberately local helper: the UNS builder
     * ({@code gg.getUns()}) is unavailable during config bootstrap (§1.5), and these {@code cmd}
     * topics need no reserved-class seam.
     */
    private static String mintTopic(String template, String deviceToken, String componentToken) {
        return template
                .replace("{device}", deviceToken)
                .replace("{component}", componentToken);
    }

}
