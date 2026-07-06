/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.io.FileReader;
import java.io.IOException;

/**
 * Configuration class for standalone messaging setup.
 * Handles both local MQTT broker and AWS IoT Core connection settings.
 *
 * <p>The nested config sections are immutable records, deserialized by Gson (2.10+)
 * via their canonical constructors. {@code getX()} delegates are retained alongside
 * the record accessors so existing call sites continue to compile unchanged.
 */
public class MessagingConfiguration {
    private MessagingConfig messaging;

    public record MessagingConfig(LocalMqttConfig local, IoTCoreConfig iotCore, LwtConfig lwt) {
        public LocalMqttConfig getLocal() { return local; }
        public IoTCoreConfig getIotCore() { return iotCore; }
        /** The optional MQTT Last-Will-and-Testament section ({@code messaging.lwt}), or null. */
        public LwtConfig getLwt() { return lwt; }
    }

    /**
     * The optional {@code messaging.lwt} section (UNS-CANONICAL-DESIGN §6, D-U9/M7): an MQTT
     * Last-Will-and-Testament registered on the <em>local-broker</em> connection at CONNECT
     * (re-registered automatically on reconnect, since Paho reuses the same connect options).
     * There is deliberately NO retain field — the will is always registered with retain=false.
     *
     * <p>{@code payload} is kept as a raw {@link JsonElement}: a JSON string is published verbatim
     * as UTF-8 bytes; a JSON object is serialized to compact JSON bytes. {@code qos} accepts 0 or 1
     * (schema enum) and defaults to 1 when absent; Gson parses both {@code 1} and a lossless
     * {@code 1.0} into the {@link Integer} component.
     */
    public record LwtConfig(String topic, JsonElement payload, Integer qos) {
        public String getTopic() { return topic; }
        public JsonElement getPayload() { return payload; }
        public Integer getQos() { return qos; }
        /** The effective QoS: the configured value, or the schema default 1 when absent. */
        public int getQosOrDefault() { return qos == null ? 1 : qos; }
    }

    public record LocalMqttConfig(String type, String host, int port, String clientId,
                                  CredentialsConfig credentials) {
        public String getType() { return type; }
        public String getHost() { return host; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public CredentialsConfig getCredentials() { return credentials; }
    }

    public record IoTCoreConfig(String endpoint, int port, String clientId,
                                CredentialsConfig credentials) {
        public String getEndpoint() { return endpoint; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public CredentialsConfig getCredentials() { return credentials; }
    }

    public record CredentialsConfig(String username, String password, String certPath,
                                    String keyPath, String caPath) {
        public String getUsername() { return username; }
        public String getPassword() { return password; }
        public String getCertPath() { return certPath; }
        public String getKeyPath() { return keyPath; }
        public String getCaPath() { return caPath; }
    }

    public MessagingConfig getMessaging() { return messaging; }

    public static MessagingConfiguration loadFromFile(String configPath) throws IOException {
        Gson gson = new Gson();
        try (FileReader reader = new FileReader(configPath)) {
            return gson.fromJson(reader, MessagingConfiguration.class);
        }
    }
}
