/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.Gson;
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

    public record MessagingConfig(LocalMqttConfig local, IoTCoreConfig iotCore) {
        public LocalMqttConfig getLocal() { return local; }
        public IoTCoreConfig getIotCore() { return iotCore; }
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
