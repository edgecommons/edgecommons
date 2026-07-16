/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.Objects;

/**
 * The library-owned {@code cfg} publisher (UNS-CANONICAL-DESIGN §4.3): announces the component's
 * effective (redacted) configuration on {@code ecv1/{device}/{component}/cfg} — once at
 * startup (after initialization completes) and again on every configuration change. The body is
 * {@code {"config": <effective config, redacted>}}; the {@code cfg} class is reserved, so the
 * publish goes through the privileged {@link com.mbreissi.edgecommons.messaging.ReservedPublisher}
 * seam. (This is the push half only — the {@code republish-cfg} pull verb lands in a later phase.)
 *
 * <p><b>Redaction v1</b> (§4.3): {@code $secret} references are never resolved (the raw config is
 * published as-is, so a {@code {"$secret": …}} ref stays a ref); every value under a
 * {@code credentials} key inside the top-level {@code messaging} section, and every value of a key
 * named {@code password} or {@code pin} (case-insensitive) anywhere, is replaced with
 * {@code "***"}.
 */
public final class EffectiveConfigPublisher implements ConfigurationChangeListener {

    private static final Logger LOGGER = LogManager.getLogger(EffectiveConfigPublisher.class);

    /** The cfg announcement's envelope header name (§4.3). */
    static final String CFG_MESSAGE_NAME = "cfg";
    static final String CFG_MESSAGE_VERSION = "1.0";
    /** The redaction placeholder. */
    static final String REDACTED = "***";

    private final ConfigManager configManager;
    private final MessagingClient messagingClient;
    /** WARN-once flag for the no-resolved-identity (test/subclass bring-up) case. */
    private boolean warnedNoIdentity = false;

    /**
     * Creates the publisher and registers it as a configuration-change listener (each hot reload
     * republishes the effective config). Call {@link #publishNow()} for the startup announcement.
     *
     * @param configManager   the component's config manager (identity + effective config source)
     * @param messagingClient the messaging client whose privileged seam performs the publish
     */
    public EffectiveConfigPublisher(ConfigManager configManager, MessagingClient messagingClient) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        configManager.addConfigChangeListener(this);
    }

    /**
     * Publishes the effective (redacted) configuration to the component's UNS {@code cfg} topic.
     * Best-effort: any failure is logged and swallowed — a cfg announcement must never crash the
     * component. No-op (WARN once) when the component identity is not resolved (mock/test
     * bring-up).
     */
    public void publishNow() {
        try {
            MessageIdentity identity = configManager.getComponentIdentity();
            if (identity == null) {
                if (!warnedNoIdentity) {
                    warnedNoIdentity = true;
                    LOGGER.warn("No resolved component identity - the effective-config publisher is disabled");
                }
                return;
            }
            JsonObject redacted = redactedEffectiveConfig();
            if (redacted == null) {
                LOGGER.warn("No effective configuration available - skipping cfg publish");
                return;
            }
            String topic = new Uns(identity, configManager.isTopicIncludeRoot()).topic(UnsClass.CFG);
            JsonObject body = new JsonObject();
            body.add("config", redacted);
            Message cfgMessage = MessageBuilder.create(CFG_MESSAGE_NAME, CFG_MESSAGE_VERSION)
                    .withPayload(body)
                    .withConfig(configManager)
                    .build();
            messagingClient.reservedPublisher().publish(topic, cfgMessage);
            LOGGER.debug("Published effective (redacted) configuration on '{}'", topic);
        } catch (Exception e) {
            LOGGER.warn("Effective-config publish failed: {}", e.toString());
        }
    }

    @Override
    public boolean onConfigurationChanged() {
        publishNow();
        return true;
    }

    /**
     * The current effective configuration, redacted (redaction v1) — the single snapshot source
     * shared by the {@code cfg} push (this publisher) and the {@code get-configuration} command
     * verb's reply (DESIGN-uns §9.5 Flow B), so both surfaces always agree byte-for-byte.
     *
     * @return the redacted deep copy of the effective config, or {@code null} when no effective
     *         configuration is available (mock/test bring-up)
     */
    public JsonObject redactedEffectiveConfig() {
        JsonObject fullConfig = configManager.getFullConfig();
        return fullConfig == null ? null : redact(fullConfig);
    }

    /**
     * Redaction v1 (§4.3) over a deep copy of the effective config: every value of a key named
     * {@code password} or {@code pin} (case-insensitive, anywhere) and every value of a
     * {@code credentials} key at any depth inside the top-level {@code messaging} section becomes
     * the string {@value #REDACTED}. {@code $secret} refs are untouched (they are never resolved
     * here, so no secret material exists to leak).
     *
     * @param config the effective config (not mutated)
     * @return the redacted deep copy
     */
    static JsonObject redact(JsonObject config) {
        JsonObject copy = config.deepCopy();
        redactObject(copy, false, true);
        return copy;
    }

    /**
     * Recursive redaction walk. {@code inMessaging} is true anywhere under the <b>top-level</b>
     * {@code messaging} section (the {@code messaging.*.credentials} rule); {@code topLevel} is
     * true only for the config root, so a nested {@code messaging} key elsewhere does not trigger
     * the credentials rule.
     */
    private static void redactObject(JsonObject obj, boolean inMessaging, boolean topLevel) {
        for (Map.Entry<String, JsonElement> entry : obj.entrySet()) {
            String key = entry.getKey();
            if (key.equalsIgnoreCase("password") || key.equalsIgnoreCase("pin")
                    || (inMessaging && key.equalsIgnoreCase("credentials"))) {
                entry.setValue(new com.google.gson.JsonPrimitive(REDACTED));
                continue;
            }
            JsonElement value = entry.getValue();
            if (value.isJsonObject()) {
                redactObject(value.getAsJsonObject(),
                        inMessaging || (topLevel && key.equals("messaging")), false);
            } else if (value.isJsonArray()) {
                for (JsonElement item : value.getAsJsonArray()) {
                    if (item.isJsonObject()) {
                        redactObject(item.getAsJsonObject(), inMessaging, false);
                    }
                }
            }
        }
    }
}
