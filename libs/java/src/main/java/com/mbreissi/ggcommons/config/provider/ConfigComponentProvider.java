/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public final class ConfigComponentProvider extends ConfigProvider {
    private static final Logger LOGGER = LogManager.getLogger(ConfigComponentProvider.class);
    public static final String GET_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/get/{ComponentName}";
    public static final String UPDATED_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/{ComponentName}/updated";

    private final String source;
    private final MessagingClient messagingClient;


    ConfigComponentProvider(ConfigManager configManager, MessagingClient messagingClient) {
        super(configManager);
        this.messagingClient = messagingClient;
        // The config-component topics are a wire-protocol contract shared with the
        // configuration-manager component, so they are substituted directly rather
        // than via resolveTemplate (which sanitizes values). This keeps the topic
        // strings byte-identical with the Python/Rust libraries' config-component
        // sources (Python .format / Rust resolve_topic), which also do not sanitize.
        source=resolveProtocolTopic(configManager, GET_TOPIC_TEMPLATE);
        String updated=resolveProtocolTopic(configManager, UPDATED_TOPIC_TEMPLATE);
        messagingClient.subscribe(updated,(topic, msg)->{
            parentConfigManager.applyConfig((JsonObject) msg.getBody());
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
            JsonObject requestPayload = new JsonObject();
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
        return String.format("Config Manager Component (source topic name: %s)", source);
    }

    /**
     * Substitutes {@code {ThingName}}/{@code {ComponentName}} into a config-component
     * topic template without sanitization, mirroring the Rust library's
     * {@code resolve_topic}. Keeps the wire-protocol topic identical across libraries.
     */
    private static String resolveProtocolTopic(ConfigManager configManager, String template) {
        return template
                .replace("{ThingName}", configManager.getThingName())
                .replace("{ComponentName}", configManager.getComponentName());
    }

}