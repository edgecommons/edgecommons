/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.Gson;
import com.google.gson.JsonObject;

import java.io.FileReader;
import java.io.IOException;

/**
 * Configuration class for standalone messaging setup.
 * Handles both local MQTT broker and optional northbound/cloud MQTT connection settings.
 *
 * <p>The nested config sections are immutable records, deserialized by Gson (2.10+)
 * via their canonical constructors. {@code getX()} delegates are retained alongside
 * the record accessors so existing call sites continue to compile unchanged.
 */
public class MessagingConfiguration {
    private MessagingConfig messaging;

    public record MessagingConfig(LocalMqttConfig local, NorthboundMqttConfig northbound) {
        public LocalMqttConfig getLocal() { return local; }
        /** Optional generic northbound/cloud MQTT broker config. */
        public NorthboundMqttConfig getNorthbound() { return northbound; }
    }

    /**
     * The optional per-broker {@code qos} section. Local and generic northbound standalone MQTT
     * support QoS 0/1/2. Greengrass IoT-Core IPC methods accept the native {@link Qos} enum and
     * reject {@link Qos#EXACTLY_ONCE}, because the transport supports only QoS 0/1.
     */
    public record QosDefaults(Integer publish, Integer subscribe) {
        public Integer getPublish() { return publish; }
        public Integer getSubscribe() { return subscribe; }
        private static int valueOrDefault(QosDefaults defaults, boolean publish) {
            if (defaults == null) {
                return 1;
            }
            Integer configured = publish ? defaults.publish() : defaults.subscribe();
            return configured == null ? 1 : configured;
        }

        public int publishOrDefault() {
            return valueOrDefault(this, true);
        }

        public int subscribeOrDefault() {
            return valueOrDefault(this, false);
        }
    }

    public record LocalMqttConfig(String type, String host, int port, String clientId,
                                  QosDefaults qos, CredentialsConfig credentials) {
        public String getType() { return type; }
        public String getHost() { return host; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public QosDefaults getQos() { return qos; }
        public CredentialsConfig getCredentials() { return credentials; }
    }

    public record NorthboundMqttConfig(String type, String host, String endpoint, int port, String clientId,
                                       QosDefaults qos, CredentialsConfig credentials) {
        public String getType() { return type; }
        public String getHost() { return host; }
        public String getEndpoint() { return endpoint; }
        public String getResolvedHost() { return host != null ? host : endpoint; }
        public int getPort() { return port; }
        public String getClientId() { return clientId; }
        public QosDefaults getQos() { return qos; }
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
            JsonObject root = gson.fromJson(reader, JsonObject.class);
            if (root != null
                    && root.has("messaging")
                    && root.get("messaging").isJsonObject()
                    && root.getAsJsonObject("messaging").has("lwt")) {
                throw new IllegalArgumentException(
                        "messaging.lwt is not supported; uns-bridge derives its site Last-Will internally");
            }
            if (root != null
                    && root.has("messaging")
                    && root.get("messaging").isJsonObject()
                    && root.getAsJsonObject("messaging").has("qos")) {
                throw new IllegalArgumentException(
                        "messaging.qos is not supported; configure QoS under messaging.local.qos and messaging.northbound.qos");
            }
            return gson.fromJson(root, MessagingConfiguration.class);
        }
    }
}
